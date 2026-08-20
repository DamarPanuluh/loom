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
use crate::{
    Result, CRATE_VERSION, GRAPH_DB, JOURNEY_SCHEMA_CUT, LOOM_DIR, SCHEMA_VERSION,
    WRITER_SCHEMA_KEY, WRITER_VERSION_KEY,
};
use anyhow::{anyhow, bail, Context};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub use crate::identity::Agent;

/// Marker written only into cloned Journey fixture graphs. Live stores never
/// carry it; compiler-owned proof surgery is confined to these copies.
const LOCAL_SNAPSHOT_MARKER: &str = "local_snapshot";

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
    identity: std::cell::RefCell<crate::identity::ExecutionIdentity>,
    /// The advisory graph lock. A `RefCell` because the Store-owned guarded
    /// Journey settlement releases it around execution (compiled operations
    /// may spawn child `loom` processes) and re-takes it before any write,
    /// through `&Store` handles held by its callers.
    _lock: std::cell::RefCell<File>,
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
        let identity = crate::identity::ExecutionIdentity::resolve_env()?;
        Self::init_with_identity(root, name, observed, identity)
    }

    /// [`Store::init`] with a caller-pinned execution identity. Tests use this
    /// to stay hermetic: identity-sensitive writes (ratification, lane gates)
    /// must not change verdict when an ambient `LOOM_AGENT` leaks into the
    /// test process from a driver's shell.
    pub fn init_with_identity(
        root: &Path,
        name: Option<&str>,
        observed: bool,
        identity: crate::identity::ExecutionIdentity,
    ) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir)
            .with_context(|| format!("creating {}", loom_dir.display()))?;
        let db_path = loom_dir.join(GRAPH_DB);
        let lock = acquire_lock(&loom_dir, true, &identity)?;
        let fresh = !db_path.exists();
        let mut conn =
            Connection::open(&db_path).with_context(|| format!("opening {}", db_path.display()))?;
        ensure_supported_persisted_schema(&conn)?;
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
        let store = Store {
            conn,
            root: root.to_path_buf(),
            identity: std::cell::RefCell::new(identity),
            _lock: std::cell::RefCell::new(lock),
        };
        store.stamp_writer_identity();
        Ok(store)
    }

    /// Open an existing graph at `root/.loom/graph.sqlite`.
    pub fn open(root: &Path) -> Result<Store> {
        let identity = crate::identity::ExecutionIdentity::resolve_env()?;
        Self::open_with_identity(root, identity)
    }

    /// Open using an identity already resolved by the enclosing command. Long
    /// running commands use this when they must release and reacquire the graph
    /// lock around child execution without reinterpreting process identity.
    pub fn open_with_identity(
        root: &Path,
        identity: crate::identity::ExecutionIdentity,
    ) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        let db_path = loom_dir.join(GRAPH_DB);
        if !db_path.exists() {
            bail!(
                "no loom graph at {} — run `loom init` first",
                db_path.display()
            );
        }
        let lock = acquire_lock(&loom_dir, true, &identity)?;
        // Heartbeat for the advisory role lease: a lane driver touching the
        // graph is proof its session is alive. Best-effort by contract.
        crate::rolelease::refresh(root, &identity);
        let mut conn = Connection::open(&db_path)?;
        ensure_supported_persisted_schema(&conn)?;
        configure(&conn)?;
        apply_schema_migrations(&mut conn)?;
        let store = Store {
            conn,
            root: root.to_path_buf(),
            identity: std::cell::RefCell::new(identity),
            _lock: std::cell::RefCell::new(lock),
        };
        store.stamp_writer_identity();
        Ok(store)
    }

    /// Record which crate *and* schema last wrote this graph. Always overwrites
    /// the schema stamp: two builds can share a crate version and still drift.
    fn stamp_writer_identity(&self) {
        let _ = self.set_meta(WRITER_VERSION_KEY, CRATE_VERSION);
        let _ = self.set_meta(WRITER_SCHEMA_KEY, &SCHEMA_VERSION.to_string());
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
        let identity = crate::identity::ExecutionIdentity::resolve_env()?;
        let lock = acquire_lock(&loom_dir, false, &identity)?;
        // Heartbeat for the advisory role lease (see `open_with_identity`).
        crate::rolelease::refresh(root, &identity);
        let conn = Connection::open(&db_path)?;
        ensure_supported_persisted_schema(&conn)?;
        configure_read(&conn)?;
        // `user_version` is the migration stamp maintained by the write path
        // (`apply_schema_migrations`); a read open must not migrate, so a mismatch
        // is an explicit "run a write command first", never a silent read of an
        // older shape.
        let user_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if user_version < SCHEMA_VERSION {
            bail!(
                "graph schema is unstamped (v{user_version}); open it once with a write-capable \
                 loom v{SCHEMA_VERSION} command before using read-only commands"
            );
        }
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            identity: std::cell::RefCell::new(identity),
            _lock: std::cell::RefCell::new(lock),
        })
    }

    /// Clone this trusted local graph into an empty temporary root without
    /// applying import semantics. The interface deliberately hides SQLite/WAL
    /// consistency and journal provenance: callers get one confined graph clone
    /// that preserves the source graph's local authority while leaving every
    /// source byte untouched.
    pub fn clone_local_snapshot(&self, destination_root: &Path) -> Result<()> {
        fn copy_addressed_file(
            source_root: &Path,
            destination_root: &Path,
            relative: &Path,
        ) -> Result<()> {
            if relative.as_os_str().is_empty()
                || relative.is_absolute()
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                bail!(
                    "local graph snapshot refuses non-confined registered path '{}'",
                    relative.display()
                );
            }
            let source = source_root.join(relative);
            if !source.is_file() {
                return Ok(());
            }
            let destination = destination_root.join(relative);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&source, &destination).with_context(|| {
                format!(
                    "copying registered snapshot artifact {} to {}",
                    source.display(),
                    destination.display()
                )
            })?;
            Ok(())
        }

        if destination_root == self.root {
            bail!("local graph snapshot destination must differ from its source");
        }
        let destination_loom = destination_root.join(LOOM_DIR);
        if destination_loom.exists() {
            bail!(
                "local graph snapshot destination '{}' is not empty",
                destination_loom.display()
            );
        }
        std::fs::create_dir_all(&destination_loom)
            .with_context(|| format!("creating {}", destination_loom.display()))?;
        let destination_db = destination_loom.join(GRAPH_DB);
        let result = (|| -> Result<()> {
            // VACUUM INTO is SQLite's consistent online-copy operation. Opening
            // the source read-only and holding this Store's graph lock ensures
            // the clone includes committed WAL state without checkpointing or
            // otherwise changing the source database.
            let source_db = self.root.join(LOOM_DIR).join(GRAPH_DB);
            let source = Connection::open_with_flags(&source_db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| {
                    format!("opening local snapshot source {}", source_db.display())
                })?;
            let destination = destination_db.to_str().ok_or_else(|| {
                anyhow!(
                    "local graph snapshot destination '{}' is not valid UTF-8",
                    destination_db.display()
                )
            })?;
            source
                .execute("VACUUM INTO ?1", params![destination])
                .with_context(|| {
                    format!("cloning local graph into {}", destination_db.display())
                })?;

            // Journal provenance is local authority and therefore must not pass
            // through `restore_entries`, whose correct import behavior marks it
            // imported. Copy the immutable JSONL bytes verbatim instead.
            let source_journal = crate::journal::path(&self.root);
            if source_journal.is_file() {
                let destination_journal = crate::journal::path(destination_root);
                let parent = destination_journal.parent().ok_or_else(|| {
                    anyhow!(
                        "journal destination '{}' has no parent",
                        destination_journal.display()
                    )
                })?;
                std::fs::create_dir_all(parent)?;
                std::fs::copy(&source_journal, &destination_journal).with_context(|| {
                    format!(
                        "copying local journal {} to {}",
                        source_journal.display(),
                        destination_journal.display()
                    )
                })?;
            }

            // Graph predicates resolve these files relative to Store::root.
            // Copy only graph-addressed artifacts, not the whole repository, so
            // the clone preserves semantic state without broadening execution.
            let mut paths = std::collections::BTreeSet::new();
            for node in self.list_nodes(Some(NodeType::CodeFile), usize::MAX)? {
                paths.insert(PathBuf::from(node.name));
            }
            for node in self.list_nodes(Some(NodeType::Journey), usize::MAX)? {
                if let Some(artifact) = node
                    .body
                    .get("artifact")
                    .and_then(serde_json::Value::as_str)
                {
                    paths.insert(PathBuf::from(artifact));
                }
            }
            // Span evidence may cite files beyond the registered CodeFile set
            // (test files, journey surface manifests). `reverify_all` re-reads
            // every cited span from disk, so a clone missing one breaks the
            // anchor and stales its edge — an unchanged snapshot must sync
            // clean (INV-2). Cited files are graph-addressed artifacts too.
            let mut cited = self.conn.prepare(
                "SELECT DISTINCT json_extract(payload, '$.file') FROM evidence WHERE kind = 'span'",
            )?;
            let cited_files = cited.query_map([], |row| row.get::<_, Option<String>>(0))?;
            for file in cited_files {
                if let Some(file) = file? {
                    paths.insert(PathBuf::from(file));
                }
            }
            paths.insert(PathBuf::from("loom.graph.json"));
            for relative in paths {
                copy_addressed_file(&self.root, destination_root, &relative)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&destination_loom) {
                return Err(error).context(format!(
                    "removing incomplete local snapshot {} also failed: {cleanup_error}",
                    destination_loom.display()
                ));
            }
            return Err(error);
        }
        if let Err(error) = std::fs::write(
            destination_loom.join(LOCAL_SNAPSHOT_MARKER),
            b"{\"schema\":\"loom.local-snapshot/v1\"}\n",
        ) {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&destination_loom) {
                return Err(error).context(format!(
                    "marking the cloned graph as a local snapshot failed, and removing {} also failed: {cleanup_error}",
                    destination_loom.display()
                ));
            }
            return Err(error).context("marking the cloned graph as a local snapshot");
        }
        Ok(())
    }

    /// True only for graphs minted by [`Store::clone_local_snapshot`].
    /// Live operator graphs never carry this marker.
    pub fn is_local_snapshot(&self) -> bool {
        let path = self.root.join(LOOM_DIR).join(LOCAL_SNAPSHOT_MARKER);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata.is_file() && !metadata.file_type().is_symlink(),
            Err(_) => false,
        }
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
        self.identity.borrow().authority()
    }

    /// Release the advisory graph write lock while keeping the connection
    /// open. Only the Store-owned guarded Journey settlement uses this: a
    /// compiled Journey's operations may spawn child `loom` processes that
    /// open the graph, and a held exclusive lock would refuse them. The
    /// harness lock serializes proof execution; settlement re-derives every
    /// trust-relevant input and refuses on drift. Re-take with
    /// [`Store::reacquire_graph_lock`] before any write.
    pub fn release_graph_lock(&self) {
        let lock_path = self.root.join(LOOM_DIR).join("lock");
        if let Ok(placeholder) = File::open(&lock_path) {
            *self._lock.borrow_mut() = placeholder;
        }
    }

    /// Re-take the advisory graph write lock under the identity this Store
    /// opened with. Fails closed (lock contention) rather than writing
    /// without the boundary.
    pub fn reacquire_graph_lock(&self) -> Result<()> {
        let loom_dir = self.root.join(LOOM_DIR);
        let identity = self.identity.borrow().clone();
        *self._lock.borrow_mut() = acquire_lock(&loom_dir, true, &identity)?;
        Ok(())
    }

    /// Runtime provenance at the store seam. Authorization is derived from
    /// the already-validated Agent; profile remains independent attribution.
    pub fn execution_identity(&self) -> crate::identity::ExecutionIdentity {
        self.identity.borrow().clone()
    }

    /// Append provenance using the identity resolved when this Store opened.
    /// Callers never read process configuration or assemble audit identity.
    pub fn append_journal(
        &self,
        event: &str,
        target_id: &str,
        payload: serde_json::Value,
    ) -> Result<crate::journal::Entry> {
        crate::journal::append(
            self.root(),
            &self.execution_identity(),
            event,
            target_id,
            payload,
        )
    }

    pub fn append_journal_once(
        &self,
        event: &str,
        target_id: &str,
        payload: serde_json::Value,
        same_transition: impl Fn(&crate::journal::Entry) -> bool,
    ) -> Result<Option<crate::journal::Entry>> {
        crate::journal::append_once(
            self.root(),
            &self.execution_identity(),
            event,
            target_id,
            payload,
            same_transition,
        )
    }

    /// Override the acting agent (CLI sets this from `LOOM_AGENT`; tests set it
    /// explicitly to exercise lane gates without env races).
    pub fn set_agent(&self, agent: Agent) {
        let identity = self.identity.borrow().with_authority(agent);
        *self.identity.borrow_mut() = identity;
    }

    /// Lane gate: a declared lane may only write edges/verdicts it owns. Solo
    /// drives every lane. `sync` is implicit (derived paths never call this).
    ///
    /// Grounding (`implements`) writes are builder-owned, but the fixer
    /// contract explicitly allows re-grounding after a repair moved code.
    /// Verdicts on those edges stay on `check_lane` so fixer still cannot
    /// record the passing claim.
    fn check_grounding_write(&self) -> Result<()> {
        match self.agent() {
            Agent::Solo => Ok(()),
            Agent::Lane(role)
                if role.satisfies(registry::OwnerRole::Builder)
                    || role == registry::OwnerRole::Fixer =>
            {
                Ok(())
            }
            Agent::Lane(role) => bail!(
                "lane gate: agent '{}' may not write 'builder'-owned facts",
                role.as_str()
            ),
        }
    }

    /// Lane gate: a declared lane may only write edges/verdicts it owns. Solo
    /// drives every lane. `sync` is implicit (derived paths never call this).
    fn check_lane(&self, owner: registry::OwnerRole) -> Result<()> {
        match self.agent() {
            Agent::Solo => Ok(()),
            Agent::Lane(role) if role.satisfies(owner) => Ok(()),
            Agent::Lane(role) => bail!(
                "lane gate: agent '{}' may not write '{}'-owned facts",
                role.as_str(),
                owner.as_str()
            ),
        }
    }

    /// Preflight authorization for a command that will create an asserted
    /// edge after performing other writes. The edge write still enforces the
    /// same gate itself; this check lets a multi-step command fail before its
    /// first mutation, while the surrounding transaction remains the final
    /// atomicity boundary for every later error.
    pub fn require_edge_kind_owner(&self, kind: EdgeKind) -> Result<()> {
        if kind == EdgeKind::Implements {
            self.check_grounding_write()
        } else {
            self.check_lane(registry::spec(kind).owner)
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

mod adjudications;
mod derived;
#[allow(unused_imports)] // consumed by diagnostics_cmd via crate::store::
pub(crate) use derived::{DebtPromotionInput, DebtPromotionResult};

mod edges;
mod facets;
/// The write boundary: every asserted fact enters through `assert_fact`.
pub mod facts;
mod judgments;
mod nodes;
pub use adjudications::HitAdjudication;
pub use edges::Dependent;
pub use facts::{edge_verdict, Assertion, FactView, Subject};
pub use judgments::{JudgmentKind, JudgmentProposal, JudgmentState};

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

const MIGRATION_7_BATCH_AUTH: &str = r#"
ALTER TABLE fact ADD COLUMN decision_mode TEXT NOT NULL DEFAULT 'individual';
ALTER TABLE fact ADD COLUMN batch_id TEXT NOT NULL DEFAULT '';
"#;

/// Migration 8 — hit-level adjudication. A suppression's key is the content
/// hash of the matched text, so it answers the same text wherever it moves and
/// expires by construction when the text changes.
const MIGRATION_8_HIT_ADJUDICATION: &str = r#"
CREATE TABLE hit_adjudication (
    rule_name    TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    excerpt      TEXT NOT NULL,
    reason       TEXT NOT NULL,
    actor        TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    PRIMARY KEY (rule_name, content_hash)
);
"#;

/// Migration 9 — the judgment inbox. Human-only judgments (ratify, reject)
/// and redefinitions get a staged proposal object: an LLM discovers the
/// candidate and files it with evidence; the human reviews a digest and
/// confirms each through the SAME typed challenge the direct command would
/// demand. Authority is unchanged — the inbox holds recommendations, never
/// decisions.
const MIGRATION_9_JUDGMENT_INBOX: &str = r#"
CREATE TABLE judgment_proposal (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('ratify','reject','redefine')),
    intent_id  TEXT NOT NULL,
    evidence   TEXT NOT NULL,
    detail     TEXT NOT NULL DEFAULT '',
    staged_by  TEXT NOT NULL,
    staged_at  TEXT NOT NULL,
    state      TEXT NOT NULL DEFAULT 'staged' CHECK (state IN ('staged','confirmed','withdrawn')),
    decided_at TEXT NOT NULL DEFAULT ''
);
CREATE INDEX idx_judgment_state ON judgment_proposal(state, kind);
"#;

/// Preserve the executor profile independently from write authority. Existing
/// facts predate profile capture and correctly remain empty rather than being
/// backfilled with an invented worker identity.
const MIGRATION_10_EXECUTOR_PROFILE: &str = r#"
ALTER TABLE fact ADD COLUMN asserted_profile TEXT NOT NULL DEFAULT '';
"#;

/// Normalize absent executor attribution to SQL NULL. V10 used an empty-string
/// sentinel; preserve real values while removing that representational split.
const MIGRATION_11_NULLABLE_EXECUTOR_PROFILE: &str = r#"
ALTER TABLE fact ADD COLUMN asserted_profile_nullable TEXT;
UPDATE "fact" SET asserted_profile_nullable = NULLIF(asserted_profile, '');
ALTER TABLE fact DROP COLUMN asserted_profile;
ALTER TABLE fact RENAME COLUMN asserted_profile_nullable TO asserted_profile;
"#;

/// Migration 13 — durable adversarial challenge facts and fact-snapshot
/// evidence. SQLite cannot extend a CHECK constraint in place, so both tables
/// are rebuilt while preserving every existing row and the edge projection.
const MIGRATION_13_ADVERSARIAL_REVIEW: &str = r#"
DROP VIEW edge_view;

CREATE TABLE fact_v13 (
    id               TEXT PRIMARY KEY,
    subject_kind     TEXT NOT NULL CHECK (subject_kind IN ('node','edge')),
    subject_id       TEXT NOT NULL,
    claim            TEXT NOT NULL CHECK (claim IN ('verdict','observation','adjudication','ratification','challenge')),
    state            TEXT NOT NULL,
    criterion        TEXT NOT NULL DEFAULT '',
    verification     TEXT NOT NULL CHECK (verification IN ('verified','cited','claimed','expired')),
    confidence       REAL NOT NULL DEFAULT 0,
    asserted_by      TEXT NOT NULL DEFAULT '',
    asserted_at      TEXT NOT NULL,
    stale            TEXT NOT NULL DEFAULT '',
    decision_mode    TEXT NOT NULL DEFAULT 'individual',
    batch_id         TEXT NOT NULL DEFAULT '',
    asserted_profile TEXT,
    UNIQUE (subject_kind, subject_id, claim)
);

CREATE TABLE evidence_v13 (
    id            TEXT PRIMARY KEY,
    fact_id       TEXT NOT NULL REFERENCES fact_v13(id) ON DELETE CASCADE,
    payload       TEXT NOT NULL DEFAULT '{}',
    kind          TEXT NOT NULL CHECK (kind IN ('run','span','journal','claim','fact_snapshot')),
    recorded_at   TEXT NOT NULL,
    holds         INTEGER NOT NULL DEFAULT 1,
    expiry_reason TEXT NOT NULL DEFAULT ''
);

INSERT INTO "fact_v13"
SELECT id,subject_kind,subject_id,claim,state,criterion,verification,confidence,
       asserted_by,asserted_at,stale,decision_mode,batch_id,asserted_profile
  FROM fact;
INSERT INTO "evidence_v13"
SELECT id,fact_id,payload,kind,recorded_at,holds,expiry_reason FROM evidence;

DROP TABLE evidence;
DROP TABLE fact;
ALTER TABLE fact_v13 RENAME TO fact;
ALTER TABLE evidence_v13 RENAME TO evidence;

CREATE INDEX idx_fact_subject      ON fact(subject_kind, subject_id);
CREATE INDEX idx_fact_verification ON fact(verification, claim);
CREATE INDEX idx_fact_claim_state  ON fact(claim, state);
CREATE INDEX idx_evidence_fact     ON evidence(fact_id, kind);
CREATE INDEX idx_evidence_holds    ON evidence(holds);

CREATE VIEW edge_view AS
SELECT e.id, e.from_id, e.to_id, e.kind, e.truth_class, e.status,
       e.depends_on, e.created_at, e.updated_at,
       COALESCE(f.criterion, '')   AS criterion,
       COALESCE(f.confidence, 0.0) AS confidence,
       COALESCE(f.asserted_by, '') AS inspected_by
FROM edge e
LEFT JOIN fact f
  ON f.subject_kind = 'edge' AND f.subject_id = e.id AND f.claim = 'verdict';
"#;

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
        M::up(MIGRATION_7_BATCH_AUTH),
        M::up(MIGRATION_8_HIT_ADJUDICATION),
        M::up(MIGRATION_9_JUDGMENT_INBOX),
        M::up(MIGRATION_10_EXECUTOR_PROFILE),
        M::up(MIGRATION_11_NULLABLE_EXECUTOR_PROFILE),
        // V12 is a semantic hard cut: the SQLite shape is unchanged, but old
        // graph vocabularies cannot be interpreted under the journey-root model.
        M::up("SELECT 1;"),
        M::up(MIGRATION_13_ADVERSARIAL_REVIEW),
    ])
}

