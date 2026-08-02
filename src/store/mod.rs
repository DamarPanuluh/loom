//! SQLite graph store — durable persistence behind a focused interface.
//!
//! Plane: this is the only module that touches SQL. Callers see typed nodes,
//! edges, facets, and verdicts; the schema, ids, timestamps, and write-time
//! integrity checks are hidden here.
//!
//! Integrity guarantees enforced at this boundary (the write boundary):
//! - INV-4: `independent` verdicts require non-empty evidence.
//! - INV-5: derived status is written ONLY by `set_derived_status`; asserted
//!   verdicts ONLY by `record_verdict`. Neither path crosses the truth-class line.
//! - INV-6: passing/failing/independent verdicts require non-empty criterion + evidence.
//! - Edge typing: every edge is validated against the edge-kind registry.

use crate::model::*;
use crate::registry;
use crate::{Result, GRAPH_DB, LOOM_DIR, SCHEMA_VERSION};
use anyhow::{anyhow, bail, Context};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Identity of a graph — what other graphs reference in a federation. Travels
/// in the export.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub graph_id: String,
    pub name: String,
    pub schema_version: u32,
    pub observed: bool,
}

/// A read-only projection of the whole graph, used by export. All collections
/// are sorted by stable keys so serialization is deterministic.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub identity: Identity,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Asserted claims and the anchors behind them. These travel so verification
    /// strength can travel — but strength is RECOMPUTED on import, because a
    /// claim of `verified` from another machine is a claim until this filesystem
    /// agrees.
    pub facts: Vec<crate::evidence::Fact>,
    pub evidence: Vec<crate::evidence::EvidenceRow>,
    pub facets: Vec<Facet>,
    pub tags: Vec<Tag>,
    /// Portable repo config from the meta table — ONLY the allowlisted keys in
    /// [`PORTABLE_META_KEYS`]. Never a blind meta dump: identity keys travel as
    /// top-level export fields, and anything not allowlisted stays local.
    pub config: std::collections::BTreeMap<String, String>,
}

/// What a [`Store::restore`] did with facets/tags whose target node/edge is not
/// present in the imported snapshot. Two disjoint outcomes:
///
/// - **Soft refs** — asserted `adjudication` verdicts on a derived Finding id.
///   The finding re-materializes (same deterministic id) on the next `sync`,
///   so the verdict is a valid dangling reference, not corruption. Always
///   kept, counted in `preserved_soft_refs`.
/// - **True orphans** — any other facet/tag with an absent target. A strict
///   `restore` refuses them; `restore_repairing` drops and reports them here.
#[derive(Debug, Default, Clone)]
pub struct RestoreReport {
    /// Soft-ref facets kept despite an absent target (durable adjudications).
    pub preserved_soft_refs: usize,
    /// Orphan facts dropped by repair, as `(subject_kind, subject_id, claim)`.
    pub dropped_facts: Vec<(String, String, String)>,
    /// Orphan facets dropped by repair, as `(target_kind, target_id, key)`.
    pub dropped_facets: Vec<(String, String, String)>,
    /// Orphan tags dropped by repair, as `(target_kind, target_id, term)`.
    pub dropped_tags: Vec<(String, String, String)>,
}

/// Meta keys that travel with the export. Each is repo-portable configuration
/// (what to track, what to ignore, the layer order, registered scan adapters,
/// structural finding thresholds) — without these an imported graph silently
/// loses its coverage exclusions and detectors. Local-only or identity meta
/// keys must NOT be added here.
pub const PORTABLE_META_KEYS: &[&str] = &[
    "layer_order",
    "ignores",
    "codefile_globs",
    "observed_globs",
    "scan_adapters",
    "thresholds",
    "evidence_policy",
    "upstream_graphs",
];

/// The SQLite-backed graph store. Holds an advisory lock for its lifetime —
/// exclusive for a write open, shared for a read-only open — so writers never
/// overlap and a reader never observes a half-migrated schema.
pub struct Store {
    conn: Connection,
    root: PathBuf,
    agent: std::cell::Cell<Agent>,
    _lock: File,
}

/// The acting agent. Solo (default) may drive every lane; a declared lane is
/// enforced at the write boundary (a quality agent cannot write a builder edge).
/// Evidence/integrity gates apply regardless of agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Solo,
    Lane(registry::OwnerRole),
}

impl Agent {
    /// Parse `LOOM_AGENT` from the environment. Unset (or the bare `llm`/`solo`
    /// sentinel) is Solo; a declared lane is that lane; anything else is an
    /// error. A typo like `llm:qualtiy` MUST fail closed — never silently
    /// disable the lane gate by falling through to Solo.
    pub fn from_env() -> Result<Agent> {
        match std::env::var("LOOM_AGENT") {
            Ok(v) => Agent::parse(&v),
            Err(_) => Ok(Agent::Solo),
        }
    }

