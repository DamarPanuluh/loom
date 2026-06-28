use std::collections::{HashMap, HashSet};

use super::{
    adjudicate, behavioral_symbol_kind, capped_join, command_or_public_surface,
    normalized_contract_string, scatter_threshold, short_contract_excerpt, teaching_for,
    AdjudicatedSmell, DeletionContext, Smell, SmellCtx, StringContractLoc,
    COMPLEX_SYMBOL_COGNITIVE, COMPLEX_SYMBOL_CYCLOMATIC, DEEPLY_NESTED_SYMBOL_DEPTH,
    LARGE_BEHAVIORAL_SYMBOL_LINES, MANY_ARGUMENTS, MANY_AWAITS, MANY_EXIT_PATHS,
    OVERSIZED_FILE_LINES, STRING_CONTRACT_SAFETY_PREAMBLE,
};
use crate::db::queries::snapshot::QuerySnapshot;

/// Shared reopen-trigger disclosure for the physical-plane detectors, whose
/// adjudications are all invalidated by a file content-hash change.
const REOPENS_ON_FILE_EDIT: &str = "the file is modified after the ruling";

/// Physical plane — ownership, file/symbol size, markers, string contracts.
pub(super) fn detect_physical_plane(
    ctx: &SmellCtx,
    smells: &mut Vec<Smell>,
    adj: &mut Vec<AdjudicatedSmell>,
) {
    detect_overlapping_ownership(
        ctx.intents,
        &ctx.linked,
        ctx.implements,
        &ctx.files_of,
        smells,
    );
    detect_scattered_intent(
        ctx.intents,
        &ctx.files_of,
        &ctx.newest_grounding,
        &ctx.last_decision,
        smells,
        adj,
    );
    detect_tangled_file(
        &ctx.intents_on_file,
        &ctx.newest_claim,
        &ctx.name_of,
        &ctx.last_decision,
        smells,
        adj,
    );
    detect_large_behavioral_symbol(ctx.snapshot, &ctx.last_decision, smells, adj);
    detect_complex_symbol(ctx.snapshot, &ctx.last_decision, smells, adj);
    detect_hub_file(ctx.snapshot, &ctx.last_decision, smells, adj);
    detect_panic_marker_risk(ctx.snapshot, &ctx.last_decision, smells, adj);
    detect_oversized_file(ctx.snapshot, &ctx.last_decision, smells, adj);
    detect_string_contract_duplicate(ctx.snapshot, &ctx.last_decision, smells, adj);
}
/// 2. Overlapping ownership — split-brain in the physical plane: two intents
/// grounded in the same file with no recorded relationship.
fn detect_overlapping_ownership(
    intents: &[crate::types::Intent],
    linked: &HashSet<(&str, &str)>,
    implements: &[crate::types::Implements],
    files_of: &HashMap<&str, HashSet<&str>>,
    smells: &mut Vec<Smell>,
) {
    // Two intents can overlap ONLY on a file they both ground — so walk each FILE's
    // owners instead of the O(N^2) intent grid. Cost is Σ(owners_per_file^2), bounded
    // by how many intents share a file rather than the total intent count: at
    // thousands of intents this stays linear in claims instead of quadratic in
    // intents. (A near-universal hub file shows up as `tangled_file`; this detector
    // does not re-explode it.) Behaviour is identical to the prior all-pairs scan —
    // same overlap test, same pair order (lower slice-index first), same output.
    let index_of: HashMap<&str, usize> = intents
        .iter()
        .enumerate()
        .map(|(i, it)| (it.id.as_str(), i))
        .collect();
    let name_of: HashMap<&str, &str> = intents
        .iter()
        .map(|it| (it.id.as_str(), it.name.as_str()))
        .collect();
    let mut claims_by_file: HashMap<&str, Vec<&crate::types::Implements>> = HashMap::new();
    for im in implements {
        // Match the active-owner filter already used by every physical-plane
        // detector. Retired/deprecated intents can keep historical IMPLEMENTS
        // rows, but they no longer own living code.
        if files_of.contains_key(im.intent_id.as_str()) {
            claims_by_file
                .entry(im.codefile_path.as_str())
                .or_default()
                .push(im);
        }
    }

    // (lower-index intent, higher-index intent) -> the set of shared ownership
    // targets across every file the pair co-owns.
    let mut pair_shared: HashMap<(&str, &str), HashSet<String>> = HashMap::new();
    for claims in claims_by_file.values() {
        for x in 0..claims.len() {
            for y in (x + 1)..claims.len() {
                let (cx, cy) = (claims[x], claims[y]);
                if cx.intent_id == cy.intent_id {
                    continue;
                }
                // Order the pair by slice index so the emitted (a, b) matches the
                // prior i<j outer loop exactly.
                let (ca, cb) =
                    if index_of.get(cx.intent_id.as_str()) <= index_of.get(cy.intent_id.as_str()) {
                        (cx, cy)
                    } else {
                        (cy, cx)
                    };
                let (aid, bid) = (ca.intent_id.as_str(), cb.intent_id.as_str());
                if linked.contains(&(aid, bid)) {
                    continue;
                }
                let a_loc = ca.locator.trim();
                let b_loc = cb.locator.trim();
                let target = match (a_loc.is_empty(), b_loc.is_empty()) {
                    // Whole-file ownership overlaps every claim inside the file.
                    (true, _) | (_, true) => Some(format!("{} (whole file)", ca.codefile_path)),
                    // Precise symbol ownership only overlaps when both intents claim
                    // the same located region. Different symbols in the same module
                    // are co-location/tangle evidence, not overlapping ownership.
                    (false, false) if a_loc == b_loc => {
                        Some(format!("{} @ {}", ca.codefile_path, a_loc))
                    }
                    (false, false) => None,
                };
                if let Some(t) = target {
                    pair_shared.entry((aid, bid)).or_default().insert(t);
                }
            }
        }
    }

    // Emit one smell per overlapping pair, in the same (a-index, b-index) order the
    // all-pairs loop produced.
    let mut pairs: Vec<((&str, &str), Vec<String>)> = pair_shared
        .into_iter()
        .map(|(p, s)| {
            let mut names: Vec<String> = s.into_iter().collect();
            names.sort();
            (p, names)
        })
        .collect();
    pairs.sort_by_key(|((a, b), _)| {
        (
            index_of.get(a).copied().unwrap_or(usize::MAX),
            index_of.get(b).copied().unwrap_or(usize::MAX),
        )
    });
    for ((aid, bid), names) in pairs {
        smells.push(Smell {
            kind: "overlapping_ownership".into(),
            score: 3.0 * names.len() as f64,
            summary: format!(
                "'{}' and '{}' both claim {} code ownership target(s) but no relationship is recorded",
                name_of.get(aid).copied().unwrap_or(aid),
                name_of.get(bid).copied().unwrap_or(bid),
                names.len()
            ),
            evidence: format!("shared: {}", capped_join(&names, ", ")),
            remedy: format!(
                "loom edge explore {aid} {bid}  → who owns what? ground the contract or mark independent with why"
            ),
            teaching: teaching_for("overlapping_ownership"),
        });
    }
}

