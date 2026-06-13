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
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{esc, label, prop};
use crate::db::LoomDb;
use crate::types::{Intent, VocabTerm};

use super::row::{col_map, get, str_val};

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

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

pub fn insert_vocab_term(db: &dyn LoomDb, term: &VocabTerm) -> Result<()> {
    let q = format!(
        "INSERT (:{lbl} {{{id}: $id, {name}: $name, {desc}: $desc, {author}: $author, \
         {created}: $created}})",
        lbl = label::VOCAB_TERM,
        id = prop::ID,
        name = prop::NAME,
        desc = prop::DESCRIPTION,
        author = prop::AUTHOR,
        created = prop::CREATED_AT,
    );
    db.execute_with_params(
        &q,
        super::row::sparams(&[
            ("id", &term.id),
            ("name", &term.name),
            ("desc", &term.description),
            ("author", &term.author),
            ("created", &term.created_at),
        ]),
    )?;
    Ok(())
}

pub fn list_vocab_terms(db: &dyn LoomDb) -> Result<Vec<VocabTerm>> {
    let q = format!(
        "MATCH (n:{lbl}) RETURN n.{id}, n.{name}, n.{desc}, n.{author}, n.{created} \
         ORDER BY n.{name}",
        lbl = label::VOCAB_TERM,
        id = prop::ID,
        name = prop::NAME,
        desc = prop::DESCRIPTION,
        author = prop::AUTHOR,
        created = prop::CREATED_AT,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result
        .rows()
        .iter()
        .map(|row| row_to_term(row, &cols))
        .collect())
}

pub fn get_vocab_term(db: &dyn LoomDb, name: &str) -> Result<Option<VocabTerm>> {
    let q = format!(
        "MATCH (n:{lbl} {{{nm}: '{}'}}) RETURN n.{id}, n.{nm}, n.{desc}, n.{author}, n.{created}",
        esc(name),
        lbl = label::VOCAB_TERM,
        id = prop::ID,
        nm = prop::NAME,
        desc = prop::DESCRIPTION,
        author = prop::AUTHOR,
        created = prop::CREATED_AT,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| row_to_term(row, &cols)))
}

fn delete_vocab_term_node(db: &dyn LoomDb, name: &str) -> Result<()> {
    let q = format!(
        "MATCH (n:{lbl} {{{nm}: '{}'}}) DETACH DELETE n",
        esc(name),
        lbl = label::VOCAB_TERM,
        nm = prop::NAME,
    );
    db.execute(&q)?;
    Ok(())
}

fn row_to_term(row: &[Value], cols: &HashMap<&str, usize>) -> VocabTerm {
    VocabTerm {
        id: str_val(get(row, cols, "n.id")),
        name: str_val(get(row, cols, "n.name")),
        description: str_val(get(row, cols, "n.description")),
        author: str_val(get(row, cols, "n.author")),
        created_at: str_val(get(row, cols, "n.created_at")),
    }
}

// ---------------------------------------------------------------------------
// Intent tags
// ---------------------------------------------------------------------------

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

/// Overwrite an intent's tags (canonicalised). Returns false when the intent
/// doesn't exist.
pub fn set_intent_tags(
    db: &dyn LoomDb,
    id: &str,
    tags: Vec<String>,
    updated_at: &str,
) -> Result<bool> {
    if super::get_intent(db, id)?.is_none() {
        return Ok(false);
    }
    let encoded = encode_tags(tags)?;
    let mut p = super::row::sparams(&[("id", id), ("updated", updated_at)]);
    p.insert("tags".into(), super::row::list_param(&encoded));
    db.execute_with_params(
        &format!(
            "MATCH (n:{lbl} {{id: $id}}) SET n.{tags} = $tags, n.{updated} = $updated",
            lbl = label::INTENT,
            tags = prop::TAGS,
            updated = prop::UPDATED_AT,
        ),
        p,
    )?;
    Ok(true)
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

/// Merge term `from` into term `to`: every intent carrying `from` is retagged
/// to `to` (deduped), then the `from` node is deleted. The cheapness of this
/// operation is the POINT of keeping terms as keys instead of edges — drift
/// converges in one sweep with no inspection state to migrate.
/// Returns the number of intents retagged.
pub fn merge_vocab_terms(db: &dyn LoomDb, from: &str, to: &str, now: &str) -> Result<usize> {
    let mut retagged = 0usize;
    // ALL intents, including deprecated — history must not dangle.
    for intent in super::list_intents(db, None, None)? {
        let tags = parse_tags(&intent)?;
        if !tags.iter().any(|t| t == from) {
            continue;
        }
        let new_tags: Vec<String> = tags
            .into_iter()
            .map(|t| if t == from { to.to_string() } else { t })
            .collect();
        set_intent_tags(db, &intent.id, new_tags, now)?;
        retagged += 1;
    }
    delete_vocab_term_node(db, from)?;
    Ok(retagged)
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
