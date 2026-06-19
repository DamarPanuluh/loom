//! VocabTerm queries — the bounded tag vocabulary.
//!
//! Deliberately a REGISTRY OF KEYS, not a knowledge plane: terms carry no
//! lifecycle, no edges, no inspection state, so re-tagging is a plain UPDATE
//! and `merge` stays cheap. The bound exists to force collision — two agents
//! describing the same responsibility in open prose rarely share words, but
//! picking from a small inlined list they collide, and collisions are exactly
//! what `duplicated_responsibility` and discovery ranking consume. Drift is
//! DETECTED (the `vocab_drift` smell) and converged (`merge`), never prevented
//! by a closed list — a blocked agent shoehorning into a wrong term would lie
//! silently, which is worse than an honest new term.

use anyhow::Result;
use std::collections::HashMap;

use crate::types::{Intent, VocabTerm};

/// Hard cap on tags per intent. Agreeable LLMs fill every slot they're given;
/// tag spam makes everything collide with everything and the signal dies.
pub const MAX_TAGS_PER_INTENT: usize = 3;

// ---------------------------------------------------------------------------
// Term normalization
// ---------------------------------------------------------------------------

/// Normalize a term to its key form (trim + lowercase) and reject anything
/// that isn't a clean key: `[a-z0-9_-]+`. Terms are join keys, not prose.
pub fn normalize_term(raw: &str) -> Result<String> {
    let t = raw.trim().to_lowercase();
    if t.is_empty() {
        anyhow::bail!("A vocab term cannot be empty.");
    }
    if !t
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "'{t}' is not a valid term: use lowercase letters, digits, '-' or '_' \
             (terms are exact-match keys, not prose)."
        );
    }
    Ok(t)
}

/// An intent's tags. Native list since schema v5 (the row reader already
/// tolerates legacy JSON strings) — kept as a function so call sites and the
/// "absent = untagged" contract read unchanged.
pub fn parse_tags(intent: &Intent) -> Result<Vec<String>> {
    Ok(intent.tags.clone())
}

/// Canonical storage form: sorted + deduped, so the export stays
/// byte-deterministic and diffs stay clean.
pub fn encode_tags(mut tags: Vec<String>) -> Result<Vec<String>> {
    tags.sort();
    tags.dedup();
    Ok(tags)
}

