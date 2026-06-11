//! `loom find` — keyword search over the semantic plane (the "ask the map"
//! entry point: intent names + descriptions, BM25-ranked).
//!
//! Scoring runs in Rust, NOT in the engine. Grafeo 0.5.x ships a BM25 text
//! index (`CREATE INDEX … USING TEXT` + `CALL grafeo.search.text`), but the
//! procedure returns INTERNAL node ids that cannot be joined back to node
//! properties through GQL — the trailing `MATCH … WHERE id(n) = node_id`
//! parses, "succeeds", and is silently dropped (probed June 2026; same
//! reliability class as relationship-property matching, see the project
//! memory `grafeo-relationship-matching`). The corpus is hundreds of
//! LLM-written intents, so a scan scores in microseconds, works on graphs
//! created before this command existed, and is deterministic by construction.

use anyhow::Result;
use serde::Serialize;

use super::hierarchy::list_all_hierarchy;
use super::implements::list_implements_for_intent;
use super::intent::list_active_intents;
use crate::db::LoomDb;
use crate::types::Intent;

/// BM25 constants — the standard defaults; nothing about this corpus argues
/// for tuning them.
const K1: f64 = 1.2;
const B: f64 = 0.75;
/// A query term hitting the NAME is a stronger signal than one buried in the
/// description — names are the addresses of the semantic plane.
const NAME_WEIGHT: f64 = 2.0;
const DESC_WEIGHT: f64 = 1.0;

/// Mirrors grafeo's SimpleTokenizer (lowercase, split on non-alphanumeric,
/// drop stop words and single chars) so a future switch to the engine index
/// would not change what matches.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from",
    "has", "have", "in", "is", "it", "its", "no", "not", "of", "on", "or",
    "that", "the", "this", "to", "was", "were", "will", "with",
];

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|t| t.len() >= 2 && !STOPWORDS.contains(&t.as_str()))
        .collect()
}

/// One ranked hit, carrying enough context to act without a second lookup
/// (the `loom next` principle): where the intent sits in the tree, where its
/// code lives, and whether its claims are currently trustworthy.
#[derive(Debug, Clone, Serialize)]
pub struct FindHit {
    pub intent: Intent,
    pub score: f64,
    /// Ancestor names, root first, ending at the direct parent. Empty for
    /// roots (and for children of deprecated parents — the chain stops where
    /// computation stops seeing).
    pub parent_chain: Vec<String>,
    /// (path, locator) for every IMPLEMENTS grounding. Empty = not realized.
    pub groundings: Vec<(String, String)>,
    /// Edges touching this intent that sit in needs_reverification — the
    /// freshness warning: a non-zero count means parts of this answer
    /// describe code that has since changed.
    pub stale_edges: usize,
}

/// BM25 over active intents' names and descriptions (+ domain, folded into
/// the description field). Returns at most `limit` hits with score > 0,
/// ranked descending; ties break on name for deterministic output.
pub fn find_intents(db: &dyn LoomDb, query: &str, limit: usize) -> Result<Vec<FindHit>> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let intents = list_active_intents(db)?;
    let n = intents.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    // Per-document token lists for the two scored fields.
    let docs: Vec<(Vec<String>, Vec<String>)> = intents
        .iter()
        .map(|i| {
            let name = tokenize(&i.name);
            let desc = tokenize(&format!("{} {}", i.description, i.domain));
            (name, desc)
        })
        .collect();

    let avg = |field: fn(&(Vec<String>, Vec<String>)) -> &Vec<String>| -> f64 {
        let total: usize = docs.iter().map(|d| field(d).len()).sum();
        (total as f64 / n as f64).max(1.0)
    };
    let avg_name = avg(|d| &d.0);
    let avg_desc = avg(|d| &d.1);

    let bm25_field = |tokens: &[String], avg_len: f64, term: &str, df: usize| -> f64 {
        let tf = tokens.iter().filter(|t| *t == term).count() as f64;
        if tf == 0.0 {
            return 0.0;
        }
        let idf = (1.0 + (n as f64 - df as f64 + 0.5) / (df as f64 + 0.5)).ln();
        idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * tokens.len() as f64 / avg_len))
    };

    // Document frequency per term, computed ONCE across the corpus (it is the
    // same for every candidate — recomputing it per intent would be O(N²)).
    let dfs: Vec<(usize, usize)> = terms
        .iter()
        .map(|term| {
            let df_name = docs.iter().filter(|d| d.0.iter().any(|t| t == term)).count();
            let df_desc = docs.iter().filter(|d| d.1.iter().any(|t| t == term)).count();
            (df_name, df_desc)
        })
        .collect();

    let mut scored: Vec<(f64, &Intent)> = Vec::new();
    for (intent, doc) in intents.iter().zip(&docs) {
        let mut score = 0.0;
        for (term, (df_name, df_desc)) in terms.iter().zip(&dfs) {
            score += NAME_WEIGHT * bm25_field(&doc.0, avg_name, term, *df_name)
                + DESC_WEIGHT * bm25_field(&doc.1, avg_desc, term, *df_desc);
        }
        if score > 0.0 {
            scored.push((score, intent));
        }
    }
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.name.cmp(&b.1.name))
    });
    scored.truncate(limit);

    // Hydrate only the winners — context is fetched for ≤ limit intents.
    let parent_of: std::collections::HashMap<String, String> = list_all_hierarchy(db)?
        .into_iter()
        .map(|(p, c)| (c, p))
        .collect();
    let name_of: std::collections::HashMap<&str, &str> =
        intents.iter().map(|i| (i.id.as_str(), i.name.as_str())).collect();

    let mut hits = Vec::with_capacity(scored.len());
    for (score, intent) in scored {
        let mut chain = Vec::new();
        let mut cur = intent.id.as_str();
        while let Some(p) = parent_of.get(cur) {
            match name_of.get(p.as_str()) {
                Some(name) => chain.push((*name).to_string()),
                None => break, // deprecated ancestor — invisible to computation
            }
            cur = p;
        }
        chain.reverse();

        let groundings = list_implements_for_intent(db, &intent.id)?
            .into_iter()
            .map(|e| (e.codefile_path, e.locator))
            .collect();

        let stale_edges = stale_edge_count(db, &intent.id)?;

        hits.push(FindHit {
            intent: intent.clone(),
            score,
            parent_chain: chain,
            groundings,
            stale_edges,
        });
    }
    Ok(hits)
}

/// How many claims touching this intent went stale (needs_reverification)
/// across RELATES_TO, GOVERNS, VALIDATES, and IMPLEMENTS.
fn stale_edge_count(db: &dyn LoomDb, intent_id: &str) -> Result<usize> {
    const STALE: &str = "needs_reverification";
    let relates = super::relates_to::edges_for_intent(db, intent_id)?
        .iter()
        .filter(|e| e.inspection_status == STALE)
        .count();
    let governs = super::governs::list_governs_for_intent(db, intent_id)?
        .iter()
        .filter(|e| e.inspection_status == STALE)
        .count();
    let validates = super::validates::list_validates_for_intent(db, intent_id)?
        .iter()
        .filter(|e| e.inspection_status == STALE)
        .count();
    let implements = super::implements::list_implements_for_intent(db, intent_id)?
        .iter()
        .filter(|e| e.inspection_status == STALE)
        .count();
    Ok(relates + governs + validates + implements)
}
