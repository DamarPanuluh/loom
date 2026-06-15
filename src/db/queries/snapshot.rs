use anyhow::Result;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};

use crate::types::{
    CodeFile, Governs, Implements, Intent, Note, QualityRule, RelatesTo, ValidatesEdge, Validation,
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
    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        intents: Vec<Intent>,
        hierarchy: Vec<(String, String)>,
        relates: Vec<RelatesTo>,
        governs: Vec<Governs>,
        rules: Vec<QualityRule>,
        validates: Vec<ValidatesEdge>,
        validations: Vec<Validation>,
        implements: Vec<Implements>,
        codefiles: Vec<CodeFile>,
        notes: Option<Vec<Note>>,
    ) -> Self {
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

        let note_cache = OnceCell::new();
        if let Some(notes) = notes {
            let _ = note_cache.set(notes);
        }

        Self {
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
            notes: note_cache,
        }
    }

    pub(crate) fn cached_notes(&self) -> Option<&[Note]> {
        self.notes.get().map(Vec::as_slice)
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