    pub fn parse(v: &str) -> Result<Agent> {
        let v = v.trim();
        if v.is_empty() || v == "llm" || v == "solo" {
            return Ok(Agent::Solo);
        }
        let lane = v.strip_prefix("llm:").unwrap_or(v);
        match lane {
            "builder" => Ok(Agent::Lane(registry::OwnerRole::Builder)),
            "analyzer" => Ok(Agent::Lane(registry::OwnerRole::Analyzer)),
            "fixer" => Ok(Agent::Lane(registry::OwnerRole::Fixer)),
            "validator" => Ok(Agent::Lane(registry::OwnerRole::Validator)),
            "quality" => Ok(Agent::Lane(registry::OwnerRole::Quality)),
            other => bail!(
                "unrecognized LOOM_AGENT '{v}' — use llm:<builder|analyzer|fixer|validator|quality>, or leave unset for solo (got lane '{other}')"
            ),
        }
    }
}

const SCHEMA: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE node (
    id          TEXT PRIMARY KEY,
    node_type   TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT '',
    truth_class TEXT NOT NULL DEFAULT 'asserted' CHECK (truth_class IN ('derived','asserted')),
    body        TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_node_type ON node(node_type);
CREATE INDEX idx_node_name ON node(name);

CREATE TABLE edge (
    id           TEXT PRIMARY KEY,
    from_id      TEXT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    to_id        TEXT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    truth_class  TEXT NOT NULL CHECK (truth_class IN ('derived','asserted')),
    status       TEXT NOT NULL DEFAULT 'uninspected',
    criterion    TEXT NOT NULL DEFAULT '',
    evidence     TEXT NOT NULL DEFAULT '',
    confidence   REAL NOT NULL DEFAULT 0,
    depends_on   TEXT NOT NULL DEFAULT '[]',
    inspected_by TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE (from_id, to_id, kind)
);
CREATE INDEX idx_edge_queue ON edge(truth_class, status);
CREATE INDEX idx_edge_kind  ON edge(kind, status);
CREATE INDEX idx_edge_from  ON edge(from_id, kind);
CREATE INDEX idx_edge_to    ON edge(to_id, kind);

CREATE TABLE facet (
    target_id   TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('node','edge')),
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    truth_class TEXT NOT NULL CHECK (truth_class IN ('derived','asserted')),
    PRIMARY KEY (target_id, target_kind, key)
);

CREATE TABLE tag (
    target_id   TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('node','edge')),
    term        TEXT NOT NULL,
    PRIMARY KEY (target_id, target_kind, term)
);

CREATE TABLE tag_vocabulary (
    term        TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL
);
"#;