/// 3. Scattered intent — one responsibility smeared across many files (threshold
/// scales with abstraction level). A decision note newer than the newest
/// grounding accepts the spread; a later grounding re-opens it.
fn detect_scattered_intent(
    intents: &[crate::types::Intent],
    files_of: &HashMap<&str, HashSet<&str>>,
    newest_grounding: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    for i in intents {
        let (Some(files), Some(threshold)) = (
            files_of.get(i.id.as_str()),
            scatter_threshold(&i.abstraction_level),
        ) else {
            continue;
        };
        if files.len() >= threshold {
            if let Some(note) = adjudicate(
                last_decision,
                "scattered_intent",
                i.id.as_str(),
                newest_grounding.get(i.id.as_str()).copied().unwrap_or(""),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "scattered_intent".into(),
                    summary: format!("'{}' is grounded in {} files", i.name, files.len()),
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "a new grounding lands on this intent".into(),
                    teaching: teaching_for("scattered_intent"),
                });
                continue;
            }
            let mut by_dir: HashMap<&str, usize> = HashMap::new();
            for f in files {
                let dir = std::path::Path::new(f)
                    .parent()
                    .and_then(|p| p.to_str())
                    .filter(|d| !d.is_empty())
                    .unwrap_or(".");
                *by_dir.entry(dir).or_insert(0) += 1;
            }
            let mut dirs: Vec<(&str, usize)> = by_dir.into_iter().collect();
            dirs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let cluster_items: Vec<String> =
                dirs.iter().map(|(d, n)| format!("{d} ({n})")).collect();
            let clusters = capped_join(&cluster_items, " · ");
            smells.push(Smell {
                kind: "scattered_intent".into(),
                score: files.len() as f64,
                summary: format!(
                    "'{}' is grounded in {} files — responsibility may be fragmented",
                    i.name,
                    files.len()
                ),
                evidence: format!(
                    "a {}-level intent normally stays under {} files; groundings cluster by directory: {}",
                    i.abstraction_level, threshold, clusters
                ),
                remedy: format!(
                    "split the INTENT, not the code (a too-coarse seed is normal): add a child intent per cohesive slice along the directory clusters, `loom edge hierarchy {id} <child>`, then move groundings down (`loom edge unimplement {id} '<dir>/**'` + `loom edge implement <child> …`); if the CODE itself is the problem, propose that separately: `loom hypothesis add … --claim \"<why this layout fights the design>\" --target {id}`; if the spread is DELIBERATE, record the call: `loom note add --smell \"scattered_intent:{id}\" --kind decision --text \"<why this layout is right>\"` resolves this finding (a new grounding re-opens it)",
                    id = i.id
                ),
                teaching: teaching_for("scattered_intent"),
            });
        }
    }
}

