use anyhow::Result;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use crate::db::LoomDb;
use crate::types::{
    CodeFile, Governs, Implements, Intent, Note, QualityRule, RelatesTo, ValidatesEdge, Validation,
};

use super::{
    list_active_intents, list_all_governs, list_all_hierarchy, list_all_implements,
    list_all_validates, list_codefiles, list_notes, list_relates_to, list_rules, list_validations,
};

#[derive(Debug, Clone)]
pub struct QuerySnapshot {
    pub intents: Vec<Intent>,
    pub hierarchy: Vec<(String, String)>,
    pub relates: Vec<RelatesTo>,
    pub governs: Vec<Governs>,
    pub rules: Vec<QualityRule>,
    pub validates: Vec<ValidatesEdge>,
    pub validations: Vec<Validation>,
    pub implements: Vec<Implements>,
    pub codefiles: Vec<CodeFile>,
    pub with_code: HashSet<String>,
    pub degrees: HashMap<String, i64>,
    /// All notes (newest last), lazily loaded the first time a consumer asks.
    /// The Note label holds thousands of append-only `transition` notes on a
    /// mature graph; several point-in-time analyses over one snapshot want the
    /// whole set (`compute_smells_from`, the doctor integrity pass, and — once
    /// the graph is driven to green — the audit-gate `graph_state`). Loading it
    /// once and sharing keeps a single `next --all` / orientation pass from
    /// re-scanning the full label per consumer. Lazy so note-free snapshot
    /// users (`report`, `coverage`, `hotspots`) never pay for it. Point-in-time
    /// like every other field: the snapshot is a read view, not a live cursor.
    notes: OnceCell<Vec<Note>>,
}

impl QuerySnapshot {
    pub fn load(db: &dyn LoomDb) -> Result<Self> {
        let intents = list_active_intents(db)?;
        let hierarchy = list_all_hierarchy(db)?;
        let relates = list_relates_to(db, None)?;
        let governs = list_all_governs(db)?;
        let rules = list_rules(db)?;
        let validates = list_all_validates(db)?;
        let validations = list_validations(db)?;
        let implements = list_all_implements(db)?;
        let codefiles = list_codefiles(db)?;

        let with_code: HashSet<String> = implements.iter().map(|im| im.intent_id.clone()).collect();
        let active_ids: HashSet<&str> = intents.iter().map(|i| i.id.as_str()).collect();
        let mut degrees: HashMap<String, i64> = HashMap::new();
        for edge in &relates {
            if edge.inspection_status == "independent"
                || !active_ids.contains(edge.from_id.as_str())
                || !active_ids.contains(edge.to_id.as_str())
            {
                continue;
            }
            *degrees.entry(edge.from_id.clone()).or_insert(0) += 1;
            *degrees.entry(edge.to_id.clone()).or_insert(0) += 1;
        }

        Ok(Self {
            intents,
            hierarchy,
            relates,
            governs,
            rules,
            validates,
            validations,
            implements,
            codefiles,
            with_code,
            degrees,
            notes: OnceCell::new(),
        })
    }

    /// All notes (newest last), loaded once per snapshot and memoised. Equivalent
    /// to `list_notes(db, None, None)` for every caller, but a second consumer
    /// holding the same snapshot reuses the first scan instead of re-walking the
    /// (often thousands-strong) Note label. Single-threaded by construction —
    /// loom is one command per process — so the `OnceCell` set never races.
    pub fn notes(&self, db: &dyn LoomDb) -> Result<&[Note]> {
        if self.notes.get().is_none() {
            let loaded = list_notes(db, None, None)?;
            let _ = self.notes.set(loaded);
        }
        Ok(self.notes.get().expect("just initialised"))
    }
}

#[derive(Debug, Clone)]
pub struct DiscoverySnapshot {
    pub linked: HashSet<(String, String)>,
    pub files_of: HashMap<String, HashSet<usize>>,
    pub intents_on_file: HashMap<String, Vec<String>>,
    pub tokens_by_intent: HashMap<String, HashSet<String>>,
    /// Decoded `tags` per active intent (empty vec = untagged) — the bounded
    /// vocabulary facet; collisions feed `duplicated_responsibility` + ranking.
    pub tags_by_intent: HashMap<String, Vec<String>>,
    /// Intents per term — the rarity denominator for collision weighting.
    pub tag_counts: HashMap<String, usize>,
    pub import_links: HashSet<(usize, usize)>,
}

impl DiscoverySnapshot {
    pub fn from_query(snapshot: &QuerySnapshot) -> Result<Self> {
        let mut linked: HashSet<(String, String)> = HashSet::new();
        for edge in &snapshot.relates {
            linked.insert((edge.from_id.clone(), edge.to_id.clone()));
            linked.insert((edge.to_id.clone(), edge.from_id.clone()));
        }
        for (parent, child) in &snapshot.hierarchy {
            linked.insert((parent.clone(), child.clone()));
            linked.insert((child.clone(), parent.clone()));
        }

        let path_index: HashMap<&str, usize> = snapshot
            .codefiles
            .iter()
            .enumerate()
            .map(|(idx, cf)| (cf.path.as_str(), idx))
            .collect();

        let mut files_of: HashMap<String, HashSet<usize>> = HashMap::new();
        let mut intents_on_file: HashMap<String, Vec<String>> = HashMap::new();
        for im in &snapshot.implements {
            if let Some(&idx) = path_index.get(im.codefile_path.as_str()) {
                files_of
                    .entry(im.intent_id.clone())
                    .or_default()
                    .insert(idx);
            }
            intents_on_file
                .entry(im.codefile_path.clone())
                .or_default()
                .push(im.intent_id.clone());
        }

        let tokens_by_intent: HashMap<String, HashSet<String>> = snapshot
            .intents
            .iter()
            .map(|intent| {
                (
                    intent.id.clone(),
                    tokenize(&format!("{} {}", intent.name, intent.description)),
                )
            })
            .collect();

        let tags_by_intent: HashMap<String, Vec<String>> = snapshot
            .intents
            .iter()
            .map(|intent| Ok((intent.id.clone(), super::vocab::parse_tags(intent)?)))
            .collect::<Result<_>>()?;
        let mut tag_counts: HashMap<String, usize> = HashMap::new();
        for tags in tags_by_intent.values() {
            for t in tags {
                *tag_counts.entry(t.clone()).or_insert(0) += 1;
            }
        }

        let mut import_links: HashSet<(usize, usize)> = HashSet::new();
        for (from_idx, cf) in snapshot.codefiles.iter().enumerate() {
            for target in &cf.imports {
                if let Some(&to_idx) = path_index.get(target.as_str()) {
                    import_links.insert((from_idx, to_idx));
                    import_links.insert((to_idx, from_idx));
                }
            }
        }

        Ok(Self {
            linked,
            files_of,
            intents_on_file,
            tokens_by_intent,
            tags_by_intent,
            tag_counts,
            import_links,
        })
    }
}

pub(crate) fn tokenize(text: &str) -> HashSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "via", "with", "for", "that", "this", "from", "into", "are", "its", "all",
        "one", "not", "has", "have", "can", "per",
    ];
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOP.contains(w))
        .map(str::to_string)
        .collect()
}
