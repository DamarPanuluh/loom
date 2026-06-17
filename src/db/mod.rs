use anyhow::Result;
use std::path::{Path, PathBuf};

pub mod queries;
pub mod schema;
pub mod sqlite;

// ---------------------------------------------------------------------------
// Typed read repository boundary
// ---------------------------------------------------------------------------

/// Backend-neutral read surface for command handlers.
pub trait GraphReadRepository {
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
    fn list_intents(
        &self,
        status_filter: Option<&str>,
        level_filter: Option<&str>,
    ) -> Result<Vec<crate::types::Intent>>;
    fn get_intent(&self, id: &str) -> Result<Option<crate::types::Intent>>;
    fn list_implements_for_intent(&self, id: &str) -> Result<Vec<crate::types::Implements>>;
    fn list_validations(&self) -> Result<Vec<crate::types::Validation>>;
    fn validations_for_intent(&self, id: &str) -> Result<Vec<crate::types::Validation>>;
    fn list_interface_surfaces(&self) -> Result<Vec<crate::types::InterfaceSurface>>;
    fn list_all_calls(&self) -> Result<Vec<crate::types::CallsEdge>>;
    fn list_inbox_items(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::InboxItem>>;
    fn notes_for_target(&self, target_id: &str) -> Result<Vec<crate::types::Note>>;
    fn notes_by_kind(&self, kind: &str) -> Result<Vec<crate::types::Note>>;
    fn list_notes(
        &self,
        target_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::Note>>;
    fn list_ignores(&self) -> Result<Vec<crate::types::Ignore>>;
    fn list_delegations(&self) -> Result<Vec<crate::types::Delegation>>;
    fn align_candidates(
        &self,
        snapshot: &queries::QuerySnapshot,
    ) -> Result<Vec<queries::AlignCandidate>>;
    fn list_hierarchy_for_intent(&self, id: &str) -> Result<Vec<crate::types::Hierarchy>>;
    fn edges_for_intent(&self, id: &str) -> Result<Vec<crate::types::RelatesTo>>;
    fn list_rules(&self) -> Result<Vec<crate::types::QualityRule>>;
    fn list_governs_for_intent(&self, id: &str) -> Result<Vec<crate::types::Governs>>;
    fn list_targets_for_hypothesis(&self, id: &str) -> Result<Vec<crate::types::TargetsEdge>>;
    fn align_candidate_count(&self, snapshot: &queries::QuerySnapshot) -> Result<i64>;
    fn prove_candidates(
        &self,
        snapshot: &queries::QuerySnapshot,
    ) -> Result<Vec<(crate::types::Hypothesis, f64)>>;
    fn list_hypotheses(&self, status: Option<&str>) -> Result<Vec<crate::types::Hypothesis>>;
    fn list_personas(&self) -> Result<Vec<crate::types::Persona>>;
    fn list_serves_for_persona(&self, id: &str) -> Result<Vec<crate::types::ServesEdge>>;
    fn list_journeys_for_persona(&self, id: &str) -> Result<Vec<crate::types::JourneysEdge>>;
    fn committed_export_stale(&self, root: &Path) -> Result<Option<bool>>;
    fn count_intents_including_deprecated(&self) -> Result<i64>;
}

impl GraphReadRepository for sqlite::SqliteGraphStore {
    fn ensure_owned(&self, action: &str) -> Result<()> {
        self.ensure_owned(action)
    }

    fn query_snapshot(&self) -> Result<queries::QuerySnapshot> {
        self.query_snapshot()
    }

    fn graph_state(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::GraphState> {
        self.graph_state(snapshot)
    }

    fn doctor_report(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::DoctorReport> {
        self.doctor_report(snapshot)
    }

    fn find_intents(&self, query: &str, limit: usize) -> Result<(Vec<queries::FindHit>, usize)> {
        self.find_intents(query, limit)
    }

    fn door_matches(&self, query: &str, limit: usize) -> Result<queries::DoorMatches> {
        self.door_matches(query, limit)
    }

    fn smell_report(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::SmellReport> {
        self.smell_report(snapshot)
    }

    fn vocab_term_count(&self) -> Result<usize> {
        self.vocab_term_count()
    }

    fn list_vocab_terms(&self) -> Result<Vec<crate::types::VocabTerm>> {
        self.list_vocab_terms()
    }

    fn layer_order(&self) -> Result<Vec<String>> {
        self.layer_order()
    }

    fn export_json(&self) -> Result<serde_json::Value> {
        self.export_json()
    }

    fn list_intents(
        &self,
        status_filter: Option<&str>,
        level_filter: Option<&str>,
    ) -> Result<Vec<crate::types::Intent>> {
        self.list_intents(status_filter, level_filter)
    }

    fn get_intent(&self, id: &str) -> Result<Option<crate::types::Intent>> {
        self.get_intent(id)
    }

    fn list_implements_for_intent(&self, id: &str) -> Result<Vec<crate::types::Implements>> {
        self.list_implements_for_intent(id)
    }

    fn list_validations(&self) -> Result<Vec<crate::types::Validation>> {
        self.list_validations()
    }

    fn validations_for_intent(&self, id: &str) -> Result<Vec<crate::types::Validation>> {
        self.validations_for_intent(id)
    }

    fn list_interface_surfaces(&self) -> Result<Vec<crate::types::InterfaceSurface>> {
        self.list_interface_surfaces()
    }

    fn list_all_calls(&self) -> Result<Vec<crate::types::CallsEdge>> {
        self.list_all_calls()
    }

    fn list_inbox_items(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::InboxItem>> {
        self.list_inbox_items(status, kind)
    }

    fn notes_for_target(&self, target_id: &str) -> Result<Vec<crate::types::Note>> {
        self.notes_for_target(target_id)
    }

    fn notes_by_kind(&self, kind: &str) -> Result<Vec<crate::types::Note>> {
        self.notes_by_kind(kind)
    }

    fn list_notes(
        &self,
        target_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::Note>> {
        self.list_notes(target_id, kind)
    }

    fn list_ignores(&self) -> Result<Vec<crate::types::Ignore>> {
        self.list_ignores()
    }

    fn list_delegations(&self) -> Result<Vec<crate::types::Delegation>> {
        self.list_delegations()
    }

    fn align_candidates(
        &self,
        snapshot: &queries::QuerySnapshot,
    ) -> Result<Vec<queries::AlignCandidate>> {
        self.align_candidates(snapshot)
    }

    fn list_hierarchy_for_intent(&self, id: &str) -> Result<Vec<crate::types::Hierarchy>> {
        self.list_hierarchy_for_intent(id)
    }

    fn edges_for_intent(&self, id: &str) -> Result<Vec<crate::types::RelatesTo>> {
        self.edges_for_intent(id)
    }

    fn list_rules(&self) -> Result<Vec<crate::types::QualityRule>> {
        self.list_rules()
    }

    fn list_governs_for_intent(&self, id: &str) -> Result<Vec<crate::types::Governs>> {
        self.list_governs_for_intent(id)
    }

    fn list_targets_for_hypothesis(&self, id: &str) -> Result<Vec<crate::types::TargetsEdge>> {
        self.list_targets_for_hypothesis(id)
    }

    fn align_candidate_count(&self, snapshot: &queries::QuerySnapshot) -> Result<i64> {
        self.align_candidate_count(snapshot)
    }

    fn prove_candidates(
        &self,
        snapshot: &queries::QuerySnapshot,
    ) -> Result<Vec<(crate::types::Hypothesis, f64)>> {
        self.prove_candidates(snapshot)
    }

    fn list_hypotheses(&self, status: Option<&str>) -> Result<Vec<crate::types::Hypothesis>> {
        self.list_hypotheses(status)
    }

    fn list_personas(&self) -> Result<Vec<crate::types::Persona>> {
        self.list_personas()
    }

    fn list_serves_for_persona(&self, id: &str) -> Result<Vec<crate::types::ServesEdge>> {
        self.list_serves_for_persona(id)
    }

    fn list_journeys_for_persona(&self, id: &str) -> Result<Vec<crate::types::JourneysEdge>> {
        self.list_journeys_for_persona(id)
    }

    fn committed_export_stale(&self, root: &Path) -> Result<Option<bool>> {
        self.committed_export_stale(root)
    }

    fn count_intents_including_deprecated(&self) -> Result<i64> {
        Ok(sqlite::SqliteGraphStore::count_all_intents(self)? as i64)
    }
}

pub struct GraphReadHandle(sqlite::SqliteGraphStore);

impl GraphReadHandle {
    pub fn open(root: &Path) -> Result<Self> {
        ensure_initialized(root)?;
        Ok(Self(sqlite::SqliteGraphStore::open(&sqlite_db_path(root))?))
    }
}

impl GraphReadRepository for GraphReadHandle {
    fn ensure_owned(&self, action: &str) -> Result<()> {
        self.0.ensure_owned(action)
    }

    fn query_snapshot(&self) -> Result<queries::QuerySnapshot> {
        self.0.query_snapshot()
    }

    fn graph_state(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::GraphState> {
        self.0.graph_state(snapshot)
    }

    fn doctor_report(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::DoctorReport> {
        self.0.doctor_report(snapshot)
    }

    fn find_intents(&self, query: &str, limit: usize) -> Result<(Vec<queries::FindHit>, usize)> {
        self.0.find_intents(query, limit)
    }

    fn door_matches(&self, query: &str, limit: usize) -> Result<queries::DoorMatches> {
        self.0.door_matches(query, limit)
    }

    fn smell_report(&self, snapshot: &queries::QuerySnapshot) -> Result<queries::SmellReport> {
        self.0.smell_report(snapshot)
    }

    fn vocab_term_count(&self) -> Result<usize> {
        self.0.vocab_term_count()
    }

    fn list_vocab_terms(&self) -> Result<Vec<crate::types::VocabTerm>> {
        self.0.list_vocab_terms()
    }

    fn layer_order(&self) -> Result<Vec<String>> {
        self.0.layer_order()
    }

    fn export_json(&self) -> Result<serde_json::Value> {
        self.0.export_json()
    }

    fn list_intents(
        &self,
        status_filter: Option<&str>,
        level_filter: Option<&str>,
    ) -> Result<Vec<crate::types::Intent>> {
        self.0.list_intents(status_filter, level_filter)
    }

    fn get_intent(&self, id: &str) -> Result<Option<crate::types::Intent>> {
        self.0.get_intent(id)
    }

    fn list_implements_for_intent(&self, id: &str) -> Result<Vec<crate::types::Implements>> {
        self.0.list_implements_for_intent(id)
    }

    fn list_validations(&self) -> Result<Vec<crate::types::Validation>> {
        self.0.list_validations()
    }

    fn validations_for_intent(&self, id: &str) -> Result<Vec<crate::types::Validation>> {
        self.0.validations_for_intent(id)
    }

    fn list_interface_surfaces(&self) -> Result<Vec<crate::types::InterfaceSurface>> {
        self.0.list_interface_surfaces()
    }

    fn list_all_calls(&self) -> Result<Vec<crate::types::CallsEdge>> {
        self.0.list_all_calls()
    }

    fn list_inbox_items(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::InboxItem>> {
        self.0.list_inbox_items(status, kind)
    }

    fn notes_for_target(&self, target_id: &str) -> Result<Vec<crate::types::Note>> {
        self.0.notes_for_target(target_id)
    }

    fn notes_by_kind(&self, kind: &str) -> Result<Vec<crate::types::Note>> {
        self.0.notes_by_kind(kind)
    }

    fn list_notes(
        &self,
        target_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::Note>> {
        self.0.list_notes(target_id, kind)
    }

    fn list_ignores(&self) -> Result<Vec<crate::types::Ignore>> {
        self.0.list_ignores()
    }

    fn list_delegations(&self) -> Result<Vec<crate::types::Delegation>> {
        self.0.list_delegations()
    }

    fn align_candidates(
        &self,
        snapshot: &queries::QuerySnapshot,
    ) -> Result<Vec<queries::AlignCandidate>> {
        self.0.align_candidates(snapshot)
    }

    fn list_hierarchy_for_intent(&self, id: &str) -> Result<Vec<crate::types::Hierarchy>> {
        self.0.list_hierarchy_for_intent(id)
    }

    fn edges_for_intent(&self, id: &str) -> Result<Vec<crate::types::RelatesTo>> {
        self.0.edges_for_intent(id)
    }

    fn list_rules(&self) -> Result<Vec<crate::types::QualityRule>> {
        self.0.list_rules()
    }

    fn list_governs_for_intent(&self, id: &str) -> Result<Vec<crate::types::Governs>> {
        self.0.list_governs_for_intent(id)
    }

    fn list_targets_for_hypothesis(&self, id: &str) -> Result<Vec<crate::types::TargetsEdge>> {
        self.0.list_targets_for_hypothesis(id)
    }

    fn align_candidate_count(&self, snapshot: &queries::QuerySnapshot) -> Result<i64> {
        self.0.align_candidate_count(snapshot)
    }

    fn prove_candidates(
        &self,
        snapshot: &queries::QuerySnapshot,
    ) -> Result<Vec<(crate::types::Hypothesis, f64)>> {
        self.0.prove_candidates(snapshot)
    }

    fn list_hypotheses(&self, status: Option<&str>) -> Result<Vec<crate::types::Hypothesis>> {
        self.0.list_hypotheses(status)
    }

    fn list_personas(&self) -> Result<Vec<crate::types::Persona>> {
        self.0.list_personas()
    }

    fn list_serves_for_persona(&self, id: &str) -> Result<Vec<crate::types::ServesEdge>> {
        self.0.list_serves_for_persona(id)
    }

    fn list_journeys_for_persona(&self, id: &str) -> Result<Vec<crate::types::JourneysEdge>> {
        self.0.list_journeys_for_persona(id)
    }

    fn committed_export_stale(&self, root: &Path) -> Result<Option<bool>> {
        self.0.committed_export_stale(root)
    }

    fn count_intents_including_deprecated(&self) -> Result<i64> {
        self.0.count_intents_including_deprecated()
    }
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
