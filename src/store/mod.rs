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
    pub facets: Vec<Facet>,
    pub tags: Vec<Tag>,
    /// Portable repo config from the meta table — ONLY the allowlisted keys in
    /// [`PORTABLE_META_KEYS`]. Never a blind meta dump: identity keys travel as
    /// top-level export fields, and anything not allowlisted stays local.
    pub config: std::collections::BTreeMap<String, String>,
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
            let set = |k: &str, v: &str| -> Result<()> {
                conn.execute("INSERT INTO meta(key,value) VALUES (?1,?2)", params![k, v])?;
                Ok(())
            };
            set("graph_id", &gid)?;
            set("name", &default_name)?;
            set("schema_version", &SCHEMA_VERSION.to_string())?;
            set("observed", if observed { "1" } else { "0" })?;
            set("created_at", &now)?;
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
        Ok(Identity {
            graph_id: get("graph_id")?,
            name: get("name")?,
            schema_version: get("schema_version")?
                .parse()
                .context("meta.schema_version is malformed")?,
            observed: get("observed").unwrap_or_else(|_| "0".into()) == "1",
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
}

mod derived;
mod edges;
mod facets;
mod nodes;

// ---- helpers -----------------------------------------------------------
// ---- helpers -------------------------------------------------------------

fn schema_migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(SCHEMA)])
}

fn apply_schema_migrations(conn: &mut Connection) -> Result<()> {
    adopt_legacy_schema_version(conn)?;
    schema_migrations()
        .to_latest(conn)
        .context("migrating graph schema")?;
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

    if legacy_schema_version
        .as_deref()
        .and_then(|s| s.parse::<u32>().ok())
        == Some(SCHEMA_VERSION)
    {
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
        // Disambiguate from std's inherent `File::try_lock_shared`
        // (`TryLockError`); we use fs2's `FileExt` throughout (`io::Error`).
        let acquired = if exclusive {
            fs2::FileExt::try_lock_exclusive(&file)
        } else {
            fs2::FileExt::try_lock_shared(&file)
        };
        match acquired {
            Ok(()) => return Ok(file),
            Err(_) if attempt < 39 => {
                std::thread::sleep(wait);
                if wait < std::time::Duration::from_millis(50) {
                    wait *= 2;
                }
            }
            Err(_) => break,
        }
    }
    bail!(
        "graph is locked by another loom process (waiting for {} access)",
        if exclusive { "write" } else { "read" }
    )
}

/// Sentinel timestamp for derived rows. Derived data is recomputed by sync, so
/// its creation time is meaningless; a fixed sentinel keeps wipe+rebuild output
/// byte-identical (INV-2).
const DERIVED_TS: &str = "";

/// Deterministic, content-addressed id for derived data (FNV-1a 64-bit over the
/// joined parts). The same inputs always yield the same id, so a wiped-and-
/// rebuilt derived plane is byte-identical.
fn derived_id(parts: &[&str]) -> String {
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
    format!("d{h:016x}")
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

/// Column list for edge SELECTs.
const EDGE_COLS: &str = "id,from_id,to_id,kind,truth_class,status,criterion,evidence,\
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
        evidence: r.get("evidence")?,
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
}