/// 4. Tangled file — one file serving many intents (`loom hotspots` made
/// actionable). A decision note on the file newer than its newest claim accepts
/// the cohabitation; a new claim re-opens it.
fn detect_tangled_file(
    intents_on_file: &HashMap<&str, Vec<&str>>,
    newest_claim: &HashMap<&str, &str>,
    name_of: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    // Self-calibrating: a "tangle" is an ownership OUTLIER for THIS repo, not a fixed
    // count — what reads as too-many-owners depends on how the repo carves intents.
    // The Tukey outlier fence over the distinct-owner-count distribution. It shares
    // this distribution with the coupling cap (which uses the FAR-outlier fence), so
    // anything the cap defers is always caught here (FAR_OUTLIER_K > OUTLIER_K).
    let owner_counts: Vec<usize> = intents_on_file
        .values()
        .map(|v| v.iter().collect::<HashSet<_>>().len())
        .collect();
    let Some(fence) = crate::db::queries::calibrate::tukey_upper_fence(
        &owner_counts,
        crate::db::queries::calibrate::OUTLIER_K,
    ) else {
        return; // too few grounded files to define a distribution -> flag nothing
    };
    for (path, iids) in intents_on_file {
        let distinct: HashSet<&&str> = iids.iter().collect();
        if (distinct.len() as f64) > fence {
            if let Some(note) = adjudicate(
                last_decision,
                "tangled_file",
                path,
                newest_claim.get(path).copied().unwrap_or(""),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "tangled_file".into(),
                    summary: format!("{} serves {} distinct intents", path, distinct.len()),
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "a new IMPLEMENTS claim lands on this file".into(),
                    teaching: teaching_for("tangled_file"),
                });
                continue;
            }
            let mut names: Vec<&str> = distinct
                .iter()
                .filter_map(|id| name_of.get(**id).copied())
                .collect();
            names.sort();
            smells.push(Smell {
                kind: "tangled_file".into(),
                score: distinct.len() as f64,
                summary: format!("{} serves {} distinct intents", path, distinct.len()),
                evidence: format!("intents: {}", capped_join(&names, " · ")),
                remedy: format!(
                    "a code split is a redesign — propose it so it gets proven before it becomes work: `loom hypothesis add --name \"split {path}\" --claim \"{path} serves {n} unrelated intents\" --proposal \"<the split, along intent lines>\" --predicted-outcome \"each intent grounds in its own module; this finding disappears\"` with a --target per owning intent; rule the cohabitation deliberate ONLY after reading the file: `loom note add --smell \"tangled_file:{path}\" --kind decision --text \"<the shared boundary that makes these intents one home, and why splitting is wrong HERE — NOT 'cohesive: one module', which restates the finding>\"` resolves this finding (a new claim re-opens it). loom rejects a vacuous or templated ruling — audit each file on its own contents",
                    n = distinct.len(),
                ),
                teaching: teaching_for("tangled_file"),
            });
        }
    }
}