fn persisted_meta_schema_version(conn: &Connection) -> Result<Option<u32>> {
    let has_meta = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !has_meta {
        return Ok(None);
    }
    conn.query_row(
        "SELECT value FROM meta WHERE key='schema_version'",
        [],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|raw| {
        raw.parse::<u32>()
            .with_context(|| format!("invalid persisted schema_version '{raw}'"))
    })
    .transpose()
}

/// Refuse incompatible persisted graphs before configuration or migration can
/// mutate their database. Version zero with no meta stamp is a fresh database;
/// version zero with a v12 meta stamp is the old-style current stamp adopted
/// below. Every genuine v1-v11 graph must be rebuilt under the journey-root
/// vocabulary rather than translated into a graph that claims new semantics.
///
/// Behind-schema graphs at or above [`JOURNEY_SCHEMA_CUT`] are allowed through
/// so [`apply_schema_migrations`] can raise them — possibly with consent.
fn meta_opt(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
        row.get::<_, String>(0)
    })
    .optional()
    .ok()
    .flatten()
}

fn parse_crate_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Auto-migrate only when this binary is a newer *crate* than the last writer.
/// Same crate version + higher schema is an unreleased-branch leak. Missing or
/// unparsable writer stamps also require consent: we cannot prove this is a
/// release upgrade. Fresh DBs (`persisted_schema == 0`) and already-current
/// graphs do not migrate.
pub(crate) fn schema_migration_requires_consent(
    persisted_schema: u32,
    binary_schema: u32,
    writer_crate: Option<&str>,
    binary_crate: &str,
) -> bool {
    if persisted_schema == 0 || persisted_schema >= binary_schema {
        return false;
    }
    match (
        parse_crate_version(writer_crate.unwrap_or("")),
        parse_crate_version(binary_crate),
    ) {
        (Some(writer), Some(binary)) if binary > writer => false,
        _ => true,
    }
}

