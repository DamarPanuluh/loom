//! `SlicedRepo` — a read repository that restricts `query_snapshot` to one
//! slice's intent territory and delegates every other read to the real store.
//!
//! This is the single chokepoint that makes EVERY `loom next` mode slice-aware:
//! each mode builds its candidate set from `query_snapshot`, so filtering the
//! snapshot filters the queue — no per-mode-runner change. Detail fetches
//! (notes/validations/edges for an already-selected item) delegate unfiltered;
//! they only ever run against items the filtered candidate set already admitted.

use std::path::Path;

use anyhow::Result;

use crate::db::queries::QuerySnapshot;
use crate::db::GraphReadRepository;

pub(super) struct SlicedRepo<'a> {
    pub(super) inner: &'a dyn GraphReadRepository,
    pub(super) snapshot: QuerySnapshot,
}

impl GraphReadRepository for SlicedRepo<'_> {
    fn query_snapshot(&self) -> Result<QuerySnapshot> {
        Ok(self.snapshot.clone())
    }

    fn ensure_owned(&self, action: &str) -> Result<()> {
        self.inner.ensure_owned(action)
    }
    fn graph_state(
        &self,
        snapshot: &crate::db::queries::QuerySnapshot,
    ) -> Result<crate::db::queries::GraphState> {
        self.inner.graph_state(snapshot)
    }
    fn doctor_report(
        &self,
        snapshot: &crate::db::queries::QuerySnapshot,
    ) -> Result<crate::db::queries::DoctorReport> {
        self.inner.doctor_report(snapshot)
    }
    fn find_intents(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<(Vec<crate::db::queries::FindHit>, usize)> {
        self.inner.find_intents(query, limit)
    }
    fn door_matches(&self, query: &str, limit: usize) -> Result<crate::db::queries::DoorMatches> {
        self.inner.door_matches(query, limit)
    }
    fn smell_report(
        &self,
        snapshot: &crate::db::queries::QuerySnapshot,
    ) -> Result<crate::db::queries::SmellReport> {
        self.inner.smell_report(snapshot)
    }
    fn vocab_term_count(&self) -> Result<usize> {
        self.inner.vocab_term_count()
    }
    fn list_vocab_terms(&self) -> Result<Vec<crate::types::VocabTerm>> {
        self.inner.list_vocab_terms()
    }
    fn layer_order(&self) -> Result<Vec<String>> {
        self.inner.layer_order()
    }
    fn export_json(&self) -> Result<serde_json::Value> {
        self.inner.export_json()
    }
    fn list_intents(
        &self,
        status_filter: Option<&str>,
        level_filter: Option<&str>,
    ) -> Result<Vec<crate::types::Intent>> {
        self.inner.list_intents(status_filter, level_filter)
    }
    fn get_intent(&self, id: &str) -> Result<Option<crate::types::Intent>> {
        self.inner.get_intent(id)
    }
    fn list_implements_for_intent(&self, id: &str) -> Result<Vec<crate::types::Implements>> {
        self.inner.list_implements_for_intent(id)
    }
    fn list_validations(&self) -> Result<Vec<crate::types::Validation>> {
        self.inner.list_validations()
    }
    fn validations_for_intent(&self, id: &str) -> Result<Vec<crate::types::Validation>> {
        self.inner.validations_for_intent(id)
    }
    fn list_interface_surfaces(&self) -> Result<Vec<crate::types::InterfaceSurface>> {
        self.inner.list_interface_surfaces()
    }
    fn list_all_calls(&self) -> Result<Vec<crate::types::CallsEdge>> {
        self.inner.list_all_calls()
    }
    fn list_inbox_items(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::InboxItem>> {
        self.inner.list_inbox_items(status, kind)
    }
    fn notes_for_target(&self, target_id: &str) -> Result<Vec<crate::types::Note>> {
        self.inner.notes_for_target(target_id)
    }
    fn notes_by_kind(&self, kind: &str) -> Result<Vec<crate::types::Note>> {
        self.inner.notes_by_kind(kind)
    }
    fn list_notes(
        &self,
        target_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<crate::types::Note>> {
        self.inner.list_notes(target_id, kind)
    }
    fn list_ignores(&self) -> Result<Vec<crate::types::Ignore>> {
        self.inner.list_ignores()
    }
    fn list_delegations(&self) -> Result<Vec<crate::types::Delegation>> {
        self.inner.list_delegations()
    }
    fn align_candidates(
        &self,
        snapshot: &crate::db::queries::QuerySnapshot,
    ) -> Result<Vec<crate::db::queries::AlignCandidate>> {
        self.inner.align_candidates(snapshot)
    }
    fn list_hierarchy_for_intent(&self, id: &str) -> Result<Vec<crate::types::Hierarchy>> {
        self.inner.list_hierarchy_for_intent(id)
    }
    fn edges_for_intent(&self, id: &str) -> Result<Vec<crate::types::RelatesTo>> {
        self.inner.edges_for_intent(id)
    }
    fn list_rules(&self) -> Result<Vec<crate::types::QualityRule>> {
        self.inner.list_rules()
    }
    fn list_governs_for_intent(&self, id: &str) -> Result<Vec<crate::types::Governs>> {
        self.inner.list_governs_for_intent(id)
    }
    fn list_targets_for_hypothesis(&self, id: &str) -> Result<Vec<crate::types::TargetsEdge>> {
        self.inner.list_targets_for_hypothesis(id)
    }
    fn align_candidate_count(&self, snapshot: &crate::db::queries::QuerySnapshot) -> Result<i64> {
        self.inner.align_candidate_count(snapshot)
    }
    fn prove_candidates(
        &self,
        snapshot: &crate::db::queries::QuerySnapshot,
    ) -> Result<Vec<(crate::types::Hypothesis, f64)>> {
        self.inner.prove_candidates(snapshot)
    }
    fn list_hypotheses(&self, status: Option<&str>) -> Result<Vec<crate::types::Hypothesis>> {
        self.inner.list_hypotheses(status)
    }
    fn list_personas(&self) -> Result<Vec<crate::types::Persona>> {
        self.inner.list_personas()
    }
    fn list_serves_for_persona(&self, id: &str) -> Result<Vec<crate::types::ServesEdge>> {
        self.inner.list_serves_for_persona(id)
    }
    fn list_journeys_for_persona(&self, id: &str) -> Result<Vec<crate::types::JourneysEdge>> {
        self.inner.list_journeys_for_persona(id)
    }
    fn committed_export_stale(&self, root: &Path) -> Result<Option<bool>> {
        self.inner.committed_export_stale(root)
    }
    fn count_intents_including_deprecated(&self) -> Result<i64> {
        self.inner.count_intents_including_deprecated()
    }
}