/// 4b. Large behavioral symbol — a pure physical snapshot signal for
/// functions/methods/defs/impls whose span is large enough to deserve
/// inspection.
fn detect_large_behavioral_symbol(
    snapshot: &QuerySnapshot,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    for cf in &snapshot.codefiles {
        for f in &cf.symbol_facts {
            if f.is_test || !behavioral_symbol_kind(f.kind.as_str()) {
                continue;
            }
            let span = f.line_end.saturating_sub(f.line_start) + 1;
            if span < LARGE_BEHAVIORAL_SYMBOL_LINES {
                continue;
            }
            let summary = format!("{} in {} spans {} lines", f.label, cf.path, span);
            let adj_scope = format!("{}:{}", cf.path, f.label);
            if let Some(note) = adjudicate(
                last_decision,
                "large_behavioral_symbol",
                &adj_scope,
                cf.last_modified.as_str(),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "large_behavioral_symbol".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: REOPENS_ON_FILE_EDIT.into(),
                    teaching: teaching_for("large_behavioral_symbol"),
                });
                continue;
            }
            let visibility = if f.visibility.is_empty() {
                "unknown"
            } else {
                f.visibility.as_str()
            };
            smells.push(Smell {
                kind: "large_behavioral_symbol".into(),
                score: span as f64 / 20.0,
                summary,
                evidence: format!(
                    "{}:{}-{} is a non-test {} symbol (kind={}, visibility={}) above the {}-line threshold",
                    cf.path,
                    f.line_start,
                    f.line_end,
                    span,
                    f.kind,
                    visibility,
                    LARGE_BEHAVIORAL_SYMBOL_LINES
                ),
                remedy: format!(
                    "inspect {}:{}-{}; split the distinct phases/modes into named helpers, or rule it deliberate ONLY after reading the body: `loom note add --smell \"large_behavioral_symbol:{}:{}\" --kind decision --text \"<the extraction you considered and the concrete reason {} resists it HERE — NOT a restatement of its size like 'reflects N cases'>\"` resolves THIS finding (editing the file re-opens it). loom rejects a vacuous ruling or one that reuses your wording from another finding — audit each symbol on its own body",
                    cf.path, f.line_start, f.line_end, cf.path, f.label, f.label
                ),
                teaching: teaching_for("large_behavioral_symbol"),
            });
        }
    }
}

/// 4b2. Complex symbol — deterministic syntax metrics over branchiness,
/// nesting, exits, broad signatures, and async suspension points. Advisory: a
/// routing signal for inspection rather than an automatic design verdict.
fn detect_complex_symbol(
    snapshot: &QuerySnapshot,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    for cf in &snapshot.codefiles {
        for f in &cf.symbol_facts {
            if f.is_test || !behavioral_symbol_kind(f.kind.as_str()) {
                continue;
            }
            let m = &f.metrics;
            let mut triggers = Vec::new();
            if m.cyclomatic >= COMPLEX_SYMBOL_CYCLOMATIC {
                triggers.push(format!("cyclomatic {}", m.cyclomatic));
            }
            if m.cognitive >= COMPLEX_SYMBOL_COGNITIVE {
                triggers.push(format!("cognitive {}", m.cognitive));
            }
            if m.max_nesting >= DEEPLY_NESTED_SYMBOL_DEPTH {
                triggers.push(format!("nesting {}", m.max_nesting));
            }
            if m.exit_count >= MANY_EXIT_PATHS {
                triggers.push(format!("exits {}", m.exit_count));
            }
            if m.arg_count >= MANY_ARGUMENTS {
                triggers.push(format!("args {}", m.arg_count));
            }
            if m.await_count >= MANY_AWAITS {
                triggers.push(format!("awaits {}", m.await_count));
            }
            if triggers.is_empty() {
                continue;
            }

            let summary = format!(
                "{} in {} has high control-flow complexity ({})",
                f.label,
                cf.path,
                triggers.join(", ")
            );
            let adj_scope = format!("{}:{}", cf.path, f.label);
            if let Some(note) = adjudicate(
                last_decision,
                "complex_symbol",
                &adj_scope,
                cf.last_modified.as_str(),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "complex_symbol".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: REOPENS_ON_FILE_EDIT.into(),
                    teaching: teaching_for("complex_symbol"),
                });
                continue;
            }
            let span = f.line_end.saturating_sub(f.line_start) + 1;
            smells.push(Smell {
                kind: "complex_symbol".into(),
                score: m.cognitive as f64
                    + m.cyclomatic as f64
                    + (m.max_nesting as f64 * 2.0)
                    + m.exit_count as f64
                    + m.await_count as f64,
                summary,
                evidence: format!(
                    "{}:{}-{} span={} cyclomatic={} cognitive={} branches={} nesting={} exits={} args={} closures={} awaits={}",
                    cf.path,
                    f.line_start,
                    f.line_end,
                    span,
                    m.cyclomatic,
                    m.cognitive,
                    m.branch_count,
                    m.max_nesting,
                    m.exit_count,
                    m.arg_count,
                    m.closure_count,
                    m.await_count,
                ),
                remedy: format!(
                    "inspect {}:{}-{}; split phases/modes/failure paths into named units, add direct proofs for risky branches, or rule it deliberate: `loom note add --smell \"complex_symbol:{}:{}\" --kind decision --text \"<the branch decomposition considered and why this control-flow shape is right HERE>\"` resolves THIS finding (editing the file re-opens it)",
                    cf.path, f.line_start, f.line_end, cf.path, f.label
                ),
                teaching: teaching_for("complex_symbol"),
            });
        }
    }
}

