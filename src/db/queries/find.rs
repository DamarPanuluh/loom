//! `loom find` — keyword search over the semantic plane (the "ask the map"
//! entry point: intent names, curated intent text, and lower-trust auxiliary
//! graph rationale, BM25-ranked).
//!
//! Scoring runs in Rust over typed SQLite rows, not in a database-specific
//! text index. The corpus is hundreds of LLM-written records, so a scan scores
//! in microseconds, works on every imported graph, and is deterministic by
//! construction. Scoring uses three weighted fields: name (2.0), curated
//! description/domain/layer/criterion (1.0), and auxiliary note/edge text (0.5).

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;

use crate::types::{Hypothesis, Intent, QualityRule, Validation, VocabTerm};

/// BM25 constants — the standard defaults; nothing about this corpus argues
/// for tuning them.
const K1: f64 = 1.2;
const B: f64 = 0.75;
/// A query term hitting the NAME is a stronger signal than one buried in the
/// description — names are the addresses of the semantic plane.
const NAME_WEIGHT: f64 = 2.0;
const DESC_WEIGHT: f64 = 1.0;
const AUX_WEIGHT: f64 = 0.5;

/// Lowercase, split on non-alphanumeric, drop stop words and single chars.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "has", "have", "in",
    "is", "it", "its", "no", "not", "of", "on", "or", "that", "the", "this", "to", "was", "were",
    "will", "with",
];

