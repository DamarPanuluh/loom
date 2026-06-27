use super::SqliteGraphStore;
use super::*;
use std::collections::{HashMap, HashSet};

const SUBSTANTIVE_NOTE_KINDS: &[&str] = &[
    "justification",
    "decision",
    "commentary",
    "idea",
    "question",
    "todo",
];

impl SqliteGraphStore {
    pub fn find_intents(&self, query: &str, limit: usize) -> Result<(Vec<FindHit>, usize)> {
        let intents = self.list_active_intents()?;
        let hierarchy = self.list_hierarchy_pairs()?;
        let aux_by_intent = self.aux_text_by_intent(&intents)?;
        rank_intents_from_parts(
            &intents,
            &hierarchy,
            &aux_by_intent,
            |intent_id| self.groundings_for_intent(intent_id),
            |intent_id| self.stale_edge_count(intent_id),
            query,
            limit,
        )
    }

    fn aux_text_by_intent(&self, intents: &[Intent]) -> Result<HashMap<String, String>> {
        let active_ids: HashSet<&str> = intents.iter().map(|i| i.id.as_str()).collect();
        let mut aux: HashMap<String, String> = HashMap::new();

        let mut append = |intent_id: &str, text: &str| {
            if !active_ids.contains(intent_id) || text.trim().is_empty() {
                return;
            }
            let entry = aux.entry(intent_id.to_string()).or_default();
            if !entry.is_empty() {
                entry.push(' ');
            }
            entry.push_str(text);
        };

        for kind in SUBSTANTIVE_NOTE_KINDS {
            for note in self.list_notes(None, Some(kind))? {
                if note.target_kind == "intent" {
                    append(&note.target_id, &note.text);
                }
            }
        }

        let snapshot = self.query_snapshot()?;
        for edge in &snapshot.relates {
            let text = format!("{} {} {}", edge.evidence, edge.notes, edge.criterion);
            append(&edge.from_id, &text);
            append(&edge.to_id, &text);
        }
        for edge in &snapshot.implements {
            let text = format!(
                "{} {} {} {}",
                edge.evidence, edge.notes, edge.criterion, edge.locator
            );
            append(&edge.intent_id, &text);
        }
        for edge in &snapshot.governs {
            let text = format!("{} {} {}", edge.evidence, edge.notes, edge.criterion);
            append(&edge.intent_id, &text);
        }

        Ok(aux)
    }

    pub fn door_matches(&self, query: &str, limit: usize) -> Result<DoorMatches> {
        Ok(door_matches_from_planes(
            self.list_vocab_terms()?,
            self.list_validations()?,
            self.list_rules()?,
            self.list_hypotheses(None)?,
            query,
            limit,
        ))
    }
}