/// 4b3. Hub file — reverse-import centrality. Advisory: shared primitives are
/// allowed, but broad dependency blast radius should be visible to the driver.
fn detect_hub_file(
    snapshot: &QuerySnapshot,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let mut importers: HashMap<&str, Vec<&str>> = HashMap::new();
    for cf in &snapshot.codefiles {
        for imported in &cf.imports {
            if imported != &cf.path {
                importers
                    .entry(imported.as_str())
                    .or_default()
                    .push(cf.path.as_str());
            }
        }
    }
    // Self-calibrating: a hub is a reverse-import OUTLIER for THIS repo, not a fixed
    // count — what reads as "heavily imported" depends on the import topology. Take
    // the Tukey outlier fence over the distinct reverse-import distribution.
    let rev_counts: Vec<usize> = importers
        .values()
        .map(|v| {
            let mut u = v.clone();
            u.sort();
            u.dedup();
            u.len()
        })
        .collect();
    let Some(fence) = crate::db::queries::calibrate::tukey_upper_fence(
        &rev_counts,
        crate::db::queries::calibrate::OUTLIER_K,
    ) else {
        return; // too few imported files to define a distribution -> flag nothing
    };
    for cf in &snapshot.codefiles {
        let Some(mut incoming) = importers.remove(cf.path.as_str()) else {
            continue;
        };
        incoming.sort();
        incoming.dedup();
        if (incoming.len() as f64) <= fence {
            continue;
        }
        let summary = format!("{} is imported by {} file(s)", cf.path, incoming.len());
        if let Some(note) = adjudicate(
            last_decision,
            "hub_file",
            cf.path.as_str(),
            cf.last_modified.as_str(),
        ) {
            adjudicated_out.push(AdjudicatedSmell {
                kind: "hub_file".into(),
                summary,
                ruling: note.text.clone(),
                ruled_by: note.author.clone(),
                ruled_at: note.created_at.clone(),
                reopens_when: REOPENS_ON_FILE_EDIT.into(),
                teaching: teaching_for("hub_file"),
            });
            continue;
        }
        smells.push(Smell {
            kind: "hub_file".into(),
            score: incoming.len() as f64,
            summary,
            evidence: format!(
                "reverse imports: {}",
                capped_join(&incoming, ", ")
            ),
            remedy: format!(
                "inspect {path}'s public surface and importers; split accidental utility grab-bags along intent/module lines, or rule the centrality deliberate: `loom note add --smell \"hub_file:{path}\" --kind decision --text \"<why this module is a stable shared hub>\"` resolves THIS finding (editing the file re-opens it)",
                path = cf.path
            ),
            teaching: teaching_for("hub_file"),
        });
    }
}