/// Shared with `loom door`'s cross-plane matchers — one tokenizer, one
/// definition of "matches".
pub(crate) fn tokenize(text: &str) -> Vec<String> {
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

pub(crate) fn rank_intents_from_parts(
    intents: &[Intent],
    hierarchy: &[(String, String)],
    aux_by_intent: &HashMap<String, String>,
    mut groundings_for_intent: impl FnMut(&str) -> Result<Vec<(String, String)>>,
    mut stale_count_for_intent: impl FnMut(&str) -> Result<usize>,
    query: &str,
    limit: usize,
) -> Result<(Vec<FindHit>, usize)> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let n = intents.len();
    if n == 0 {
        return Ok((Vec::new(), 0));
    }

    // Per-document token lists for the three scored fields.
    let docs: Vec<(Vec<String>, Vec<String>, Vec<String>)> = intents
        .iter()
        .map(|i| {
            let name = tokenize(&i.name);
            let desc = tokenize(&format!(
                "{} {} {} {}",
                i.description, i.domain, i.layer, i.criterion
            ));
            let aux = aux_by_intent
                .get(&i.id)
                .map(|text| tokenize(text))
                .unwrap_or_default();
            (name, desc, aux)
        })
        .collect();

    type IntentTokenFields = (Vec<String>, Vec<String>, Vec<String>);
    let avg = |field: fn(&IntentTokenFields) -> &Vec<String>| -> f64 {
        let total: usize = docs.iter().map(|d| field(d).len()).sum();
        (total as f64 / n as f64).max(1.0)
    };
    let avg_name = avg(|d| &d.0);
    let avg_desc = avg(|d| &d.1);
    let avg_aux = avg(|d| &d.2);

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
    let dfs: Vec<(usize, usize, usize)> = terms
        .iter()
        .map(|term| {
            let df_name = docs
                .iter()
                .filter(|d| d.0.iter().any(|t| t == term))
                .count();
            let df_desc = docs
                .iter()
                .filter(|d| d.1.iter().any(|t| t == term))
                .count();
            let df_aux = docs
                .iter()
                .filter(|d| d.2.iter().any(|t| t == term))
                .count();
            (df_name, df_desc, df_aux)
        })
        .collect();

    let mut scored: Vec<(f64, &Intent)> = Vec::new();
    for (intent, doc) in intents.iter().zip(&docs) {
        let mut score = 0.0;
        for (term, (df_name, df_desc, df_aux)) in terms.iter().zip(&dfs) {
            score += NAME_WEIGHT * bm25_field(&doc.0, avg_name, term, *df_name)
                + DESC_WEIGHT * bm25_field(&doc.1, avg_desc, term, *df_desc)
                + AUX_WEIGHT * bm25_field(&doc.2, avg_aux, term, *df_aux);
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
    let match_total = scored.len();
    scored.truncate(limit);

    // Hydrate only the winners — context is fetched for ≤ limit intents.
    let parent_of: std::collections::HashMap<String, String> =
        hierarchy.iter().cloned().map(|(p, c)| (c, p)).collect();
    let name_of: std::collections::HashMap<&str, &str> = intents
        .iter()
        .map(|i| (i.id.as_str(), i.name.as_str()))
        .collect();

    let mut hits = Vec::with_capacity(scored.len());
    for (score, intent) in scored {
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut cur = intent.id.as_str();
        while let Some(p) = parent_of.get(cur) {
            if !visited.insert(cur) {
                break;
            }
            match name_of.get(p.as_str()) {
                Some(name) => chain.push((*name).to_string()),
                None => break, // deprecated ancestor — invisible to computation
            }
            cur = p;
        }
        chain.reverse();

        let mut groundings = groundings_for_intent(&intent.id)?;
        groundings.sort();
        let stale_edges = stale_count_for_intent(&intent.id)?;

        hits.push(FindHit {
            intent: intent.clone(),
            score,
            parent_chain: chain,
            groundings,
            stale_edges,
        });
    }
    Ok((hits, match_total))
}

// ---------------------------------------------------------------------------
// `loom door` — cross-plane matches beyond the semantic plane
// ---------------------------------------------------------------------------

/// One non-intent hit for `loom door`: where the utterance's words already
/// live in the OTHER planes (vocabulary, consumer journeys, norms).
#[derive(Debug, Clone, Serialize)]
pub struct PlaneHit {
    pub id: String,
    pub name: String,
    /// Plane-specific context: the vocab term's why, the saga's run command,
    /// the rule's severity + description.
    pub detail: String,
    /// The query tokens that hit — the why of the match.
    pub matched: Vec<String>,
}

/// Everything the non-semantic planes know about an utterance, by token
/// overlap on names + descriptions (same tokenizer as `find_intents`, so one
/// definition of "matches"). Deliberately judgment-free: the door assembles
/// context for the LLM's routing decision; it never ranks landings. Ranked by
/// overlap count (ties on name), capped at `limit` per plane.
pub struct DoorMatches {
    pub vocab: Vec<PlaneHit>,
    pub sagas: Vec<PlaneHit>,
    pub rules: Vec<PlaneHit>,
    pub hypotheses: Vec<PlaneHit>,
}

pub(crate) fn door_matches_from_planes(
    vocab_terms: Vec<VocabTerm>,
    validations: Vec<Validation>,
    rules: Vec<QualityRule>,
    hypotheses: Vec<Hypothesis>,
    query: &str,
    limit: usize,
) -> DoorMatches {
    let terms = tokenize(query);
    let overlap = |text: &str| -> Vec<String> {
        if terms.is_empty() {
            return Vec::new();
        }
        let toks = tokenize(text);
        let mut hit: Vec<String> = terms.iter().filter(|t| toks.contains(t)).cloned().collect();
        hit.dedup();
        hit
    };
    let rank = |mut hits: Vec<PlaneHit>| -> Vec<PlaneHit> {
        hits.sort_by(|a, b| {
            b.matched
                .len()
                .cmp(&a.matched.len())
                .then_with(|| a.name.cmp(&b.name))
        });
        hits.truncate(limit);
        hits
    };

    let vocab = rank(
        vocab_terms
            .into_iter()
            .filter_map(|t| {
                let matched = overlap(&format!("{} {}", t.name, t.description));
                (!matched.is_empty()).then_some(PlaneHit {
                    id: t.id,
                    name: t.name,
                    detail: t.description,
                    matched,
                })
            })
            .collect(),
    );

    let sagas = rank(
        validations
            .into_iter()
            .filter(|v| v.validation_type == "saga")
            .filter_map(|v| {
                let matched = overlap(&format!("{} {} {}", v.name, v.description, v.command));
                (!matched.is_empty()).then(|| PlaneHit {
                    id: v.id,
                    name: v.name,
                    detail: format!("[{}] {}", v.last_result, v.command),
                    matched,
                })
            })
            .collect(),
    );

    let rules = rank(
        rules
            .into_iter()
            .filter_map(|r| {
                let matched = overlap(&format!(
                    "{} {} {}",
                    r.name, r.description, r.detection_logic
                ));
                (!matched.is_empty()).then(|| PlaneHit {
                    id: r.id,
                    name: r.name,
                    detail: format!("[{}] {}", r.severity, r.description),
                    matched,
                })
            })
            .collect(),
    );

    let hypotheses = rank(
        hypotheses
            .into_iter()
            .filter_map(|h| {
                let matched = overlap(&format!(
                    "{} {} {} {} {}",
                    h.name, h.claim, h.proposal, h.predicted_outcome, h.evidence
                ));
                (!matched.is_empty()).then(|| PlaneHit {
                    id: h.id,
                    name: h.name,
                    detail: format!("[{}] {}", h.status, h.claim),
                    matched,
                })
            })
            .collect(),
    );

    DoorMatches {
        vocab,
        sagas,
        rules,
        hypotheses,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(id: &str) -> Intent {
        Intent {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            criterion: String::new(),
            abstraction_level: "feature".to_string(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "active".to_string(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: String::new(),
            boundary: String::new(),
            lifecycle: "implemented".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn validation(id: &str) -> Validation {
        Validation {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            validation_type: "saga".to_string(),
            command: String::new(),
            last_run: String::new(),
            last_result: "not_run".to_string(),
            last_executed_run: String::new(),
            discrimination_status: String::new(),
        }
    }

    fn rule(id: &str) -> QualityRule {
        QualityRule {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            detection_logic: String::new(),
            severity: "warning".to_string(),
            kind: String::new(),
            inspection_effort: "mid".to_string(),
            evidence_examples: String::new(),
            signal_expectations: String::new(),
            applies_when: String::new(),
        }
    }

    fn hypothesis(id: &str) -> Hypothesis {
        Hypothesis {
            id: id.to_string(),
            name: id.to_string(),
            claim: String::new(),
            proposal: String::new(),
            predicted_outcome: String::new(),
            status: "proposed".to_string(),
            author: "llm".to_string(),
            evidence: String::new(),
            inspected_by: String::new(),
            last_inspected: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn rank_intents_scores_name_desc_then_aux() {
        let mut name_hit = intent("name-hit");
        name_hit.name = "quartz".to_string();
        let mut desc_hit = intent("desc-hit");
        desc_hit.description = "quartz".to_string();
        let aux_hit = intent("aux-hit");
        let intents = vec![aux_hit, desc_hit, name_hit];
        let aux_by_intent = HashMap::from([("aux-hit".to_string(), "quartz".to_string())]);

        let (hits, total) = rank_intents_from_parts(
            &intents,
            &[],
            &aux_by_intent,
            |_| Ok(Vec::new()),
            |_| Ok(0),
            "quartz",
            10,
        )
        .unwrap();

        assert_eq!(total, 3);
        let ids = hits
            .iter()
            .map(|h| h.intent.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["name-hit", "desc-hit", "aux-hit"]);
    }

    #[test]
    fn rank_intents_matches_criterion_as_curated_text() {
        let mut criterion_hit = intent("criterion-hit");
        criterion_hit.criterion = "zephyr".to_string();
        let intents = vec![criterion_hit];
        let aux_by_intent = HashMap::new();

        let (hits, total) = rank_intents_from_parts(
            &intents,
            &[],
            &aux_by_intent,
            |_| Ok(Vec::new()),
            |_| Ok(0),
            "zephyr",
            10,
        )
        .unwrap();

        assert_eq!(total, 1);
        assert_eq!(hits[0].intent.id, "criterion-hit");
    }

    #[test]
    fn door_matches_command_detection_logic_and_hypotheses() {
        let mut saga = validation("saga-hit");
        saga.command = "loom saga run chronofuse".to_string();
        let mut rule = rule("rule-hit");
        rule.detection_logic = "detects helioflux regressions".to_string();
        let mut hypothesis = hypothesis("hyp-hit");
        hypothesis.claim = "noctilume boundary drifts".to_string();

        let command = door_matches_from_planes(
            Vec::new(),
            vec![saga.clone()],
            vec![rule.clone()],
            vec![hypothesis.clone()],
            "chronofuse",
            10,
        );
        assert_eq!(command.sagas.len(), 1);
        assert_eq!(command.sagas[0].id, "saga-hit");

        let detection = door_matches_from_planes(
            Vec::new(),
            vec![saga],
            vec![rule],
            vec![hypothesis.clone()],
            "helioflux",
            10,
        );
        assert_eq!(detection.rules.len(), 1);
        assert_eq!(detection.rules[0].id, "rule-hit");

        let hyp = door_matches_from_planes(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![hypothesis],
            "noctilume",
            10,
        );
        assert_eq!(hyp.hypotheses.len(), 1);
        assert_eq!(hyp.hypotheses[0].id, "hyp-hit");
    }
}