/// How many intents carry each registered term — the rarity denominator for
/// collision weighting AND the usage column of `loom vocab list`. Counts over
/// the given intents (callers pass the active set).
pub fn tag_counts(intents: &[Intent]) -> Result<HashMap<String, usize>> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for i in intents {
        for t in parse_tags(i)? {
            *counts.entry(t).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

/// Rarity-weighted collision strength of two tag sets: Σ 1/freq(term) over the
/// shared terms. A term only 2 intents carry contributes 0.5; one 30 intents
/// carry contributes ~0.03 — so spammed broad terms decay toward zero weight
/// (the TF-IDF property that makes the signal robust to over-tagging) and the
/// detectors never need a closed list to stay precise.
pub fn shared_tag_weight(
    a: &[String],
    b: &[String],
    counts: &HashMap<String, usize>,
) -> (f64, Vec<String>) {
    let mut shared: Vec<String> = a.iter().filter(|t| b.contains(t)).cloned().collect();
    shared.sort();
    shared.dedup();
    let weight = shared
        .iter()
        .map(|t| 1.0 / (*counts.get(t).unwrap_or(&1)).max(1) as f64)
        .sum();
    (weight, shared)
}

// ---------------------------------------------------------------------------
// The nudge — "did you mean …?"
// ---------------------------------------------------------------------------

/// Levenshtein distance — terms are short keys, the DP table is tiny.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Light plural stemming so `retries`/`retry` and `caches`/`cache` compare on
/// the same stem. Deliberately tiny — real stemming would over-merge keys.
fn stem(t: &str) -> String {
    if let Some(base) = t.strip_suffix("ies").filter(|b| b.len() >= 2) {
        return format!("{base}y");
    }
    if let Some(base) = t.strip_suffix('s').filter(|b| b.len() >= 3) {
        return base.to_string();
    }
    t.to_string()
}

/// True when two terms read like the same word: same stem, one containing the
/// other (`auth`/`authn`), or a small edit distance. MORPHOLOGICAL drift only,
/// on purpose — semantic synonyms (`authn`/`authentication`) are for the agent
/// to catch from the inlined registry; a string metric guessing at meaning
/// would produce unresolvable false positives.
pub fn terms_look_alike(a: &str, b: &str) -> bool {
    if a == b {
        return false;
    }
    let (sa, sb) = (stem(a), stem(b));
    if sa == sb {
        return true;
    }
    let (a, b) = (sa.as_str(), sb.as_str());
    if a.len() >= 4 && b.len() >= 4 && (a.contains(b) || b.contains(a)) {
        return true;
    }
    let max_dist = if a.len().min(b.len()) >= 6 { 2 } else { 1 };
    edit_distance(a, b) <= max_dist
}

/// Registered terms nearest to an unknown input, with usage counts — the body
/// of the write-time "did you mean?" nudge. Look-alikes first, then everything
/// else by usage, so the caller can inline the WHOLE registry when it's small
/// (LLMs pick well from a presented list and guess badly from an unseen one).
pub fn nearest_terms<'a>(
    input: &str,
    terms: &'a [VocabTerm],
    counts: &HashMap<String, usize>,
) -> Vec<(&'a VocabTerm, usize)> {
    let mut ranked: Vec<(&VocabTerm, usize, bool)> = terms
        .iter()
        .map(|t| {
            let usage = *counts.get(&t.name).unwrap_or(&0);
            (t, usage, terms_look_alike(input, &t.name))
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then(b.1.cmp(&a.1))
            .then(a.0.name.cmp(&b.0.name))
    });
    ranked.into_iter().map(|(t, usage, _)| (t, usage)).collect()
}

// ---------------------------------------------------------------------------
// Suggesting terms — arm the registry from the graph's OWN vocabulary
// ---------------------------------------------------------------------------

/// A candidate vocabulary term mined from the graph's own intents.
#[derive(Debug, Clone)]
pub struct VocabSuggestion {
    /// The token, already a valid key (`tokenize` yields lowercase alphanumerics).
    pub term: String,
    /// Distinct active intents whose name/description contains the token —
    /// the collision potential (1 is useless, which is why the floor is 2).
    pub intent_count: usize,
    /// A few example intent names (alphabetical, capped) for the surface.
    pub examples: Vec<String>,
}

/// Candidate vocabulary terms mined from the graph's own intents: tokens that
/// recur across `>= min_intents` active intents and aren't already registered,
/// ranked by how many intents share them (collision potential) then by term.
///
/// Deliberately reuses the SAME tokenization the `duplicated_responsibility`
/// lexical fallback keys on, so registering the top suggestions arms exactly
/// that detector with bounded terms. loom proposes the KEY only — the
/// contrastive definition stays the operator's judgment, because a registry of
/// precise, distinguishable keys is the entire value (a canned generic pack
/// would inject terms this codebase never uses and dilute every collision).
pub fn suggest_vocab_terms(
    intents: &[Intent],
    registered: &std::collections::HashSet<String>,
    min_intents: usize,
) -> Vec<VocabSuggestion> {
    // Generic words that clear the shared tokenizer's small stoplist but name no
    // responsibility, so they make useless keys. Kept LOCAL to suggestion: the
    // duplicate-detector's lexical fallback reads the shared tokenizer, which we
    // must not perturb — this only declutters the human-facing candidate list.
    const NOISE: &[&str] = &[
        "every", "only", "also", "when", "then", "than", "each", "any", "may", "must", "use",
        "used", "uses", "such", "what", "which", "while", "where", "here", "there", "been",
        "being", "were", "your", "you", "their", "them", "they",
    ];
    let mut by_token: HashMap<String, Vec<String>> = HashMap::new();
    for intent in intents {
        // `tokenize` returns a set, so each intent contributes a token once.
        for tok in super::snapshot::tokenize(&format!("{} {}", intent.name, intent.description)) {
            if registered.contains(&tok) || NOISE.contains(&tok.as_str()) {
                continue;
            }
            by_token.entry(tok).or_default().push(intent.name.clone());
        }
    }
    // Ubiquity cap: a token in a large fraction of intents collides with
    // everything and discriminates nothing (the same reason tags are capped at
    // 3 — broad keys say nothing). Only applied once there are enough intents to
    // take a meaningful fraction of; below that, every recurring token is worth
    // surfacing.
    let cap = if intents.len() >= 12 {
        intents.len() / 4
    } else {
        usize::MAX
    };
    let mut out: Vec<VocabSuggestion> = by_token
        .into_iter()
        .filter(|(_, names)| names.len() >= min_intents && names.len() <= cap)
        .map(|(term, mut names)| {
            let intent_count = names.len();
            names.sort();
            names.truncate(3);
            VocabSuggestion {
                term,
                intent_count,
                examples: names,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.intent_count
            .cmp(&a.intent_count)
            .then_with(|| a.term.cmp(&b.term))
    });
    out
}

#[cfg(test)]
mod suggest_tests {
    use super::*;
    use std::collections::HashSet;

    fn intent(name: &str, desc: &str) -> Intent {
        Intent {
            id: name.to_string(),
            name: name.to_string(),
            description: desc.to_string(),
            criterion: String::new(),
            abstraction_level: "feature".to_string(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".to_string(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: String::new(),
            boundary: String::new(),
            lifecycle: "implemented".to_string(),
            created_at: "t0".to_string(),
            updated_at: "t0".to_string(),
        }
    }

    #[test]
    fn suggests_recurring_tokens_ranked_by_shared_intents() {
        let intents = vec![
            intent("store records", "persistence layer"),
            intent("reload store", "persistence reload path"),
            intent("public router", "routing of requests"),
        ];
        let none = HashSet::new();
        let got = suggest_vocab_terms(&intents, &none, 2);
        // "store" (2 intents) and "persistence" (2) recur; "routing"/"router"
        // appear once → excluded by the min-2 floor.
        let terms: Vec<&str> = got.iter().map(|s| s.term.as_str()).collect();
        assert!(terms.contains(&"store"), "got {terms:?}");
        assert!(terms.contains(&"persistence"), "got {terms:?}");
        assert!(!terms.contains(&"routing"), "got {terms:?}");
        // Every survivor recurs across at least the floor.
        assert!(got.iter().all(|s| s.intent_count >= 2));
    }

    #[test]
    fn ubiquitous_tokens_are_capped_out() {
        // 12 intents: "common" in all 12 (collides with everything → useless
        // key), "shared" in 3. The cap (len/4 = 3) keeps "shared", drops
        // "common"; "only" is generic noise and never surfaces.
        let mut intents: Vec<Intent> = (0..12)
            .map(|i| intent(&format!("intent {i} common only"), ""))
            .collect();
        for i in intents.iter_mut().take(3) {
            i.description = "shared".to_string();
        }
        let none = HashSet::new();
        let got = suggest_vocab_terms(&intents, &none, 2);
        let terms: Vec<&str> = got.iter().map(|s| s.term.as_str()).collect();
        assert!(terms.contains(&"shared"), "got {terms:?}");
        assert!(
            !terms.contains(&"common"),
            "ubiquitous token survived: {terms:?}"
        );
        assert!(!terms.contains(&"only"), "noise word survived: {terms:?}");
    }

    #[test]
    fn already_registered_terms_are_not_resuggested() {
        let intents = vec![
            intent("store records", "persistence"),
            intent("reload store", "persistence"),
        ];
        let registered: HashSet<String> = ["store".to_string()].into_iter().collect();
        let got = suggest_vocab_terms(&intents, &registered, 2);
        let terms: Vec<&str> = got.iter().map(|s| s.term.as_str()).collect();
        assert!(
            !terms.contains(&"store"),
            "registered term resuggested: {terms:?}"
        );
        assert!(terms.contains(&"persistence"));
    }
}