/// 4c. Panic/unwrap/todo markers in implemented behavior — places where
/// sad-path behavior depends on an invariant that must be inspected, proven, or
/// explicitly accepted.
fn detect_panic_marker_risk(
    snapshot: &QuerySnapshot,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    for cf in &snapshot.codefiles {
        for f in &cf.symbol_facts {
            if f.is_test || !behavioral_symbol_kind(f.kind.as_str()) || f.panic_marker_count == 0 {
                continue;
            }
            let summary = format!(
                "{} in {} has {} panic/unfinished marker(s)",
                f.label, cf.path, f.panic_marker_count
            );
            let adj_scope = format!("{}:{}", cf.path, f.label);
            if let Some(note) = adjudicate(
                last_decision,
                "panic_marker_risk",
                &adj_scope,
                cf.last_modified.as_str(),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "panic_marker_risk".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: REOPENS_ON_FILE_EDIT.into(),
                    teaching: teaching_for("panic_marker_risk"),
                });
                continue;
            }
            let markers = if f.panic_markers.is_empty() {
                "unknown".to_string()
            } else {
                f.panic_markers.join(", ")
            };
            let path_weight = if command_or_public_surface(&cf.path, f) {
                2.0
            } else {
                1.0
            };
            smells.push(Smell {
                kind: "panic_marker_risk".into(),
                score: f.panic_marker_count as f64 * path_weight,
                summary,
                evidence: format!(
                    "{}:{}-{} markers=[{}] count={}{}",
                    cf.path,
                    f.line_start,
                    f.line_end,
                    markers,
                    f.panic_marker_count,
                    if path_weight > 1.0 {
                        " on command/public surface"
                    } else {
                        ""
                    }
                ),
                remedy: format!(
                    "inspect {}:{}-{}; replace recoverable aborts with handled errors/proofs, move unfinished behavior to planned work, or accept the invariant: `loom note add --smell \"panic_marker_risk:{}:{}\" --kind decision --text \"<why these markers are deliberate>\"` resolves THIS finding (editing the file re-opens it)",
                    cf.path, f.line_start, f.line_end, cf.path, f.label
                ),
                teaching: teaching_for("panic_marker_risk"),
            });
        }
    }
}

/// 4d. Oversized file — the irreducible god-file signal: the file's total
/// physical extent (last symbol end line), keyed on `oversized_file:<path>` so a
/// per-symbol large_behavioral_symbol ruling cannot launder it.
fn detect_oversized_file(
    snapshot: &QuerySnapshot,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    for cf in &snapshot.codefiles {
        let extent = cf
            .symbol_facts
            .iter()
            .map(|f| f.line_end)
            .max()
            .unwrap_or(0);
        if extent < OVERSIZED_FILE_LINES {
            continue;
        }
        let summary = format!(
            "{} spans ~{} lines (last symbol ends at line {})",
            cf.path, extent, extent
        );
        if let Some(note) = adjudicate(
            last_decision,
            "oversized_file",
            cf.path.as_str(),
            cf.last_modified.as_str(),
        ) {
            adjudicated_out.push(AdjudicatedSmell {
                kind: "oversized_file".into(),
                summary,
                ruling: note.text.clone(),
                ruled_by: note.author.clone(),
                ruled_at: note.created_at.clone(),
                reopens_when: REOPENS_ON_FILE_EDIT.into(),
                teaching: teaching_for("oversized_file"),
            });
            continue;
        }
        smells.push(Smell {
            kind: "oversized_file".into(),
            score: extent as f64 / 200.0,
            summary,
            evidence: format!(
                "{}: physical extent {} lines (last symbol end) >= {} god-file threshold",
                cf.path, extent, OVERSIZED_FILE_LINES
            ),
            remedy: format!(
                "split {path} along intent/module lines so each new file owns one responsibility (a code split is a redesign — propose it: `loom hypothesis add --name \"split {path}\" --claim \"<why this file is too big>\" --proposal \"<the split>\" --predicted-outcome \"<measurable result, e.g. each split file owns one responsibility and this oversized_file finding clears>\" --target <owning intent>`); rule it deliberate ONLY after reading the file (one protocol, one generated block, one truly cohesive module): `loom note add --smell \"oversized_file:{path}\" --kind decision --text \"<the split you considered and the concrete reason it is wrong for THIS file — NOT 'size reflects N items', which restates the size>\"` resolves THIS finding (editing the file re-opens it). loom rejects a vacuous or templated ruling — audit each file on its own contents. A per-symbol `large_behavioral_symbol` ruling does NOT clear this — it is keyed on the file, not a symbol.",
                path = cf.path
            ),
            teaching: teaching_for("oversized_file"),
        });
    }
}

