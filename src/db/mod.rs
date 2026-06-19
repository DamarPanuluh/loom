use anyhow::Result;
use std::path::{Path, PathBuf};

pub mod queries;
pub mod schema;
pub mod sqlite;

// ---------------------------------------------------------------------------
// Typed read repository boundary
// ---------------------------------------------------------------------------

/// Generates the read repository in ONE place: the `GraphReadRepository` trait
/// plus its two delegating impls (the concrete `SqliteGraphStore` and the
/// read-only `GraphReadHandle` newtype). A new read method is declared once,
/// here — not maintained across a trait decl and two hand-written delegation
/// impls that silently drift apart.
///
/// The `SqliteGraphStore` impl forwards `self.$name(..)`: inherent methods take
/// resolution priority over trait methods, so this calls the concrete inherent
/// method rather than recursing. Every trait method therefore needs an inherent
/// method of the same name on `SqliteGraphStore` (see the
/// `count_intents_including_deprecated` inherent alias for the one that would
/// otherwise differ).
macro_rules! read_repository {
    ( $( fn $name:ident ( &self $(, $arg:ident : $argty:ty )* $(,)? ) -> $ret:ty ; )+ ) => {
        /// Backend-neutral read surface for command handlers.
        pub trait GraphReadRepository {
            $( fn $name(&self $(, $arg: $argty)*) -> $ret; )+
        }

        impl GraphReadRepository for sqlite::SqliteGraphStore {
            $( fn $name(&self $(, $arg: $argty)*) -> $ret { self.$name($($arg),*) } )+
        }

        impl GraphReadRepository for GraphReadHandle {
            $( fn $name(&self $(, $arg: $argty)*) -> $ret { self.0.$name($($arg),*) } )+
        }
    };
}

/// Read-only handle over the SQLite store. Opening through `open` (below) keeps
/// the write surface (~80 mutating inherent methods) out of read command
/// handlers' reach, and opens the file read-only.
pub struct GraphReadHandle(sqlite::SqliteGraphStore);

impl GraphReadHandle {
    pub fn open(root: &Path) -> Result<Self> {
        ensure_initialized(root)?;
        // Reads open the graph SQLITE_OPEN_READ_ONLY: no write lock, no WAL
        // write-lock, no per-invocation schema setup. open_readonly falls back to
        // the read-write open (which migrates) when the schema is stale, so an
        // older graph still upgrades on its first touch.
        Ok(Self(sqlite::SqliteGraphStore::open_readonly(
            &sqlite_db_path(root),
        )?))
    }
}

read_repository! {
    fn ensure_owned(&self, action: &str) -> Result<()>;
    fn query_snapshot(&self) -> Result<queries::QuerySnapshot>;
    fn graph_state(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::GraphState>;
    fn doctor_report(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::DoctorReport>;
    fn find_intents(&self, query: &str, limit: usize) -> Result<(Vec<queries::FindHit>, usize)>;
    fn door_matches(&self, query: &str, limit: usize) -> Result<queries::DoorMatches>;
    fn smell_report(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::SmellReport>;
    fn vocab_term_count(&self) -> Result<usize>;
    fn list_vocab_terms(&self) -> Result<Vec<crate::types::VocabTerm>>;
    fn layer_order(&self) -> Result<Vec<String>>;
    fn export_json(&self) -> Result<serde_json::Value>;
    fn list_intents(&self, status_filter: Option<&str>, level_filter: Option<&str>) -> Result<Vec<crate::types::Intent>>;
    fn get_intent(&self, id: &str) -> Result<Option<crate::types::Intent>>;
    fn list_implements_for_intent(&self, id: &str) -> Result<Vec<crate::types::Implements>>;
    fn list_validations(&self) -> Result<Vec<crate::types::Validation>>;
    fn validations_for_intent(&self, id: &str) -> Result<Vec<crate::types::Validation>>;
    fn list_interface_surfaces(&self) -> Result<Vec<crate::types::InterfaceSurface>>;
    fn list_all_calls(&self) -> Result<Vec<crate::types::CallsEdge>>;
    fn list_inbox_items(&self, status: Option<&str>, kind: Option<&str>) -> Result<Vec<crate::types::InboxItem>>;
    fn notes_for_target(&self, target_id: &str) -> Result<Vec<crate::types::Note>>;
    fn notes_by_kind(&self, kind: &str) -> Result<Vec<crate::types::Note>>;
    fn list_notes(&self, target_id: Option<&str>, kind: Option<&str>) -> Result<Vec<crate::types::Note>>;
    fn list_ignores(&self) -> Result<Vec<crate::types::Ignore>>;
    fn list_delegations(&self) -> Result<Vec<crate::types::Delegation>>;
    fn align_candidates(&self, snapshot: &queries::QuerySnapshot) -> Result<Vec<queries::AlignCandidate>>;
    fn list_hierarchy_for_intent(&self, id: &str) -> Result<Vec<crate::types::Hierarchy>>;
    fn edges_for_intent(&self, id: &str) -> Result<Vec<crate::types::RelatesTo>>;
    fn list_rules(&self) -> Result<Vec<crate::types::QualityRule>>;
    fn list_governs_for_intent(&self, id: &str) -> Result<Vec<crate::types::Governs>>;
    fn list_targets_for_hypothesis(&self, id: &str) -> Result<Vec<crate::types::TargetsEdge>>;
    fn align_candidate_count(&self, snapshot: &queries::QuerySnapshot) -> Result<i64>;
    fn prove_candidates(&self, snapshot: &queries::QuerySnapshot) -> Result<Vec<(crate::types::Hypothesis, f64)>>;
    fn list_hypotheses(&self, status: Option<&str>) -> Result<Vec<crate::types::Hypothesis>>;
    fn list_personas(&self) -> Result<Vec<crate::types::Persona>>;
    fn list_serves_for_persona(&self, id: &str) -> Result<Vec<crate::types::ServesEdge>>;
    fn list_journeys_for_persona(&self, id: &str) -> Result<Vec<crate::types::JourneysEdge>>;
    fn committed_export_stale(&self, root: &Path) -> Result<Option<bool>>;
    fn count_intents_including_deprecated(&self) -> Result<i64>;
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

pub fn loom_dir(cwd: &Path) -> PathBuf {
    cwd.join(".loom")
}

pub fn db_path(cwd: &Path) -> PathBuf {
    sqlite_db_path(cwd)
}

pub fn sqlite_db_path(cwd: &Path) -> PathBuf {
    loom_dir(cwd).join("graph.sqlite")
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