pub(crate) fn ahead_schema_error(
    graph_schema: u32,
    binary_schema: u32,
    binary_crate: &str,
    writer_crate: Option<&str>,
    writer_schema: Option<&str>,
) -> String {
    let writer_bits = match (writer_crate, writer_schema) {
        (Some(c), Some(s)) => format!("Last writer: loom {c} (schema v{s})."),
        (Some(c), None) => format!("Last writer: loom {c} (writer schema unstamped)."),
        (None, Some(s)) => format!("Last writer crate unstamped (schema v{s})."),
        (None, None) => "Last writer: unknown.".to_string(),
    };
    let same_crate = writer_crate == Some(binary_crate);
    if same_crate {
        format!(
            "this graph is v{graph_schema}; this loom understands v{binary_schema}. \
             {writer_bits} This binary is also loom {binary_crate} — a same-version \
             schema fork, not a newer release. There is no downgrade. \
             Reinstalling {binary_crate} will not help. Restore a pre-migration \
             backup, or use the build that understands v{graph_schema}. \
             The graph is untouched."
        )
    } else {
        format!(
            "this graph is v{graph_schema}; this loom understands v{binary_schema}. \
             {writer_bits} It was written by a newer loom — upgrade this binary \
             (see README) to a build that understands v{graph_schema}. \
             There is no downgrade. The graph is untouched."
        )
    }
}