impl Store {
    /// Initialize a fresh graph at `root/.loom/graph.sqlite`. Idempotent: if the
    /// store already exists, opens it and backfills identity defaults.
    pub fn init(root: &Path, name: Option<&str>, observed: bool) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir)
            .with_context(|| format!("creating {}", loom_dir.display()))?;
        let db_path = loom_dir.join(GRAPH_DB);
        let lock = acquire_lock(&loom_dir, true)?;
        let fresh = !db_path.exists();
        let mut conn =
            Connection::open(&db_path).with_context(|| format!("opening {}", db_path.display()))?;
        configure(&conn)?;
        apply_schema_migrations(&mut conn)?;
        if fresh {
            let default_name = name
                .map(str::to_string)
                .or_else(|| {
                    root.canonicalize()
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                })
                .unwrap_or_else(|| "loom".to_string());
            let (gid, now) = id_and_now(&conn)?;
            // One atomic seed: either the graph is born with a full identity or
            // the file stays empty. A half-written meta table (a graph_id with no
            // schema_version) would leave `open` unable to tell fresh from broken.
            let tx = conn.transaction()?;
            let set = |k: &str, v: &str| -> Result<()> {
                tx.execute("INSERT INTO meta(key,value) VALUES (?1,?2)", params![k, v])?;
                Ok(())
            };
            set("graph_id", &gid)?;
            set("name", &default_name)?;
            set("schema_version", &SCHEMA_VERSION.to_string())?;
            set("observed", if observed { "1" } else { "0" })?;
            set("created_at", &now)?;
            tx.commit()?;
        } else if name.is_some() || observed {
            // Backfill identity on an existing graph.
            if let Some(n) = name {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES ('name',?1)
                     ON CONFLICT(key) DO UPDATE SET value=?1",
                    params![n],
                )?;
            }
            if observed {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES ('observed','1')
                     ON CONFLICT(key) DO UPDATE SET value='1'",
                    [],
                )?;
            }
        }
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            agent: std::cell::Cell::new(Agent::from_env()?),
            _lock: lock,
        })
    }

    /// Open an existing graph at `root/.loom/graph.sqlite`.
    pub fn open(root: &Path) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        let db_path = loom_dir.join(GRAPH_DB);
        if !db_path.exists() {
            bail!(
                "no loom graph at {} — run `loom init` first",
                db_path.display()
            );
        }
        let lock = acquire_lock(&loom_dir, true)?;
        let mut conn = Connection::open(&db_path)?;
        configure(&conn)?;
        apply_schema_migrations(&mut conn)?;
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            agent: std::cell::Cell::new(Agent::from_env()?),
            _lock: lock,
        })
    }

    /// Open an existing graph read-only. Takes a SHARED advisory lock, so many
    /// readers proceed together and only wait while a writer holds the boundary,
    /// and sets `query_only` so no read command can mutate the graph. Never
    /// migrates: a schema behind `SCHEMA_VERSION` is reported, not silently
    /// upgraded, because migration is a write and must go through a write open.
    pub fn open_read(root: &Path) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        let db_path = loom_dir.join(GRAPH_DB);
        if !db_path.exists() {
            bail!(
                "no loom graph at {} — run `loom init` first",
                db_path.display()
            );
        }
        let lock = acquire_lock(&loom_dir, false)?;
        let conn = Connection::open(&db_path)?;
        configure_read(&conn)?;
        // `user_version` is the migration stamp maintained by the write path
        // (`apply_schema_migrations`); a read open must not migrate, so a mismatch
        // is an explicit "run a write command first", never a silent read of an
        // older shape.
        let user_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        // Older graph: a write command migrates it forward. NEWER graph: this
        // binary is behind, and there is nothing the operator can run to fix
        // that here — telling them to `loom sync` invites an action that fails
        // with raw migration-library jargon, which is how a clear failure
        // becomes a confusing one.
        if user_version > SCHEMA_VERSION {
            bail!(
                "this graph is v{user_version}; this loom understands v{SCHEMA_VERSION}. \
                 It was written by a newer loom — upgrade this one \
                 (`cargo install --path .`) rather than migrating the graph, which \
                 only ever moves forward"
            );
        }
        if user_version != SCHEMA_VERSION {
            bail!(
                "graph schema (v{user_version}) needs migration to v{SCHEMA_VERSION} — run a \
                 write command (e.g. `loom sync`) once to migrate before read-only commands"
            );
        }
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            agent: std::cell::Cell::new(Agent::from_env()?),
            _lock: lock,
        })
    }

    /// Walk up from `start` to find the nearest ancestor containing `.loom/`.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            if dir.join(LOOM_DIR).join(GRAPH_DB).exists() {
                return Some(dir.to_path_buf());
            }
            cur = dir.parent();
        }
        None
    }

    /// The acting agent.
    pub fn agent(&self) -> Agent {
        self.agent.get()
    }

    /// Override the acting agent (CLI sets this from `LOOM_AGENT`; tests set it
    /// explicitly to exercise lane gates without env races).
    pub fn set_agent(&self, agent: Agent) {
        self.agent.set(agent);
    }

    /// Lane gate: a declared lane may only write edges/verdicts it owns. Solo
    /// drives every lane. `sync` is implicit (derived paths never call this).
    fn check_lane(&self, owner: registry::OwnerRole) -> Result<()> {
        match self.agent.get() {
            Agent::Solo => Ok(()),
            Agent::Lane(role) if role == owner => Ok(()),
            Agent::Lane(role) => bail!(
                "lane gate: agent '{}' may not write '{}'-owned facts",
                role.as_str(),
                owner.as_str()
            ),
        }
    }

    /// Governed research is analysis work; solo remains an unrestricted driver.
    pub fn require_research_owner(&self) -> Result<()> {
        self.check_lane(registry::OwnerRole::Analyzer)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn identity(&self) -> Result<Identity> {
        let get = |k: &str| -> Result<String> {
            self.conn
                .query_row("SELECT value FROM meta WHERE key=?1", params![k], |r| {
                    r.get::<_, String>(0)
                })
                .with_context(|| format!("reading meta '{k}'"))
        };
        // `observed` did not exist in the earliest graphs, so absence remains a
        // backward-compatible `false`. A present value is strict: swallowing a
        // database error or typo here could silently turn a monitor into an
        // owned graph and enable its build/fix lanes.
        let observed_raw = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='observed'", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .context("reading meta 'observed'")?;
        let observed = match observed_raw.as_deref() {
            None | Some("0") => false,
            Some("1") => true,
            Some(value) => bail!("meta.observed is malformed: expected '0' or '1', got '{value}'"),
        };
        Ok(Identity {
            graph_id: get("graph_id")?,
            name: get("name")?,
            schema_version: get("schema_version")?
                .parse()
                .context("meta.schema_version is malformed")?,
            observed,
        })
    }

    /// Set the graph's mode: `observed` = maps code the driver does not own
    /// (discovery-only; build/fix/coverage/elaborate lanes disabled), `owned` =
    /// the normal build-and-prove mode. This is the post-init counterpart to
    /// `loom init --observed`; `sync` never changes it, because scanning files
    /// says nothing about who owns them. Returns the value actually set.
    pub fn set_observed(&self, observed: bool) -> Result<bool> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES ('observed',?1)
             ON CONFLICT(key) DO UPDATE SET value=?1",
            params![if observed { "1" } else { "0" }],
        )?;
        Ok(observed)
    }

    /// Begin an explicit transaction for a multi-mutation batch (`loom apply`).
    /// Uses `unchecked_transaction` so it composes with the store's `&self`
    /// write methods; drop it without `commit` to roll the whole batch back —
    /// the atomicity guarantee behind `loom apply`.
    pub fn begin(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self.conn.unchecked_transaction()?)
    }

    /// Open a transaction ONLY when the connection is not already inside one.
    ///
    /// A low-level write that needs its several statements to be atomic still has
    /// to compose with an outer `begin()` batch (`loom apply`), where a nested
    /// `BEGIN` is an error. When an ambient transaction is already open this
    /// returns `None` and the outer batch owns atomicity; otherwise it returns a
    /// fresh tx the caller must `commit`. Either way the statements run on
    /// `self.conn`, so they land in whichever transaction is active.
    pub(crate) fn maybe_tx(&self) -> Result<Option<rusqlite::Transaction<'_>>> {
        if self.conn.is_autocommit() {
            Ok(Some(self.conn.unchecked_transaction()?))
        } else {
            Ok(None)
        }
    }
}