/// 4e. Repeated string contracts — long user-facing/help/error/example strings
/// copied across symbols can drift silently. Conservative: ignores short
/// labels, path-like values, and tests.
fn detect_string_contract_duplicate(
    snapshot: &QuerySnapshot,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let mut strings: HashMap<String, Vec<StringContractLoc<'_>>> = HashMap::new();
    for cf in &snapshot.codefiles {
        for f in &cf.symbol_facts {
            if f.is_test {
                continue;
            }
            for literal in &f.string_literals {
                let Some(key) = normalized_contract_string(&literal.value) else {
                    continue;
                };
                strings.entry(key).or_default().push(StringContractLoc {
                    path: cf.path.as_str(),
                    file_modified: cf.last_modified.as_str(),
                    label: f.label.as_str(),
                    line: literal.line,
                    value: literal.value.as_str(),
                });
            }
        }
    }
    let deletion_ctx = DeletionContext::new(snapshot);
    for (_key, mut locs) in strings {
        locs.sort_by(|a, b| {
            a.path
                .cmp(b.path)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.label.cmp(b.label))
        });
        locs.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.label == b.label);
        let distinct_files = locs.iter().map(|l| l.path).collect::<HashSet<_>>().len();
        let distinct_symbols = locs
            .iter()
            .map(|l| (l.path, l.label))
            .collect::<HashSet<_>>()
            .len();
        if distinct_files < 2 && distinct_symbols < 2 {
            continue;
        }
        let anchor = locs[0];
        let newest = locs
            .iter()
            .map(|l| l.file_modified)
            .max()
            .unwrap_or(anchor.file_modified);
        let excerpt = short_contract_excerpt(anchor.value);
        let summary = format!(
            "string contract repeated in {} location(s): \"{}\"",
            locs.len(),
            excerpt
        );
        if let Some(note) = adjudicate(
            last_decision,
            "string_contract_duplicate",
            anchor.path,
            newest,
        ) {
            adjudicated_out.push(AdjudicatedSmell {
                kind: "string_contract_duplicate".into(),
                summary,
                ruling: note.text.clone(),
                ruled_by: note.author.clone(),
                ruled_at: note.created_at.clone(),
                reopens_when: "one of the files carrying the repeated string changes".into(),
                teaching: teaching_for("string_contract_duplicate"),
            });
            continue;
        }
        let evidence = locs
            .iter()
            .take(8)
            .map(|l| format!("{}:{} '{}'", l.path, l.line, l.label))
            .collect::<Vec<_>>()
            .join(" · ");
        let intent_clause = deletion_ctx.clause(locs.iter().map(|l| {
            (
                l.path,
                l.label,
                crate::db::queries::symbol_match::symbol_identifier(l.label),
            )
        }));
        smells.push(Smell {
            kind: "string_contract_duplicate".into(),
            score: locs.len() as f64 * (anchor.value.len() as f64 / 40.0).max(1.0),
            summary,
            evidence: format!(
                "normalized repeated text appears in {} symbol(s) across {} file(s): {} | {}",
                distinct_symbols, distinct_files, evidence, intent_clause
            ),
            remedy: format!(
                "{STRING_CONTRACT_SAFETY_PREAMBLE}inspect the repeated text; extract one source of truth if the wording must change together, or rule the copies independent ONLY after reading both: `loom note add --smell \"string_contract_duplicate:{}\" --kind decision --text \"<the contract each copy serves and why they must evolve apart — NOT 'intentional', which restates the finding>\"` resolves this finding (editing any carrying file re-opens it). loom rejects a vacuous or templated ruling — audit each pair on its own text",
                anchor.path
            ),
            teaching: teaching_for("string_contract_duplicate"),
        });
    }
}