fn ensure_supported_persisted_schema(conn: &Connection) -> Result<()> {
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let meta_version = persisted_meta_schema_version(conn)?;
    for version in [(user_version != 0).then_some(user_version), meta_version]
        .into_iter()
        .flatten()
    {
        if version < JOURNEY_SCHEMA_CUT {
            bail!(
                "this graph is v{version}; loom v12 introduced the journey paradigm — re-init \
                 and rebuild (loom bootstrap suggest, author journeys, loom journey derive). \
                 The graph is untouched."
            );
        }
        if version > SCHEMA_VERSION {
            let writer_crate = meta_opt(conn, WRITER_VERSION_KEY);
            let writer_schema = meta_opt(conn, WRITER_SCHEMA_KEY);
            bail!(
                "{}",
                ahead_schema_error(
                    version,
                    SCHEMA_VERSION,
                    CRATE_VERSION,
                    writer_crate.as_deref(),
                    writer_schema.as_deref(),
                )
            );
        }
    }
    Ok(())
}

fn apply_schema_migrations(conn: &mut Connection) -> Result<()> {
    ensure_supported_persisted_schema(conn)?;
    adopt_legacy_schema_version(conn)?;
    let from_version: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let writer_crate = meta_opt(conn, WRITER_VERSION_KEY);
    let requires_consent = schema_migration_requires_consent(
        from_version,
        SCHEMA_VERSION,
        writer_crate.as_deref(),
        CRATE_VERSION,
    );
    let consented = std::env::var("LOOM_SCHEMA_MIGRATE").ok().as_deref() == Some("1");
    if from_version < SCHEMA_VERSION && from_version != 0 && requires_consent && !consented {
        bail!(
            "refusing to migrate graph schema v{from_version} → v{SCHEMA_VERSION}: this binary \
             is still loom {CRATE_VERSION} (same crate as the last writer, or writer unknown). \
             An unreleased branch with a higher schema would rewrite the graph in place. \
             Set LOOM_SCHEMA_MIGRATE=1 to consent, or use a release that bumped the crate \
             version. The graph is untouched."
        );
    }
    if from_version < SCHEMA_VERSION && from_version != 0 {
        let writer = writer_crate.as_deref().unwrap_or("unknown");
        if consented && requires_consent {
            eprintln!(
                "loom: migrating graph schema v{from_version} → v{SCHEMA_VERSION} \
                 (consented via LOOM_SCHEMA_MIGRATE=1; last writer {writer})"
            );
        } else {
            eprintln!(
                "loom: migrating graph schema v{from_version} → v{SCHEMA_VERSION} \
                 (last writer loom {writer} → {CRATE_VERSION})"
            );
        }
    }
    schema_migrations()
        .to_latest(conn)
        .context("migrating graph schema")?;
    let foreign_key_issue: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT \"table\", rowid, parent FROM pragma_foreign_key_check LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((table, rowid, parent)) = foreign_key_issue {
        bail!(
            "graph migration left a foreign-key violation: table={table} rowid={rowid} parent={parent}"
        );
    }
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

    if let Some(legacy) = legacy_schema_version {
        if (JOURNEY_SCHEMA_CUT..=SCHEMA_VERSION).contains(&legacy) {
            conn.pragma_update(None, "user_version", legacy)?;
        }
    }
    Ok(())
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", SQLITE_BUSY_TIMEOUT_MS as i64)?;
    Ok(())
}

