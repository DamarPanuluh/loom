use anyhow::Result;
use grafeo::{Config, GrafeoDB, QueryResult, Session};
use std::path::{Path, PathBuf};

pub mod queries;
pub mod schema;

// ---------------------------------------------------------------------------
// Trait — swappable DB backend
// ---------------------------------------------------------------------------

pub trait LoomDb {
    fn execute(&self, query: &str) -> Result<QueryResult>;
}

// ---------------------------------------------------------------------------
// Concrete implementation backed by Grafeo
// ---------------------------------------------------------------------------

pub struct GrafeoDb {
    // A single long-lived session, reused for every statement.
    //
    // Grafeo guarantees read-your-writes *within* a session: each
    // auto-committed mutation advances the epoch that the session's subsequent
    // reads observe. Creating a fresh session per statement (the previous
    // design) is NOT reliable — a newly created session does not always observe
    // a write another session just committed, which surfaced intermittently as
    // "Edge was just inserted but cannot be retrieved". loom is single-threaded
    // (one CLI command per process), so a single shared session is correct.
    //
    // Declared before `_inner` so the session is dropped first, before the
    // database handle runs its own teardown.
    session: Session,
    // Keep the database handle alive for the process lifetime.
    _inner: GrafeoDB,
}

impl GrafeoDb {
    /// Open or create a persistent on-disk database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let config = Config::persistent(path);
        let db = GrafeoDB::with_config(config).map_err(|e| {
            anyhow::anyhow!(
                "Could not open Grafeo database at {}.\n\
                 Ensure the path exists and is writable, or run `loom init` first.\n\
                 Cause: {}",
                path.display(),
                e
            )
        })?;
        let session = db.session();
        Ok(Self {
            session,
            _inner: db,
        })
    }

    /// In-memory database — used by the test suite.
    #[cfg(test)]
    pub fn in_memory() -> Self {
        let db = GrafeoDB::new_in_memory();
        let session = db.session();
        Self {
            session,
            _inner: db,
        }
    }
}

impl LoomDb for GrafeoDb {
    fn execute(&self, query: &str) -> Result<QueryResult> {
        self.session.execute(query).map_err(|e| {
            anyhow::anyhow!(
                "Query execution failed: {}\nQuery: {}",
                e,
                query.chars().take(200).collect::<String>()
            )
        })
    }
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

pub fn loom_dir(cwd: &Path) -> PathBuf {
    cwd.join(".loom")
}

pub fn db_path(cwd: &Path) -> PathBuf {
    loom_dir(cwd).join("graph.grafeo")
}

/// Verify that .loom/ exists in `cwd`. Returns the DB path or a helpful error.
pub fn ensure_initialized(cwd: &Path) -> Result<PathBuf> {
    let dir = loom_dir(cwd);
    if !dir.exists() {
        anyhow::bail!(
            "No loom graph found in this directory.\n\
             Run `loom init` to create one, or `cd` into a directory that has one."
        );
    }
    Ok(db_path(cwd))
}