mod derived;
#[allow(unused_imports)] // consumed by diagnostics_cmd via crate::store::
pub(crate) use derived::{DebtPromotionInput, DebtPromotionResult};

mod edges;
mod facets;
/// The write boundary: every asserted fact enters through `assert_fact`.
pub mod facts;
mod nodes;
pub use facts::{edge_verdict, Assertion, FactView, Subject};

// ---- helpers -------------------------------------------------------------

/// Migration 3 — the evidence spine.
///
/// `fact` becomes the home of every asserted claim. Without it `assert_fact`
/// cannot be a chokepoint: adjudication and ratification lived in `facet`, and
/// `set_facet` is a public primitive every command reaches for directly.
///
/// The verdict columns leave `edge` and come back as a VIEW projected from the
/// fact. Readers keep working unchanged; writers lose the ability to set them at
/// all, because there is no longer a column to write.
///
/// This is a hardcut, not a data migration: every asserted verdict is reset,
/// because not one of them was anchored to anything loom could re-check. The
/// ladder going red afterwards is the point.
const MIGRATION_3_EVIDENCE_SPINE: &str = r#"
CREATE TABLE fact (
    id           TEXT PRIMARY KEY,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('node','edge')),
    subject_id   TEXT NOT NULL,
    claim        TEXT NOT NULL CHECK (claim IN ('verdict','observation','adjudication','ratification')),
    state        TEXT NOT NULL,
    criterion    TEXT NOT NULL DEFAULT '',
    verification TEXT NOT NULL CHECK (verification IN ('verified','cited','claimed','expired')),
    confidence   REAL NOT NULL DEFAULT 0,
    asserted_by  TEXT NOT NULL DEFAULT '',
    asserted_at  TEXT NOT NULL,
    stale        TEXT NOT NULL DEFAULT '',
    UNIQUE (subject_kind, subject_id, claim)
);
CREATE INDEX idx_fact_subject      ON fact(subject_kind, subject_id);
CREATE INDEX idx_fact_verification ON fact(verification, claim);
CREATE INDEX idx_fact_claim_state  ON fact(claim, state);