/// Connection setup for a read-only open. Sets the busy timeout and enforces
/// `query_only`, so a mis-routed read command fails loudly instead of writing.
/// Deliberately does NOT set `journal_mode` (a write) or run migrations — a read
/// open never mutates the file.
fn configure_read(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "busy_timeout", SQLITE_BUSY_TIMEOUT_MS as i64)?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(())
}

/// Wall-clock budget for acquiring an exclusive graph lock before failing with
/// a named contention error. Writers stay fail-fast so competing mutations do
/// not silently queue behind one another. Registered in `loom limits`.
pub(crate) const LOCK_WAIT_BUDGET_MS: u64 = 2_000;

/// Read-only diagnostics are commonly issued immediately after a graph write.
/// Give those commands a longer, still-bounded grace period so a routine
/// `status`/`next` observation does not turn a healthy in-flight write into an
/// EX_TEMPFAIL. Registered in `loom limits`.
pub(crate) const READ_LOCK_WAIT_BUDGET_MS: u64 = 10_000;

/// Statement-level SQLite busy timeout, so brief lock overlap retries inside
/// the store instead of surfacing SQLITE_BUSY.
pub(crate) const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;

fn acquire_lock(
    loom_dir: &Path,
    exclusive: bool,
    identity: &crate::identity::ExecutionIdentity,
) -> Result<File> {
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
    let (limit_name, budget_ms) = if exclusive {
        ("lock_wait_ms", LOCK_WAIT_BUDGET_MS)
    } else {
        ("read_lock_wait_ms", READ_LOCK_WAIT_BUDGET_MS)
    };
    let budget = std::time::Duration::from_millis(budget_ms);
    let deadline = std::time::Instant::now() + budget;
    let mut wait = std::time::Duration::from_millis(5);
    loop {
        let acquired = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match acquired {
            Ok(()) => {
                record_lock_holder(&file, exclusive, identity);
                return Ok(file);
            }
            // A held lock may release any moment — retry with backoff, but
            // never past the budget: a hang is never an acceptable failure mode.
            Err(std::fs::TryLockError::WouldBlock) => {
                let now = std::time::Instant::now();
                if now + wait >= deadline {
                    break;
                }
                std::thread::sleep(wait);
                if wait < std::time::Duration::from_millis(50) {
                    wait *= 2;
                }
            }
            // A real I/O error will not heal by waiting.
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("locking {}", lock_path.display()));
            }
        }
    }
    bail!(
        "{LOCK_CONTENTION_MARKER}: graph lock exceeded {limit_name}={budget_ms} — {} \
         (waiting for {} access); retry after it exits",
        describe_lock_holder(&lock_path),
        if exclusive { "write" } else { "read" }
    )
}

