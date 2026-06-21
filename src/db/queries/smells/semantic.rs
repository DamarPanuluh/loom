use std::collections::{HashMap, HashSet};

use super::{
    adjudicate, jaccard, teaching_for, AdjudicatedSmell, Smell, SmellCtx, DUP_TAG_WEIGHT,
    DUP_UNTAGGED_SHARED_TOKENS, DUP_UNTAGGED_SIMILARITY, TWIN_SIMILARITY,
};
use crate::db::queries::snapshot::DiscoverySnapshot;

/// Semantic plane — same-responsibility / duplicate-detection signals.
pub(super) fn detect_semantic_plane(
    ctx: &SmellCtx,
    smells: &mut Vec<Smell>,
    adj: &mut Vec<AdjudicatedSmell>,
) {
    detect_twin_intents(&ctx.intents_by_level, &ctx.linked, &ctx.signal_toks, smells);
    detect_duplicated_responsibility(
        &ctx.intents_by_level,
        &ctx.linked,
        ctx.discovery,
        &ctx.signal_toks,
        smells,
    );
    detect_duplicate_detection_unarmed(
        ctx.intents,
        ctx.discovery,
        &ctx.files_of,
        &ctx.newest_grounding,
        &ctx.roots,
        ctx.vocab_terms.len(),
        &ctx.last_decision,
        smells,
        adj,
    );
}
/// 1. Twin intents — split-brain in the semantic plane: two intents at the same
/// abstraction level that read like the same responsibility, with no recorded
/// relationship between them.
fn detect_twin_intents(
    intents_by_level: &HashMap<&str, Vec<&crate::types::Intent>>,
    linked: &HashSet<(&str, &str)>,
    signal_toks: &HashMap<&str, HashSet<String>>,
    smells: &mut Vec<Smell>,
) {
    for same_level in intents_by_level.values() {
        for (a, b) in candidate_pairs_from_keyed(same_level, &token_items(same_level, signal_toks))
        {
            if linked.contains(&(a.id.as_str(), b.id.as_str())) {
                continue;
            }
            let sim = jaccard(&signal_toks[a.id.as_str()], &signal_toks[b.id.as_str()]);
            if sim >= TWIN_SIMILARITY {
                smells.push(Smell {
                    kind: "twin_intents".into(),
                    score: sim * 10.0,
                    summary: format!(
                        "'{}' and '{}' read like the same responsibility twice",
                        a.name, b.name
                    ),
                    evidence: format!(
                        "name+description similarity {:.2} at the same level ({}), no edge between them",
                        sim, a.abstraction_level
                    ),
                    remedy: format!(
                        "loom edge explore {a} {b}  → ground a real relationship or mark independent with why; if one should absorb the other, propose the merge: `loom hypothesis add --name \"merge …\" --claim \"two intents own one responsibility\" --proposal \"<which absorbs which>\" --predicted-outcome \"one intent, one criterion; this finding disappears\" --target {a} --target {b}`",
                        a = a.id, b = b.id
                    ),
                    teaching: teaching_for("twin_intents"),
                });
            }
        }
    }
}