CREATE TABLE evidence (
    id            TEXT PRIMARY KEY,
    fact_id       TEXT NOT NULL REFERENCES fact(id) ON DELETE CASCADE,
    payload       TEXT NOT NULL DEFAULT '{}',
    kind          TEXT NOT NULL CHECK (kind IN ('run','span','journal','claim')),
    recorded_at   TEXT NOT NULL,
    holds         INTEGER NOT NULL DEFAULT 1,
    expiry_reason TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_evidence_fact  ON evidence(fact_id, kind);
CREATE INDEX idx_evidence_holds ON evidence(holds);

ALTER TABLE edge DROP COLUMN criterion;
ALTER TABLE edge DROP COLUMN evidence;
ALTER TABLE edge DROP COLUMN confidence;
ALTER TABLE edge DROP COLUMN inspected_by;

CREATE VIEW edge_view AS
SELECT e.id, e.from_id, e.to_id, e.kind, e.truth_class, e.status,
       e.depends_on, e.created_at, e.updated_at,
       COALESCE(f.criterion, '')   AS criterion,
       COALESCE(f.confidence, 0.0) AS confidence,
       COALESCE(f.asserted_by, '') AS inspected_by
FROM edge e
LEFT JOIN fact f
  ON f.subject_kind = 'edge' AND f.subject_id = e.id AND f.claim = 'verdict';

UPDATE edge SET status = 'uninspected' WHERE truth_class = 'asserted';
DELETE FROM facet WHERE key IN (
    'evidence_spans', 'stale_cause', 'adjudication',
    'ratification', 'ratified_by', 'ratified_at', 'ratified_presence'
);
"#;

/// Strip `proof_level` from validation bodies.
///
/// v3 made proof strength DERIVED — computed from what a proof actually does —
/// and deleted the flag that let a caller claim it. The stored values stayed
/// behind, so `loom validation show` printed `"proof_level":"L5"` in the body
/// directly above a derived `strength: S1`. Nothing reads it; it is inert data
/// that contradicts the number beside it, and it travels in every export.
const MIGRATION_4_DROP_CLAIMED_PROOF_LEVEL: &str = r#"
UPDATE node
   SET body = json_remove(body, '$.proof_level')
 WHERE node_type = 'validation'
   AND json_extract(body, '$.proof_level') IS NOT NULL;
"#;

/// Remove delegated ratification. Policy-authored facts cease to establish
/// wantedness, and their evidence is removed by the fact foreign key cascade.
/// Journal entries are intentionally append-only and live outside SQLite, so
/// the historical acts remain available to audit.
const MIGRATION_5_DROP_RATIFY_POLICIES: &str = r#"
DELETE FROM meta WHERE key = 'ratify_policies';
DELETE FROM fact
 WHERE claim = 'ratification'
   AND asserted_by LIKE 'policy:%';
"#;

/// Stamped into the lock-contention error so a RUNNER can recognise its own
/// infrastructure failing, rather than attributing it to the code under test.
/// A child blocked on a lock its parent holds exits non-zero exactly like a
/// failing test, and that ambiguity once made loom record a false failing
/// verdict against a behavior that passes.
pub const LOCK_CONTENTION_MARKER: &str = "loom-lock-contention";

/// Process exit code loom uses when it aborts because another loom process holds
/// the graph. A parent that spawned this loom keys on this code — not on a
/// stderr substring, which a failing test could print verbatim and be
/// misclassified — to tell "loom's own lock got in the way" (unobservable, and
/// never a verdict about the code) from a genuine non-zero failure. 75 is
/// `EX_TEMPFAIL` from sysexits: a temporary failure, retry invited.
pub const LOCK_CONTENTION_EXIT_CODE: i32 = 75;

fn schema_migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(SCHEMA),
        M::up(
            "CREATE INDEX IF NOT EXISTS idx_tag_term ON tag(term);
             CREATE INDEX IF NOT EXISTS idx_facet_key_value ON facet(key, value);",
        ),
        M::up(MIGRATION_3_EVIDENCE_SPINE),
        M::up(MIGRATION_4_DROP_CLAIMED_PROOF_LEVEL),
        M::up(MIGRATION_5_DROP_RATIFY_POLICIES),
        M::up("SELECT 1;"),
    ])
}

fn apply_schema_migrations(conn: &mut Connection) -> Result<()> {
    adopt_legacy_schema_version(conn)?;
    // Refuse a graph from the future BEFORE handing it to the migrator, which
    // reports it as "migration number that is too high" — an accurate sentence
    // about its own internals and a useless one to the person holding an old
    // binary. Migrations only move forward; the fix is always to upgrade loom.
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if user_version > SCHEMA_VERSION {
        bail!(
            "this graph is v{user_version}; this loom understands v{SCHEMA_VERSION}. \
             It was written by a newer loom — upgrade this one \
             (`cargo install --path .`). The graph is untouched."
        );
    }
    schema_migrations()
        .to_latest(conn)
        .context("migrating graph schema")?;
    // Keep the portable identity stamp aligned with the migration counter when
    // meta already exists (re-open / upgrade). Fresh init inserts schema_version
    // itself after this returns.
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let has_schema_meta = conn
        .query_row("SELECT 1 FROM meta WHERE key='schema_version'", [], |_| {
            Ok(true)
        })
        .optional()?
        .unwrap_or(false);
    if has_schema_meta {
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='schema_version'",
            params![user_version.to_string()],
        )?;
    }
    Ok(())
}