/// Stamp the holder's identity into the lock file so a contender refuses
/// against an identity, not a mystery — the graph-lock counterpart of the
/// harness lock's holder record. Best-effort: the flock itself is the
/// enforcement, and it releases with the holder's process even if this
/// write fails.
fn record_lock_holder(
    file: &File,
    exclusive: bool,
    execution: &crate::identity::ExecutionIdentity,
) {
    use std::io::Write;
    let identity = serde_json::json!({
        "pid": std::process::id(),
        "agent": execution.actor(),
        "profile": execution.profile(),
        "mode": if exclusive { "write" } else { "read" },
        "command": std::env::args().collect::<Vec<_>>().join(" "),
        "since": crate::journal::millis_to_iso(
            crate::journal::now_iso().parse::<i64>().unwrap_or(0),
        ),
    });
    let mut f = file;
    let _ = f.set_len(0);
    let _ = f.write_all(identity.to_string().as_bytes());
}

/// Render the recorded holder for the contention error. The record lags the
/// lock by microseconds (acquire → write) and outlives it (release does not
/// truncate), so name it as the RECORDED holder: with shared read locks it
/// is also just the most recent of possibly several concurrent readers.
fn describe_lock_holder(lock_path: &Path) -> String {
    let parsed = std::fs::read_to_string(lock_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    match parsed {
        Some(h) => {
            let profile = h
                .get("profile")
                .and_then(|v| v.as_str())
                .map(|p| format!(" / profile {p}"))
                .unwrap_or_default();
            format!(
                "recorded holder is agent {}{} pid {} ({} access, since {})\n  command: {}",
                h.get("agent").and_then(|v| v.as_str()).unwrap_or("?"),
                profile,
                h.get("pid").and_then(|v| v.as_u64()).unwrap_or(0),
                h.get("mode").and_then(|v| v.as_str()).unwrap_or("?"),
                h.get("since").and_then(|v| v.as_str()).unwrap_or("?"),
                h.get("command").and_then(|v| v.as_str()).unwrap_or("?"),
            )
        }
        None => "held by another loom process (identity unread)".to_string(),
    }
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
        let asserted_profile_not_null: i64 = store
            .conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('fact') WHERE name = 'asserted_profile'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            asserted_profile_not_null, 0,
            "absent executor attribution must be represented as SQL NULL"
        );
    }

    #[test]
    fn v12_store_migrates_in_place_to_v13_without_losing_fact_evidence() {
        let tmp = TmpRoot::new("loom-store-v12-to-v13");
        let store = Store::init(tmp.path(), Some("v12-upgrade"), false).unwrap();
        let finding = store
            .add_node(
                NodeType::Finding,
                "preserve this observation",
                "migration must keep it",
                "code_audit",
                serde_json::json!({
                    "kind": "code_audit",
                    "source": "code_audit",
                    "evidence": "migration fixture",
                    "impact": "loss would corrupt history",
                    "confidence": 0.8,
                    "link": "migration-fixture"
                }),
            )
            .unwrap();
        let fact = store
            .assert_fact(
                crate::store::Assertion::new(
                    crate::store::Subject::Node(finding.id.clone()),
                    Claim::Observation,
                    "observed",
                    "solo",
                )
                .confidence(0.8)
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    "migration fixture".into(),
                )]),
            )
            .unwrap();
        let evidence_id = fact.evidence[0].id.clone();
        store
            .conn
            .pragma_update(None, "user_version", 12u32)
            .unwrap();
        store
            .conn
            .execute("UPDATE meta SET value='12' WHERE key='schema_version'", [])
            .unwrap();
        // Simulate a real release upgrade (crate 0.34.0 → 0.34.1), not a
        // same-crate feature-branch leak. Same-crate 12→13 is refused below.
        store.set_meta(WRITER_VERSION_KEY, "0.34.0").unwrap();
        store.set_meta(WRITER_SCHEMA_KEY, "12").unwrap();
        drop(store);

        let read_error = match Store::open_read(tmp.path()) {
            Ok(_) => panic!("read-only v12 open must require migration"),
            Err(error) => error.to_string(),
        };
        assert!(read_error.contains("write-capable"), "{read_error}");

        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), 13);
        assert_eq!(store.identity().unwrap().schema_version, 13);
        assert!(store.fact_by_id(&fact.fact.id).unwrap().is_some());
        let evidence_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM evidence WHERE id=?1",
                [&evidence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(evidence_count, 1);
        let fact_sql: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='fact'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let evidence_sql: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='evidence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(fact_sql.contains("'challenge'"));
        assert!(evidence_sql.contains("'fact_snapshot'"));
        let fk_issues: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(fk_issues, 0);
    }

    #[test]
    fn v12_store_refuses_same_crate_migrate_without_consent() {
        let tmp = TmpRoot::new("loom-store-v12-same-crate-consent");
        let store = Store::init(tmp.path(), Some("v12-consent"), false).unwrap();
        store
            .conn
            .pragma_update(None, "user_version", 12u32)
            .unwrap();
        store
            .conn
            .execute("UPDATE meta SET value='12' WHERE key='schema_version'", [])
            .unwrap();
        drop(store);

        let error = match Store::open(tmp.path()) {
            Ok(_) => panic!("same-crate 12→13 must not migrate in place"),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("LOOM_SCHEMA_MIGRATE"),
            "refusal must name the consent flag: {error}"
        );
        let conn = Connection::open(tmp.path().join(LOOM_DIR).join(GRAPH_DB)).unwrap();
        assert_eq!(
            sqlite_user_version(&conn),
            12,
            "refused migration must leave the graph untouched"
        );
    }

    #[test]
    fn pre_v12_graph_is_refused_without_changing_its_stamps() {
        let tmp = TmpRoot::new("loom-store-v11-hard-cut");
        {
            let store = Store::init(tmp.path(), Some("old"), false).unwrap();
            drop(store);
        }
        let db = tmp.path().join(crate::LOOM_DIR).join(crate::GRAPH_DB);
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 11u32).unwrap();
        conn.execute("UPDATE meta SET value='11' WHERE key='schema_version'", [])
            .unwrap();
        drop(conn);

        for error in [
            Store::open_read(tmp.path())
                .err()
                .expect("read open refuses"),
            Store::open(tmp.path()).err().expect("write open refuses"),
            Store::init(tmp.path(), None, false)
                .err()
                .expect("idempotent init refuses"),
        ] {
            let message = error.to_string();
            assert!(message.contains("journey paradigm"), "{message}");
            assert!(message.contains("re-init and rebuild"), "{message}");
        }

        let conn = Connection::open(&db).unwrap();
        assert_eq!(sqlite_user_version(&conn), 11);
        let meta: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(meta, "11");
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
        let journal = store
            .append_journal(
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
        // Rewind to a faithful v4 graph: the version stamp AND the columns
        // later migrations added, or migration 7 re-adds what is already
        // there and the simulation fails on its own bookkeeping.
        store
            .conn
            .execute_batch(
                "ALTER TABLE fact DROP COLUMN asserted_profile;
                 ALTER TABLE fact DROP COLUMN decision_mode;
                 ALTER TABLE fact DROP COLUMN batch_id;
                 DROP TABLE hit_adjudication;
                 DROP TABLE judgment_proposal;",
            )
            .unwrap();
        store
            .conn
            .pragma_update(None, "user_version", 4u32)
            .unwrap();
        drop(store);

        // Public Store opens must refuse v4 under the v12 hard cut. Exercise
        // the preserved historical migration directly: those steps exist to
        // build fresh databases and remain independently valid, not to upgrade
        // persisted pre-journey graphs.
        let db = tmp.path().join(LOOM_DIR).join(GRAPH_DB);
        let mut conn = Connection::open(db).unwrap();
        schema_migrations().to_latest(&mut conn).unwrap();
        drop(conn);

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
        let store = Store::init_with_identity(
            tmp.path(),
            Some("grounding-role-kind"),
            false,
            crate::identity::ExecutionIdentity::solo(),
        )
        .unwrap();
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

        // loom-stability-exempt: in-module test of set_node_status itself
        store.set_node_status(&validation.id, "passed").unwrap();
        store
            .conn
            .execute(
                "UPDATE node SET updated_at='stable-sentinel' WHERE id=?1",
                params![validation.id],
            )
            .unwrap();
        // loom-stability-exempt: in-module test of set_node_status itself
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

    #[test]
    fn finding_hash_treats_a_vanished_codefile_as_absence() {
        let tmp = TmpRoot::new("loom-store-vanished-finding-file");
        let store = Store::init(tmp.path(), Some("vanished-file"), false).unwrap();
        let finding = store
            .add_node(
                NodeType::Finding,
                "behavior that outlived its file",
                "impact",
                "code_audit",
                serde_json::json!({
                    "file": "src/gone.rs",
                    "evidence": "src/gone.rs:1",
                    "kind": "code_audit",
                    "source": "llm",
                    "confidence": 0.7,
                }),
            )
            .unwrap();
        assert_eq!(
            store.finding_codefile_hash(&finding.id).unwrap(),
            None,
            "a finding that names an unregistered file is orphaned evidence, not a graph crash"
        );
        assert!(store.finding_owner_intents(&finding.id).unwrap().is_empty());
    }

    #[test]
    fn same_crate_schema_bump_requires_consent() {
        assert!(
            schema_migration_requires_consent(12, 13, Some("0.34.1"), "0.34.1"),
            "unreleased branch at the same crate version must not silently migrate"
        );
        assert!(
            schema_migration_requires_consent(12, 13, None, "0.34.1"),
            "missing writer stamp cannot be assumed to be a release upgrade"
        );
        assert!(
            !schema_migration_requires_consent(12, 13, Some("0.34.1"), "0.35.0"),
            "a crate bump is a real release upgrade"
        );
        assert!(
            !schema_migration_requires_consent(0, 13, None, "0.34.1"),
            "fresh databases must still run to_latest"
        );
        assert!(
            !schema_migration_requires_consent(12, 12, Some("0.34.1"), "0.34.1"),
            "already-current graphs do not migrate"
        );
    }

    #[test]
    fn ahead_schema_error_names_the_fork_when_crate_matches() {
        let fork = ahead_schema_error(13, 12, "0.34.1", Some("0.34.1"), Some("13"));
        assert!(fork.contains("same-version"), "{fork}");
        assert!(fork.contains("will not help"), "{fork}");
        assert!(fork.contains("no downgrade"), "{fork}");
        assert!(
            !fork.contains("upgrade this binary"),
            "same-crate fork must not instruct an impossible upgrade: {fork}"
        );

        let upgrade = ahead_schema_error(13, 12, "0.34.1", Some("0.35.0"), Some("13"));
        assert!(
            upgrade.contains("newer loom") && upgrade.contains("upgrade"),
            "{upgrade}"
        );
        assert!(upgrade.contains("no downgrade"), "{upgrade}");
    }

    #[test]
    fn write_open_stamps_writer_crate_and_schema() {
        let tmp = TmpRoot::new("loom-store-writer-stamp");
        let store = Store::init(tmp.path(), Some("stamp"), false).unwrap();
        let expected_schema = SCHEMA_VERSION.to_string();
        assert_eq!(
            store.get_meta(WRITER_VERSION_KEY).unwrap().as_deref(),
            Some(CRATE_VERSION)
        );
        assert_eq!(
            store.get_meta(WRITER_SCHEMA_KEY).unwrap().as_deref(),
            Some(expected_schema.as_str())
        );
    }
}