/// 1b. Duplicated responsibility — two same-level intents whose REGISTERED tags
/// collide (rarity-weighted), grounded in DISJOINT files with no import between
/// them and no recorded relationship. An untagged coded pair gets a stricter
/// lexical fallback so missing tags do not blind the detector.
fn detect_duplicated_responsibility(
    intents_by_level: &HashMap<&str, Vec<&crate::types::Intent>>,
    linked: &HashSet<(&str, &str)>,
    discovery: &DiscoverySnapshot,
    signal_toks: &HashMap<&str, HashSet<String>>,
    smells: &mut Vec<Smell>,
) {
    for same_level in intents_by_level.values() {
        let mut items = token_items(same_level, signal_toks);
        items.extend(tag_items(same_level, &discovery.tags_by_intent));
        for (a, b) in candidate_pairs_from_keyed(same_level, &items) {
            if linked.contains(&(a.id.as_str(), b.id.as_str())) {
                continue;
            }
            let (Some(fa), Some(fb)) = (
                discovery.files_of.get(a.id.as_str()),
                discovery.files_of.get(b.id.as_str()),
            ) else {
                continue; // duplicate implementation requires real code on both sides
            };
            if fa.intersection(fb).next().is_some() {
                continue; // overlapping_ownership owns this case
            }
            let imports = fa
                .iter()
                .flat_map(|x| fb.iter().map(move |y| (*x, *y)))
                .any(|p| discovery.import_links.contains(&p));
            if imports {
                continue; // undeclared_coupling owns this case
            }
            let empty_tags: &[String] = &[];
            let ta = discovery
                .tags_by_intent
                .get(a.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or(empty_tags);
            let tb = discovery
                .tags_by_intent
                .get(b.id.as_str())
                .map(Vec::as_slice)
                .unwrap_or(empty_tags);
            let (weight, shared_terms) =
                crate::db::queries::vocab::shared_tag_weight(ta, tb, &discovery.tag_counts);
            if weight >= DUP_TAG_WEIGHT {
                let term_detail = shared_terms
                    .iter()
                    .map(|t| {
                        format!(
                            "'{}' ({} intents carry it)",
                            t,
                            discovery.tag_counts.get(t).copied().unwrap_or(1)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                smells.push(Smell {
                        kind: "duplicated_responsibility".into(),
                        score: weight * 8.0,
                        summary: format!(
                            "'{}' and '{}' collide on rare vocabulary but live in unrelated code — same responsibility twice?",
                            a.name, b.name
                        ),
                        evidence: format!(
                            "shared tag(s) {} (collision weight {:.2}); groundings are disjoint with no import between them, so no physical detector can see this pair",
                            term_detail, weight
                        ),
                        remedy: format!(
                            "loom edge explore {a} {b}  → ground the real relationship or mark independent with why; if one implementation should absorb the other, propose the merge: `loom hypothesis add --name \"merge …\" --claim \"one responsibility is implemented twice\" --proposal \"<which absorbs which>\" --predicted-outcome \"one intent, one grounding; this finding disappears\" --target {a} --target {b}`",
                            a = a.id, b = b.id
                        ),
                        teaching: teaching_for("duplicated_responsibility"),
                    });
                continue;
            }
            if ta.is_empty() || tb.is_empty() {
                let shared_tokens: Vec<String> = signal_toks[a.id.as_str()]
                    .intersection(&signal_toks[b.id.as_str()])
                    .cloned()
                    .collect();
                let sim = jaccard(&signal_toks[a.id.as_str()], &signal_toks[b.id.as_str()]);
                if sim < DUP_UNTAGGED_SIMILARITY || shared_tokens.len() < DUP_UNTAGGED_SHARED_TOKENS
                {
                    continue;
                }
                let mut shared_tokens = shared_tokens;
                shared_tokens.sort();
                smells.push(Smell {
                        kind: "duplicated_responsibility".into(),
                        score: 2.0 + sim * 8.0,
                        summary: format!(
                            "'{}' and '{}' read alike, are under-tagged, and live in unrelated code — same responsibility twice?",
                            a.name, b.name
                        ),
                        evidence: format!(
                            "untagged lexical fallback: name+description similarity {:.2} with shared token(s) {}; tag coverage is {} vs {}; groundings are disjoint with no import between them",
                            sim,
                            shared_tokens.join(", "),
                            if ta.is_empty() { "none" } else { "present" },
                            if tb.is_empty() { "none" } else { "present" },
                        ),
                        remedy: format!(
                            "first make the detector honest: `loom vocab list` then `loom intent tag add {a} <term>` and/or `loom intent tag add {b} <term>`; then inspect the pair with `loom edge explore {a} {b}` to ground the real relationship or mark independent",
                            a = a.id, b = b.id
                        ),
                        teaching: teaching_for("duplicated_responsibility"),
                    });
            }
        }
    }
}

/// 1c. Duplicate detector coverage — once an intent has code, untagged coverage
/// is audit-relevant. A root decision may accept the blind spot; a newly
/// grounded untagged coded intent re-opens it.
fn detect_duplicate_detection_unarmed(
    intents: &[crate::types::Intent],
    discovery: &DiscoverySnapshot,
    files_of: &HashMap<&str, HashSet<&str>>,
    newest_grounding: &HashMap<&str, &str>,
    roots: &[&crate::types::Intent],
    vocab_registry_len: usize,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let coded: Vec<&crate::types::Intent> = intents
        .iter()
        .filter(|i| files_of.contains_key(i.id.as_str()))
        .collect();
    if coded.len() < 2 {
        return;
    }
    let untagged: Vec<&crate::types::Intent> = coded
        .iter()
        .copied()
        .filter(|i| {
            discovery
                .tags_by_intent
                .get(i.id.as_str())
                .map(|t| t.is_empty())
                .unwrap_or(true)
        })
        .collect();
    if untagged.is_empty() {
        return;
    }
    let registry = vocab_registry_len;
    let newest_untagged_grounding = untagged
        .iter()
        .filter_map(|i| newest_grounding.get(i.id.as_str()).copied())
        .max()
        .unwrap_or("");
    let sample: Vec<&str> = untagged.iter().take(5).map(|i| i.name.as_str()).collect();
    let summary = if registry == 0 {
        format!(
            "duplicated-responsibility tag detector is unarmed: no vocabulary and {} coded intent(s) are untagged",
            untagged.len()
        )
    } else {
        format!(
            "duplicated-responsibility tag detector is under-armed: {} of {} coded intent(s) are untagged",
            untagged.len(),
            coded.len()
        )
    };
    let adjudicated_note = roots.first().and_then(|root| {
        adjudicate(
            last_decision,
            "duplicate_detection_unarmed",
            root.id.as_str(),
            newest_untagged_grounding,
        )
    });
    if let Some(note) = adjudicated_note {
        adjudicated_out.push(AdjudicatedSmell {
            kind: "duplicate_detection_unarmed".into(),
            summary,
            ruling: note.text.clone(),
            ruled_by: note.author.clone(),
            ruled_at: note.created_at.clone(),
            reopens_when: "a new or newly grounded untagged coded intent lands after the ruling"
                .into(),
            teaching: teaching_for("duplicate_detection_unarmed"),
        });
    } else {
        smells.push(Smell {
            kind: "duplicate_detection_unarmed".into(),
            score: 4.0 + untagged.len() as f64,
            summary,
            evidence: format!(
                "{} of {} coded intent(s) have no registered tag; fallback lexical matching is weaker than bounded vocabulary. Examples: {}",
                untagged.len(),
                coded.len(),
                sample.join(" · ")
            ),
            remedy: if registry == 0 {
                "seed the bounded vocabulary (`loom vocab add <term> --why \"covers X, not Y\"`), then tag coded intents with `loom intent tag add <intent> <term>`; if the remaining blind spot is deliberate, record it on the graph root with `loom note add --smell \"duplicate_detection_unarmed:<root-id>\" --kind decision --text \"<why untagged coded intents are acceptable>\"`".into()
            } else {
                "tag the untagged coded intents from the registered vocabulary (`loom vocab list`, then `loom intent tag add <intent> <term>`); if the remaining blind spot is deliberate, record it on the graph root with `loom note add --smell \"duplicate_detection_unarmed:<root-id>\" --kind decision --text \"<why untagged coded intents are acceptable>\"`".into()
            },
            teaching: teaching_for("duplicate_detection_unarmed"),
        });
    }
}

/// `(token, same-level index)` items for every intent's signal tokens — the
/// keying that drives candidate generation for the semantic detectors.
fn token_items<'a>(
    same_level: &[&crate::types::Intent],
    signal_toks: &'a HashMap<&str, HashSet<String>>,
) -> Vec<(&'a str, usize)> {
    let mut items = Vec::new();
    for (idx, intent) in same_level.iter().enumerate() {
        if let Some(toks) = signal_toks.get(intent.id.as_str()) {
            for tok in toks {
                items.push((tok.as_str(), idx));
            }
        }
    }
    items
}

/// `(tag, same-level index)` items — the duplicated-responsibility detector also
/// fires on a shared rare tag, so tags join tokens as candidate keys.
fn tag_items<'a>(
    same_level: &[&crate::types::Intent],
    tags_by_intent: &'a HashMap<String, Vec<String>>,
) -> Vec<(&'a str, usize)> {
    let mut items = Vec::new();
    for (idx, intent) in same_level.iter().enumerate() {
        if let Some(tags) = tags_by_intent.get(intent.id.as_str()) {
            for tag in tags {
                items.push((tag.as_str(), idx));
            }
        }
    }
    items
}

/// Same-level intent pairs sharing at least one key — an EXACT superset of the
/// pairs either semantic detector can fire on (both need shared tokens or a
/// shared tag), assembled from an inverted index so detection is O(candidates)
/// instead of O(level²) — the scan that was killed at 90s on 20k intents. A key
/// shared by more than `BUCKET_CAP` same-level intents is non-discriminating (it
/// only yields low-similarity pairs below every threshold), so its O(k²)
/// expansion is skipped; the cap never fires below its size. Pairs come back
/// sorted, so smell order stays deterministic regardless of bucket iteration.
fn candidate_pairs_from_keyed<'a>(
    same_level: &[&'a crate::types::Intent],
    items: &[(&str, usize)],
) -> Vec<(&'a crate::types::Intent, &'a crate::types::Intent)> {
    const BUCKET_CAP: usize = 64;
    let mut buckets: HashMap<&str, Vec<usize>> = HashMap::new();
    for &(key, idx) in items {
        buckets.entry(key).or_default().push(idx);
    }
    let mut pairs: HashSet<(usize, usize)> = HashSet::new();
    for members in buckets.values_mut() {
        members.sort_unstable();
        members.dedup();
        if members.len() > BUCKET_CAP {
            continue;
        }
        for p in 0..members.len() {
            for q in (p + 1)..members.len() {
                pairs.insert((members[p], members[q]));
            }
        }
    }
    let mut pairs: Vec<(usize, usize)> = pairs.into_iter().collect();
    pairs.sort_unstable();
    pairs
        .into_iter()
        .map(|(i, j)| (same_level[i], same_level[j]))
        .collect()
}