fn adopt_legacy_schema_version(conn: &Connection) -> Result<()> {
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if user_version != 0 {
        return Ok(());
    }

    let has_meta = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !has_meta {
        return Ok(());
    }

    let legacy_schema_version = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?;

    let legacy_schema_version = legacy_schema_version
        .map(|raw| {
            raw.parse::<u32>()
                .with_context(|| format!("invalid persisted schema_version '{raw}'"))
        })
        .transpose()?;

    if legacy_schema_version == Some(SCHEMA_VERSION) {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(())
}

/// Connection setup for a read-only open. Sets the busy timeout and enforces
/// `query_only`, so a mis-routed read command fails loudly instead of writing.
/// Deliberately does NOT set `journal_mode` (a write) or run migrations — a read
/// open never mutates the file.
fn configure_read(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(())
}

fn acquire_lock(loom_dir: &Path, exclusive: bool) -> Result<File> {
    let lock_path = loom_dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening lock {}", lock_path.display()))?;
    // Retry briefly: a just-dropped lock from a prior open in this or another
    // process can lag a few ms before the OS releases it. WAL + busy_timeout
    // handle real query concurrency; this flock only guards the open boundary
    // (schema migration above all). Writers take it exclusive; a read-only open
    // takes it shared, so N readers proceed together and only wait while a writer
    // actually holds the boundary.
    let mut wait = std::time::Duration::from_millis(5);
    for attempt in 0..40 {
        let acquired = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match acquired {
            Ok(()) => return Ok(file),
            // A held lock may release any moment — retry with backoff.
            Err(std::fs::TryLockError::WouldBlock) if attempt < 39 => {
                std::thread::sleep(wait);
                if wait < std::time::Duration::from_millis(50) {
                    wait *= 2;
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => break,
            // A real I/O error will not heal by waiting.
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("locking {}", lock_path.display()));
            }
        }
    }
    bail!(
        "{LOCK_CONTENTION_MARKER}: graph is locked by another loom process (waiting for {} access)",
        if exclusive { "write" } else { "read" }
    )
}

/// Sentinel timestamp for derived rows. Derived data is recomputed by sync, so
/// its creation time is meaningless; a fixed sentinel keeps wipe+rebuild output
/// byte-identical (INV-2).
const DERIVED_TS: &str = "";

/// Deterministic FNV-1a 64-bit digest over the joined parts. Returns the bare
/// 16-hex digest (no prefix). Callers choose a plane prefix (`d` for derived
/// rows, `c` for debt clusters, `p` for promoted findings).
pub fn fnv_hex_digest(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h ^= 0x1f;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        for b in p.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Deterministic, content-addressed id for derived data (FNV-1a 64-bit over the
/// joined parts). The same inputs always yield the same id, so a wiped-and-
/// rebuilt derived plane is byte-identical.
fn derived_id(parts: &[&str]) -> String {
    format!("d{}", fnv_hex_digest(parts))
}

/// Whether an id has the content-addressed shape used by derived nodes.
/// Centralized because dormant derived-Finding adjudications are the sole
/// intentional missing-subject reference in restore and audit.
pub(crate) fn is_derived_node_id(id: &str) -> bool {
    id.strip_prefix('d').is_some_and(|digest| {
        digest.len() == 16
            && digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    })
}

/// Generate a fresh 128-bit hex id and an RFC3339 timestamp in one query, using
/// SQLite's own functions (no external rng/clock crate).
fn id_and_now(conn: &Connection) -> Result<(String, String)> {
    conn.query_row(
        "SELECT lower(hex(randomblob(16))), strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .map_err(Into::into)
}

fn now(conn: &Connection) -> Result<String> {
    conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
        r.get::<_, String>(0)
    })
    .map_err(Into::into)
}

fn parse_named<T: std::str::FromStr>(row: &rusqlite::Row, col: &str) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(col)?;
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

fn parse_col<T: std::str::FromStr>(row: &rusqlite::Row, idx: usize) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(idx)?;
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

/// Column list for node SELECTs. Order-independent (mappers read by name) but
/// kept as one constant so every query selects the full row.
const NODE_COLS: &str =
    "id,node_type,name,description,status,truth_class,body,created_at,updated_at";

/// Column list for edge SELECTs. Read from `edge_view`, never `edge`: the
/// verdict fields (`criterion`, `confidence`, `inspected_by`) are PROJECTIONS of
/// the edge's `verdict` fact, so an edge row cannot disagree with the fact that
/// justifies it — there is no column to write them into.
const EDGE_COLS: &str = "id,from_id,to_id,kind,truth_class,status,criterion,\
                         confidence,depends_on,inspected_by,created_at,updated_at";

fn row_to_node(r: &rusqlite::Row) -> rusqlite::Result<Node> {
    let body_str: String = r.get("body")?;
    Ok(Node {
        id: r.get("id")?,
        node_type: parse_named(r, "node_type")?,
        name: r.get("name")?,
        description: r.get("description")?,
        status: r.get("status")?,
        truth_class: parse_named(r, "truth_class")?,
        body: serde_json::from_str(&body_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn row_to_edge(r: &rusqlite::Row) -> rusqlite::Result<Edge> {
    let depends_str: String = r.get("depends_on")?;
    Ok(Edge {
        id: r.get("id")?,
        from_id: r.get("from_id")?,
        to_id: r.get("to_id")?,
        kind: parse_named(r, "kind")?,
        truth_class: parse_named(r, "truth_class")?,
        status: parse_named(r, "status")?,
        criterion: r.get("criterion")?,
        confidence: r.get("confidence")?,
        depends_on: serde_json::from_str(&depends_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?,
        inspected_by: r.get("inspected_by")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TmpRoot(PathBuf);

    impl TmpRoot {
        fn new(prefix: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sqlite_user_version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn graph_schema_migrations_are_valid() {
        schema_migrations().validate().unwrap();
    }

    #[test]
    fn fresh_init_sets_sqlite_user_version() {
        let tmp = TmpRoot::new("loom-store-fresh-migration");
        let store = Store::init(tmp.path(), Some("fresh"), false).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), SCHEMA_VERSION);
        assert_eq!(store.identity().unwrap().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn migration_removes_policy_ratification_and_evidence_but_keeps_journal() {
        let tmp = TmpRoot::new("loom-store-drop-ratify-policy");
        let store = Store::init(tmp.path(), Some("legacy-policy"), false).unwrap();
        let intent = store
            .add_node(
                NodeType::Intent,
                "delegated behavior",
                "a policy had approved this behavior",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        let journal = crate::journal::append(
            tmp.path(),
            "ratification",
            &intent.id,
            serde_json::json!({ "ratified_by": "policy:legacy" }),
        )
        .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO meta(key,value) VALUES ('ratify_policies','[]')",
                [],
            )
            .unwrap();
        let insert_fact = concat!(
            "INSERT INTO ",
            "fact(id,subject_kind,subject_id,claim,state,criterion,verification,confidence,asserted_by,asserted_at,stale) ",
            "VALUES ('policy-fact','node',?1,'ratification','ratified','policy approval','cited',1.0,'policy:legacy','2026-01-01T00:00:00Z','')"
        );
        store.conn.execute(insert_fact, [&intent.id]).unwrap();
        store
            .conn
            .execute(
                concat!(
                    "INSERT INTO ",
                    "evidence(id,fact_id,payload,kind,recorded_at,holds,expiry_reason) ",
                    "VALUES ('policy-evidence','policy-fact','{}','claim','2026-01-01T00:00:00Z',1,'')"
                ),
                [],
            )
            .unwrap();
        store
            .conn
            .pragma_update(None, "user_version", 4u32)
            .unwrap();
        drop(store);

        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(store.ratification(&intent.id).unwrap(), "unratified");
        assert_eq!(store.get_meta("ratify_policies").unwrap(), None);
        let evidence_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM evidence WHERE id='policy-evidence'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(evidence_count, 0, "fact evidence must cascade away");
        assert!(
            crate::journal::exists(tmp.path(), &journal.id).unwrap(),
            "migration must preserve append-only history"
        );
    }

    #[test]
    fn derived_ids_require_lowercase_hex() {
        assert!(is_derived_node_id("d0123456789abcdef"));
        assert!(!is_derived_node_id("d0123456789abcdeF"));
        assert!(!is_derived_node_id("d0123456789ABCDE"));
    }

    #[test]
    fn old_style_schema_is_adopted_without_rerunning_create_table() {
        let tmp = TmpRoot::new("loom-store-legacy-migration");
        let loom_dir = tmp.path().join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir).unwrap();
        let db_path = loom_dir.join(GRAPH_DB);
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO meta(key,value) VALUES
                 ('graph_id','legacy'),
                 ('name','legacy'),
                 ('schema_version',?1),
                 ('observed','0'),
                 ('created_at','legacy')",
                params![SCHEMA_VERSION.to_string()],
            )
            .unwrap();
            assert_eq!(sqlite_user_version(&conn), 0);
        }

        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), SCHEMA_VERSION);
        assert_eq!(store.identity().unwrap().name, "legacy");
    }

    #[test]
    fn malformed_legacy_schema_version_is_reported() {
        let tmp = TmpRoot::new("loom-store-invalid-legacy-version");
        let loom_dir = tmp.path().join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir).unwrap();
        let db_path = loom_dir.join(GRAPH_DB);
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO meta(key,value) VALUES ('schema_version','not-a-number')",
            [],
        )
        .unwrap();
        drop(conn);

        let error = Store::open(tmp.path())
            .err()
            .expect("malformed legacy schema version must fail open");
        assert!(
            error
                .to_string()
                .contains("invalid persisted schema_version"),
            "corrupt schema metadata must be surfaced: {error}"
        );
    }

    #[test]
    fn facet_and_tag_writes_reject_missing_typed_targets() {
        let tmp = TmpRoot::new("loom-store-annotation-targets");
        let store = Store::init(tmp.path(), Some("annotations"), false).unwrap();

        let facet_error = store
            .set_facet(
                "missing-node",
                TargetKind::Node,
                "level",
                "feature",
                TruthClass::Asserted,
            )
            .expect_err("a facet must not create an orphan node reference");
        assert!(facet_error.to_string().contains("no node target"));

        let tag_error = store
            .set_tag("missing-edge", TargetKind::Edge, "reviewed")
            .expect_err("a tag must not create an orphan edge reference");
        assert!(tag_error.to_string().contains("no edge target"));

        let intent = store
            .add_node(
                NodeType::Intent,
                "annotations keep typed targets",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .set_facet(
                &intent.id,
                TargetKind::Node,
                "level",
                "feature",
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_tag(&intent.id, TargetKind::Node, "integrity")
            .unwrap();
    }

    #[test]
    fn identity_defaults_only_missing_observed_and_rejects_malformed_values() {
        let tmp = TmpRoot::new("loom-store-observed-meta");
        let store = Store::init(tmp.path(), Some("observed-meta"), false).unwrap();

        store
            .conn
            .execute("DELETE FROM meta WHERE key='observed'", [])
            .unwrap();
        assert!(!store.identity().unwrap().observed);

        store
            .conn
            .execute(
                "INSERT INTO meta(key,value) VALUES ('observed','maybe')",
                [],
            )
            .unwrap();
        let error = store
            .identity()
            .expect_err("a malformed observed flag must not become owned mode");
        assert!(error.to_string().contains("meta.observed is malformed"));
    }

    #[test]
    fn grounding_roles_reject_non_implements_edges() {
        let tmp = TmpRoot::new("loom-store-grounding-role-kind");
        let store = Store::init(tmp.path(), Some("grounding-role-kind"), false).unwrap();
        let first = store
            .add_node(
                NodeType::Intent,
                "first behavior",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        let second = store
            .add_node(
                NodeType::Intent,
                "second behavior",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        let relates = store
            .add_edge(
                EdgeKind::Relates,
                &first.id,
                &second.id,
                TruthClass::Asserted,
            )
            .unwrap();

        let error = store
            .set_grounding_role(&relates.id, GroundingRole::Consumes)
            .expect_err("a relationship edge must not accept grounding semantics");
        assert!(error.to_string().contains("only to implements edges"));
        assert!(
            store
                .get_facet(&relates.id, TargetKind::Edge, "role")
                .unwrap()
                .is_none(),
            "rejected role write must leave no facet"
        );
    }

    #[test]
    fn repeated_node_status_preserves_updated_at() {
        let tmp = TmpRoot::new("loom-store-idempotent-node-status");
        let store = Store::init(tmp.path(), Some("idempotent-node-status"), false).unwrap();
        let validation = store
            .add_node(
                NodeType::Validation,
                "repeatable proof",
                "",
                "not_run",
                serde_json::json!({}),
            )
            .unwrap();

        store.set_node_status(&validation.id, "passed").unwrap();
        store
            .conn
            .execute(
                "UPDATE node SET updated_at='stable-sentinel' WHERE id=?1",
                params![validation.id],
            )
            .unwrap();
        store.set_node_status(&validation.id, "passed").unwrap();

        assert_eq!(
            store.get_node(&validation.id).unwrap().unwrap().updated_at,
            "stable-sentinel",
            "an unchanged status must not dirty the portable graph"
        );
    }

    #[test]
    fn delete_node_cascades_body_linked_notes_and_their_annotations() {
        let tmp = TmpRoot::new("loom-store-note-cascade");
        let store = Store::init(tmp.path(), Some("note-cascade"), false).unwrap();
        let inbox = store
            .add_node(
                NodeType::InboxItem,
                "journey fixture",
                "",
                "routed",
                serde_json::json!({}),
            )
            .unwrap();
        let decision = store
            .add_note(&inbox.id, "decision", "routed by journey")
            .unwrap();
        let nested = store
            .add_note(&decision.id, "context", "why it was routed")
            .unwrap();
        store
            .set_tag(&decision.id, TargetKind::Node, "fixture")
            .unwrap();

        store.delete_node(&inbox.id).unwrap();

        assert!(store.get_node(&decision.id).unwrap().is_none());
        assert!(store.get_node(&nested.id).unwrap().is_none());
        assert!(store
            .tags_of(&decision.id, TargetKind::Node)
            .unwrap()
            .is_empty());
    }
}
