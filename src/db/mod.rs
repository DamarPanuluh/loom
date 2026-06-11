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
             Run `loom init` to create one, or `cd` into a directory that has one\n\
             (or pin one explicitly: `--graph <path>` / `export LOOM_GRAPH=<path>`)."
        );
    }
    Ok(db_path(cwd))
}

// ---------------------------------------------------------------------------
// Graph targeting — which repo's graph does this command hit?
// ---------------------------------------------------------------------------

/// The `--graph` flag's value, stamped once at dispatch. OnceLock instead of
/// mutating the process env: no unsafe, no surprise inheritance by child
/// processes (a validation command spawned by `loom validate` should resolve
/// its OWN target, not silently inherit this process's flag).
static EXPLICIT_GRAPH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_explicit_graph(path: &str) {
    let _ = EXPLICIT_GRAPH.set(PathBuf::from(path));
}

/// Resolve the directory whose graph this command targets:
/// `--graph <path>` > `$LOOM_GRAPH` > current working directory.
///
/// The pin exists because cwd-implicit targeting has ONE sharp edge: a script
/// whose `cd` fails and falls back into a directory that happens to contain a
/// `.loom/` mutates that graph silently (it happened — it cost a graph's note
/// history). `LOOM_GRAPH`, set once per session, makes every loom call hit the
/// pinned graph no matter what `cd` does. Interactive driving keeps the
/// zero-ceremony cwd default.
pub fn resolve_root() -> Result<PathBuf> {
    if let Some(p) = EXPLICIT_GRAPH.get() {
        anyhow::ensure!(
            p.is_dir(),
            "--graph points at '{}', which is not a directory — point it at the repo root that contains `.loom/` (the directory, not a file).",
            p.display()
        );
        return Ok(p.clone());
    }
    if let Ok(p) = std::env::var("LOOM_GRAPH") {
        if !p.trim().is_empty() {
            let pb = PathBuf::from(&p);
            anyhow::ensure!(
                pb.is_dir(),
                "LOOM_GRAPH points at '{p}', which is not a directory — point it at the repo root that contains `.loom/` (the directory, not a file)."
            );
            return Ok(pb);
        }
    }
    Ok(std::env::current_dir()?)
}
