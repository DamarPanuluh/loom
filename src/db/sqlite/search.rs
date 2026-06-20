use super::SqliteGraphStore;
use super::*;

impl SqliteGraphStore {
    pub fn find_intents(&self, query: &str, limit: usize) -> Result<(Vec<FindHit>, usize)> {
        let intents = self.list_active_intents()?;
        let hierarchy = self.list_hierarchy_pairs()?;
        rank_intents_from_parts(
            &intents,
            &hierarchy,
            |intent_id| self.groundings_for_intent(intent_id),
            |intent_id| self.stale_edge_count(intent_id),
            query,
            limit,
        )
    }
    pub fn door_matches(&self, query: &str, limit: usize) -> Result<DoorMatches> {
        Ok(door_matches_from_planes(
            self.list_vocab_terms()?,
            self.list_validations()?,
            self.list_rules()?,
            query,
            limit,
        ))
    }
}
