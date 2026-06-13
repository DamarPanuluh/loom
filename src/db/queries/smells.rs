//! Derived problem signals (`loom smells`) — the graph as an *instrument*,
//! not just a ledger.
//!
//! Everything else in loom records what an agent told it; this module computes
//! what nobody noticed: duplicate responsibility (split-brain), overlapping
//! ownership, fragmentation, files doing too much, and normative-plane gaps
//! (a QualityRule exists but was never held against an intent that has real
//! code — the measuring stick lying unused next to the thing it should
//! measure). Pure graph computation — no LLM judgment in the *flagging*.
//!
//! `loom smells` returns suspicions, but they are not free-floating advice:
//! OPEN findings gate green (`graph_state` routes phase=audit until zero
//! remain). The escape from a false positive is never gaming a threshold —
//! each kind has an explicit adjudication path (an `independent` verdict, a
//! merge, a decision note newer than the structure it judges), so the verdict
//! on each finding stays with the inspecting agent, via the exact remedy
//! command each smell carries.

use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use crate::db::LoomDb;
use super::snapshot::{DiscoverySnapshot, QuerySnapshot};
// Thresholds — deliberately conservative: a smell should be worth a look.
/// Name+description token overlap at/above this is a twin-intent suspicion.
pub const TWIN_SIMILARITY: f64 = 0.4;
/// Rarity-weighted shared-tag weight at/above this is a duplicated-responsibility
/// suspicion: one near-unique shared term (2 carriers → 0.5) or several
/// moderately rare ones. Broad spammed terms decay toward zero on their own.
pub const DUP_TAG_WEIGHT: f64 = 0.5;
/// Untagged fallback: when bounded vocabulary is absent on either side, strong
/// description overlap can still flag disjoint coded responsibility.
pub const DUP_UNTAGGED_SIMILARITY: f64 = 0.30;
/// Minimum shared tokens for the untagged fallback. This keeps one generic word
/// from standing in for the missing vocabulary facet.
pub const DUP_UNTAGGED_SHARED_TOKENS: usize = 2;
/// Proposed hypotheses are useful pressure, but a large queue means the
/// pre-decision plane is becoming a note dump instead of a proof pipeline.
pub const HYPOTHESIS_BACKLOG_LIMIT: usize = 10;
/// A proposed hypothesis older than this is stale enough to surface even when
/// the queue is small. Non-RFC3339 timestamps (old tests/imports) are ignored.
pub const HYPOTHESIS_STALE_DAYS: i64 = 14;
/// Scatter thresholds are level-aware: a feature should be cohesive (few
/// files); a component legitimately spans a directory; a system intent
/// grounds to manifests and is never "scattered".
pub fn scatter_threshold(level: &str) -> Option<usize> {
    match level {
        "feature" => Some(4),
        "component" | "cross_cutting" => Some(10),
        _ => None, // system
    }
}
/// A file implemented by this many intents or more is tangled.
pub const TANGLE_INTENTS: usize = 3;

/// One derived finding, with the exact remedy that resolves it.
#[derive(Debug, Clone, Serialize)]
pub struct Smell {
    /// twin_intents | duplicated_responsibility | overlapping_ownership
    /// | scattered_intent | tangled_file | unmeasured_intents
    /// | undeclared_coupling | layering_violation | recurrent_trouble
    /// | unused_rule | happy_path_only | vocab_drift | duplicate_detection_unarmed
    /// | hypothesis_accumulation | symbol_accountability_gap
    pub kind: String,
    /// Higher = look first (kind-relative magnitude).
    pub score: f64,
    /// One line: what looks wrong.
    pub summary: String,
    /// The computed numbers/names behind the suspicion.
    pub evidence: String,
    /// The exact command sequence that resolves or refutes it.
    pub remedy: String,
    /// LLM-facing teaching: why this smell matters, what to inspect, what to
    /// avoid, and what "done" means.
    pub teaching: SmellTeaching,
}

/// A finding the detector WOULD raise, suppressed by a recorded ruling — the
/// audit trail `loom smells` shows instead of silently reporting zero. The
/// dogfood lesson: five godfile findings vanished behind five decision notes
/// stamped in one second, and nothing in any output said so; "no findings"
/// and "five findings, all ruled deliberate" must never look alike.
#[derive(Debug, Clone, Serialize)]
pub struct AdjudicatedSmell {
    pub kind: String,
    /// What would have been flagged.
    pub summary: String,
    /// The decision note's text — the recorded "looked at it, here's the call".
    pub ruling: String,
    pub ruled_by: String,
    pub ruled_at: String,
    /// The structural change that voids this ruling and re-opens the finding.
    pub reopens_when: String,
    /// The same LLM-facing lesson as the open finding would carry.
    pub teaching: SmellTeaching,
}

#[derive(Debug, Clone, Serialize)]
pub struct SmellTeaching {
    pub principle: String,
    pub inspect: Vec<String>,
    pub avoid: Vec<String>,
    pub done_when: String,
}

/// What the instrument actually measured: open suspicions AND the suppressed
/// ones with their rulings. Phase gating consumes `open`; the audit surface
/// (`loom smells`) shows both.
#[derive(Debug, Clone, Serialize)]
pub struct SmellReport {
    pub open: Vec<Smell>,
    pub adjudicated: Vec<AdjudicatedSmell>,
    /// Coverage disclosure for `duplicated_responsibility`: tag collisions are
    /// the strongest signal, and untagged coded intents fall back to a weaker
    /// lexical detector. `coded_intents` counts active intents with ≥1
    /// IMPLEMENTS edge; `tagged_coded_intents` how many of those carry ≥1 tag.
    pub coded_intents: usize,
    pub tagged_coded_intents: usize,
    /// Blind-spot disclosure for `layering_violation`: the detector judges
    /// imports against the DECLARED order only. `coded_layers` counts the
    /// distinct non-empty layers across coded intents; `declared_layers` the
    /// length of `loom layer order`. Layers in use with no declared order
    /// = the layering instrument is unarmed, and the report must say so.
    pub coded_layers: usize,
    pub declared_layers: usize,
}

fn teaching_for(kind: &str) -> SmellTeaching {
    match kind {
        "twin_intents" => SmellTeaching {
            principle: "Similar wording is a suspicion, not proof; inspect both meanings and code before merging or declaring independence.".into(),
            inspect: vec![
                "read both intent criteria and descriptions".into(),
                "inspect each intent's groundings before recording the edge verdict".into(),
                "use `loom edge explore <a> <b>` only after evidence is checked".into(),
            ],
            avoid: vec!["do not merge or mark independent from name similarity alone".into()],
            done_when: "a RELATES_TO verdict explains the real relationship, or a proven merge hypothesis replaces one responsibility with one intent".into(),
        },
        "duplicated_responsibility" => SmellTeaching {
            principle: "Duplicate responsibility hides when unrelated files implement the same idea; tags and lexical fallback only point to where inspection must happen.".into(),
            inspect: vec![
                "compare both intents' criteria, tags, and grounded code".into(),
                "check whether the two implementations should share one owner or remain explicitly independent".into(),
                "record the result with `loom edge explore <a> <b>`".into(),
            ],
            avoid: vec!["do not treat a tag or token collision as proof without reading the code".into()],
            done_when: "the pair has a grounded/independent relationship, or a proven merge hypothesis removes the duplicated ownership".into(),
        },
        "duplicate_detection_unarmed" => SmellTeaching {
            principle: "A quiet duplicate audit is weak when coded intents lack registered vocabulary tags; the lexical fallback is not equivalent to bounded terms.".into(),
            inspect: vec![
                "`loom vocab list`".into(),
                "review untagged coded intents and assign precise registered terms".into(),
                "`loom smells` after tagging to re-run duplicate detection".into(),
            ],
            avoid: vec!["do not accept a no-duplicate result while most coded intents are untagged".into()],
            done_when: "coded intents are tagged enough for duplicate detection, or a root decision records why the remaining blind spot is accepted".into(),
        },
        "overlapping_ownership" => SmellTeaching {
            principle: "Two intents claiming the same file need an explicit ownership contract; shared code is physical evidence of a relationship.".into(),
            inspect: vec![
                "`loom codefile show <path>` for the shared file".into(),
                "read the shared file once and decide what each intent owns".into(),
                "`loom edge explore <a> <b>` to ground or refute the relationship".into(),
            ],
            avoid: vec!["do not leave shared-file ownership implicit".into()],
            done_when: "the intents have a grounded relationship, an independent verdict, or one grounding is moved to the correct owner".into(),
        },
        "scattered_intent" => SmellTeaching {
            principle: "A scattered intent usually means the graph intent is too broad; split intent meaning before proposing code movement.".into(),
            inspect: vec![
                "read the directory clusters in the evidence".into(),
                "`loom intent show <intent>` to inspect all groundings".into(),
                "look for cohesive child responsibilities along the file clusters".into(),
            ],
            avoid: vec!["do not start a code refactor before proving the graph split or design problem".into()],
            done_when: "groundings are moved to cohesive child intents, or a newer decision explains why this spread is deliberate".into(),
        },
        "tangled_file" => SmellTeaching {
            principle: "A tangled file may be deliberate coordination or a real split candidate; code splitting is redesign work and should be proven first.".into(),
            inspect: vec![
                "`loom codefile show <path>`".into(),
                "read the listed intent owners and the shared transaction/module boundary".into(),
                "if splitting is needed, propose it through `loom hypothesis add`".into(),
            ],
            avoid: vec!["do not split a coordinator file just to silence the smell".into()],
            done_when: "cohabitation has a current decision note, or an adopted/proven hypothesis restructures ownership".into(),
        },
        "unmeasured_intents" => SmellTeaching {
            principle: "A quality rule only matters where it has been honestly held against coded behavior; independent is a valid measured result.".into(),
            inspect: vec![
                "`loom next --mode quality`".into(),
                "measure at the highest honest component altitude before dropping to leaves".into(),
                "record passing, failing, or independent with concrete evidence".into(),
            ],
            avoid: vec!["do not stamp broad rules across leaves with vacuous evidence".into()],
            done_when: "the rule has GOVERNS verdicts directly or via honest ancestor coverage for every coded intent it should measure".into(),
        },
        "undeclared_coupling" => SmellTeaching {
            principle: "Static imports are executable evidence that two owned responsibilities touch; the semantic graph must either declare or remove that coupling.".into(),
            inspect: vec![
                "read the importing and imported files named in evidence".into(),
                "inspect the two owning intents' criteria".into(),
                "`loom edge explore <a> <b>` to ground the contract or record the issue".into(),
            ],
            avoid: vec!["do not add a relationship without naming the actual call/import contract".into()],
            done_when: "the coupling is grounded with evidence, marked as an issue to untangle, or the import is removed".into(),
        },
        "layering_violation" => SmellTeaching {
            principle: "A recorded relationship does not excuse dependency direction; layer order judges whether imports point the right way.".into(),
            inspect: vec![
                "`loom layer list`".into(),
                "read the upward import named in evidence".into(),
                "decide whether to invert, extract lower shared code, redeclare layers, or record a deliberate exception".into(),
            ],
            avoid: vec!["do not silence an up-dependency by adding RELATES_TO; direction is a separate norm".into()],
            done_when: "the dependency points down, the layer order is corrected, or a current decision on the importing intent justifies the exception".into(),
        },
        "recurrent_trouble" => recurrent_teaching("edge", "<id>"),
        "happy_path_only" => SmellTeaching {
            principle: "Failure and degradation behavior are real only when realized, grounded, and proven; naming sad/fallback children is not enough.".into(),
            inspect: vec![
                "inspect the parent's aspect-tagged children".into(),
                "check lifecycle, IMPLEMENTS groundings, and passed validations for sad/fallback paths".into(),
                "add or prove the missing path, or record why it is not applicable".into(),
            ],
            avoid: vec!["do not clear failure-path debt with planned or unproven child intents".into()],
            done_when: "sad and fallback paths are implemented, grounded, and directly proven, or a current decision explains why they are not required".into(),
        },
        "unused_rule" => SmellTeaching {
            principle: "A rule connected to nothing is not a quality bar; it is dormant policy text.".into(),
            inspect: vec![
                "`loom rule list`".into(),
                "find the highest honest intent surface the rule should govern".into(),
                "apply it with `loom rule verdict` or delete it if it was a mistake".into(),
            ],
            avoid: vec!["do not keep unused rules as implied standards".into()],
            done_when: "the rule governs at least one relevant intent, or it is removed as unused policy".into(),
        },
        "vocab_drift" => SmellTeaching {
            principle: "Near-synonym vocabulary terms split the collision signal that duplicate detection depends on.".into(),
            inspect: vec![
                "`loom vocab list`".into(),
                "compare the two term definitions and their tagged intents".into(),
                "merge synonyms or rename/retag to make the distinction sharp".into(),
            ],
            avoid: vec!["do not let agents choose between look-alike terms for the same concept".into()],
            done_when: "the look-alike terms are merged, or the remaining terms have names and definitions that no longer collide".into(),
        },
        "unjourneyed_surface" => SmellTeaching {
            principle: "User-visible code needs passed consumer-journey proof; per-leaf tests and declared-but-unrun sagas do not prove the composed experience.".into(),
            inspect: vec![
                "`loom saga list`".into(),
                "inspect whether a passed saga step binds to this intent or its relevant tree path".into(),
                "add and pass a saga, mark visibility internal, or record why no journey can exercise it".into(),
            ],
            avoid: vec!["do not treat user_visible as proven by unit coverage alone".into()],
            done_when: "a passed consumer saga covers the surface through the tree, or a current ruling explains why it is not consumer-reachable".into(),
        },
        "hypothesis_accumulation" => SmellTeaching {
            principle: "A hypothesis is a falsifiable proof item, not long-term memory; accumulation teaches the next LLM to prove, reject, or adopt instead of stockpiling ideas.".into(),
            inspect: vec![
                "`loom next --mode prove` for the highest-blast-radius proposal".into(),
                "`loom hypothesis list --status proposed` to batch the backlog".into(),
                "target untargeted hypotheses before proving, or reject them if they are not actionable".into(),
            ],
            avoid: vec![
                "do not add another hypothesis when existing proposals are unproven".into(),
                "do not convert speculative notes into planned work before proof".into(),
            ],
            done_when: "the proposed backlog is below the threshold and no proposed hypothesis is stale; each old idea is supported, refuted, adopted, or rejected with evidence".into(),
        },
        "symbol_accountability_gap" => SmellTeaching {
            principle: "Behavior-significant symbols should be owned, accepted, or turned into explicit work; raw helper coverage is not the target.".into(),
            inspect: vec![
                "`loom coverage --json` and read actionable_symbol_gaps".into(),
                "`loom codefile show <path>` for each top gap before changing graph ownership".into(),
                "decide whether the symbol needs a precise locator, a split intent, or a decision note accepting broad ownership".into(),
            ],
            avoid: vec![
                "do not chase 100% raw symbol coverage".into(),
                "do not create intents for every private helper".into(),
                "do not bulk-ground symbols without checking intent meaning".into(),
            ],
            done_when: "actionable symbol gaps are grounded with precise locators, accepted with a current decision note, or converted into real intent split/build work".into(),
        },
        _ => SmellTeaching {
            principle: "This smell is a computed suspicion; inspect the named graph and code evidence before changing behavior.".into(),
            inspect: vec!["read the evidence and run the remedy command with concrete evidence".into()],
            avoid: vec!["do not silence the finding without a structural fix or decision note".into()],
            done_when: "the finding is fixed or adjudicated through its remedy".into(),
        },
    }
}

fn recurrent_teaching(target_kind: &str, target_id: &str) -> SmellTeaching {
    let selector = if target_kind == "intent" {
        format!("--intent {target_id}")
    } else {
        format!("--edge {target_id}")
    };
    SmellTeaching {
        principle: "Repeated failing/needs_change transitions mean the criterion, design boundary, or ownership model is wrong; patching again is suspect.".into(),
        inspect: vec![
            format!("loom note list {selector} --kind transition --limit 0"),
            "read the last failed criterion/evidence for the same target".into(),
            "identify the stable root cause before proposing another fix".into(),
        ],
        avoid: vec!["do not apply another narrow patch without a hypothesis explaining why recurrence will stop".into()],
        done_when: "a proven/adopted redesign or decision note newer than the last regression explains why the target will not keep regressing".into(),
    }
}

/// Jaccard similarity of two token sets (0.0 when either is empty).
pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    inter / union
}

/// Compute every OPEN smell, sorted by score (descending) within insertion
/// order of kind. Callers truncate for display.
///
/// "Open" is the operative word: a finding that was adjudicated — structurally
/// resolved, or refuted through its remedy (an `independent` verdict, a vocab
/// merge, a decision note newer than the structure it judges) — is not
/// returned. That is what lets `graph_state` gate phase=complete on zero
/// findings without inviting threshold-gaming: green means every suspicion
/// was ANSWERED, not that every heuristic is happy.
pub fn compute_smells(db: &dyn LoomDb) -> Result<SmellReport> {
    let snapshot = QuerySnapshot::load(db)?;
    compute_smells_from(db, &snapshot)
}

/// Snapshot-reusing form for callers that already hold one (`graph_state`,
/// `loom next --all`). `db` is still needed for notes and the vocab registry.
pub fn compute_smells_from(db: &dyn LoomDb, snapshot: &QuerySnapshot) -> Result<SmellReport> {
    let discovery = DiscoverySnapshot::from_query(snapshot)?;
    let intents = &snapshot.intents;
    let implements = &snapshot.implements;
    let hierarchy = &snapshot.hierarchy;
    let relates = &snapshot.relates;
    let rules = &snapshot.rules;
    let governs = &snapshot.governs;

    // Lookup structures.
    let linked: HashSet<(&str, &str)> = discovery
        .linked
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let mut files_of: HashMap<&str, HashSet<&str>> = HashMap::new();
    let intents_on_file: HashMap<&str, Vec<&str>> = discovery
        .intents_on_file
        .iter()
        .map(|(path, ids)| (path.as_str(), ids.iter().map(|id| id.as_str()).collect()))
        .collect();
    for im in implements {
        files_of
            .entry(im.intent_id.as_str())
            .or_default()
            .insert(im.codefile_path.as_str());
    }
    let name_of: HashMap<&str, &str> = intents
        .iter()
        .map(|i| (i.id.as_str(), i.name.as_str()))
        .collect();
    let toks: HashMap<&str, HashSet<String>> = discovery
        .tokens_by_intent
        .iter()
        .map(|(id, toks)| (id.as_str(), toks.clone()))
        .collect();

    // The adjudication ledger: every note, loaded once. `last_decision` maps a
    // target id to its newest kind=decision note — the recorded "looked at it,
    // here's the call" that resolves a structural finding until the structure
    // changes again underneath it (recurrent_trouble set the pattern).
    let all_notes = snapshot.notes(db)?;
    let mut last_decision: HashMap<&str, &crate::types::Note> = HashMap::new();
    for n in all_notes {
        if n.kind == "decision" && !n.target_id.is_empty() {
            let e = last_decision.entry(n.target_id.as_str()).or_insert(n);
            if n.created_at > e.created_at {
                *e = n;
            }
        }
    }
    // Staleness anchors for those decisions: the newest grounding per intent
    // and per file. A decision older than the newest grounding judged a
    // different structure — the finding re-opens. ("" on pre-v3 edges sorts
    // before any timestamp, so old graphs are grandfathered until regrounded.)
    let mut newest_grounding: HashMap<&str, &str> = HashMap::new();
    let mut newest_claim: HashMap<&str, &str> = HashMap::new();
    for im in implements {
        let g = newest_grounding.entry(im.intent_id.as_str()).or_default();
        if im.created_at.as_str() > *g {
            *g = &im.created_at;
        }
        let c = newest_claim.entry(im.codefile_path.as_str()).or_default();
        if im.created_at.as_str() > *c {
            *c = &im.created_at;
        }
    }
    let cf_id_of: HashMap<&str, &str> = snapshot
        .codefiles
        .iter()
        .map(|cf| (cf.path.as_str(), cf.id.as_str()))
        .collect();
    let is_child: HashSet<&str> = hierarchy.iter().map(|(_, c)| c.as_str()).collect();
    let mut roots: Vec<&crate::types::Intent> = intents
        .iter()
        .filter(|i| i.status != "deprecated" && !is_child.contains(i.id.as_str()))
        .collect();
    roots.sort_by_key(|i| (i.abstraction_level != "system", i.name.clone()));
    // Adjudicated: a decision on `target` newer than the structure's newest
    // change at `anchor` ("" = no structural timestamp recorded). Returns the
    // ruling note — suppressed findings surface WITH their ruling, never
    // silently.
    let adjudicated = |target: &str, anchor: &str| -> Option<&crate::types::Note> {
        last_decision
            .get(target)
            .filter(|n| n.created_at.as_str() > anchor)
            .copied()
    };

    let mut smells: Vec<Smell> = Vec::new();
    let mut adjudicated_out: Vec<AdjudicatedSmell> = Vec::new();

    // 1. Twin intents — split-brain in the semantic plane: two intents at the
    //    same abstraction level that read like the same responsibility, with
    //    no recorded relationship between them.
    for i in 0..intents.len() {
        for j in (i + 1)..intents.len() {
            let (a, b) = (&intents[i], &intents[j]);
            if a.abstraction_level != b.abstraction_level
                || a.status == "deprecated"
                || b.status == "deprecated"
                || linked.contains(&(a.id.as_str(), b.id.as_str()))
            {
                continue;
            }
            let sim = jaccard(&toks[a.id.as_str()], &toks[b.id.as_str()]);
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

    // 1b. Duplicated responsibility — the collision detector the bounded tag
    //     vocabulary exists for: two same-level intents whose REGISTERED tags
    //     collide (rarity-weighted), grounded in DISJOINT files with no import
    //     between them and no recorded relationship. Exactly the case every
    //     other detector misses: lexical twins need shared wording,
    //     overlapping_ownership needs a shared file, undeclared_coupling needs
    //     an import — same responsibility implemented twice in unrelated code
    //     has none of those. Tags remain the strongest signal, but an untagged
    //     coded pair gets a stricter lexical fallback so missing tags do not
    //     make the detector entirely blind.
    for i in 0..intents.len() {
        for j in (i + 1)..intents.len() {
            let (a, b) = (&intents[i], &intents[j]);
            if a.abstraction_level != b.abstraction_level
                || linked.contains(&(a.id.as_str(), b.id.as_str()))
            {
                continue;
            }
            // Physical separation: no shared file, no import between their code.
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
                super::vocab::shared_tag_weight(ta, tb, &discovery.tag_counts);
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
                let shared_tokens: Vec<String> = toks[a.id.as_str()]
                    .intersection(&toks[b.id.as_str()])
                    .cloned()
                    .collect();
                let sim = jaccard(&toks[a.id.as_str()], &toks[b.id.as_str()]);
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

    // 1c. Duplicate detector coverage — tags are still optional at write time,
    //     but once an intent has code, untagged coverage is audit-relevant. The
    //     lexical fallback above is deliberately weaker than bounded vocabulary;
    //     this aggregate finding keeps "fallback only" from looking as strong as
    //     registered terms. A root decision may consciously accept the remaining
    //     blind spot, but a newly grounded untagged coded intent re-opens it.
    {
        let coded: Vec<&crate::types::Intent> = intents
            .iter()
            .filter(|i| files_of.contains_key(i.id.as_str()))
            .collect();
        if coded.len() >= 2 {
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
            if !untagged.is_empty() {
                let registry = super::vocab::list_vocab_terms(db)?.len();
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
                let adjudicated_note = roots
                    .first()
                    .and_then(|root| adjudicated(root.id.as_str(), newest_untagged_grounding));
                if let Some(note) = adjudicated_note {
                    adjudicated_out.push(AdjudicatedSmell {
                        kind: "duplicate_detection_unarmed".into(),
                        summary,
                        ruling: note.text.clone(),
                        ruled_by: note.author.clone(),
                        ruled_at: note.created_at.clone(),
                        reopens_when:
                            "a new or newly grounded untagged coded intent lands after the ruling"
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
                            "seed the bounded vocabulary (`loom vocab add <term> --why \"covers X, not Y\"`), then tag coded intents with `loom intent tag add <intent> <term>`; if the remaining blind spot is deliberate, record it on the graph root with `loom note add --intent <root> --kind decision --text \"<why untagged coded intents are acceptable>\"`".into()
                        } else {
                            "tag the untagged coded intents from the registered vocabulary (`loom vocab list`, then `loom intent tag add <intent> <term>`); if the remaining blind spot is deliberate, record it on the graph root with `loom note add --intent <root> --kind decision --text \"<why untagged coded intents are acceptable>\"`".into()
                        },
                        teaching: teaching_for("duplicate_detection_unarmed"),
                    });
                }
            }
        }
    }

    // 2. Overlapping ownership — split-brain in the physical plane: two
    //    intents grounded in the same file with no recorded relationship.
    //    (Parent/child sharing a file is structure, not a smell — `linked`
    //    covers HIERARCHY too.)
    for i in 0..intents.len() {
        for j in (i + 1)..intents.len() {
            let (a, b) = (&intents[i], &intents[j]);
            if linked.contains(&(a.id.as_str(), b.id.as_str())) {
                continue;
            }
            let (Some(fa), Some(fb)) = (files_of.get(a.id.as_str()), files_of.get(b.id.as_str()))
            else {
                continue;
            };
            let shared: Vec<&&str> = fa.intersection(fb).collect();
            if !shared.is_empty() {
                let mut names: Vec<String> = shared.iter().map(|s| s.to_string()).collect();
                names.sort();
                smells.push(Smell {
                    kind: "overlapping_ownership".into(),
                    score: 3.0 * shared.len() as f64,
                    summary: format!(
                        "'{}' and '{}' both claim {} file(s) but no relationship is recorded",
                        a.name, b.name, shared.len()
                    ),
                    evidence: format!("shared: {}", names.join(", ")),
                    remedy: format!(
                        "loom edge explore {} {}  → who owns what? ground the contract or mark independent with why",
                        a.id, b.id
                    ),
                    teaching: teaching_for("overlapping_ownership"),
                });
            }
        }
    }

    // 3. Scattered intent — one responsibility smeared across many files
    //    (threshold scales with abstraction level).
    for i in intents {
        let (Some(files), Some(threshold)) = (
            files_of.get(i.id.as_str()),
            scatter_threshold(&i.abstraction_level),
        ) else {
            continue;
        };
        if files.len() >= threshold {
            // Adjudicated: a decision note on the intent newer than its newest
            // grounding says the spread is deliberate. A grounding added after
            // the decision re-opens the question.
            if let Some(note) = adjudicated(
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
            // Group the grounded files by directory — the mechanical clustering
            // evidence for a split. The flagging stays judgment-free: loom shows
            // where the files cluster; the driving LLM names the child intents.
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
            let clusters = dirs
                .iter()
                .map(|(d, n)| format!("{d} ({n})"))
                .collect::<Vec<_>>()
                .join(" · ");
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
                    "split the INTENT, not the code (a too-coarse seed is normal): add a child intent per cohesive slice along the directory clusters, `loom edge hierarchy {id} <child>`, then move groundings down (`loom edge unimplement {id} '<dir>/**'` + `loom edge implement <child> …`); if the CODE itself is the problem, propose that separately: `loom hypothesis add … --claim \"<why this layout fights the design>\" --target {id}`; if the spread is DELIBERATE, record the call: `loom note add --intent {id} --kind decision --text \"<why this layout is right>\"` resolves this finding (a new grounding re-opens it)",
                    id = i.id
                ),
                teaching: teaching_for("scattered_intent"),
            });
        }
    }

    // 4. Tangled file — one file serving many intents (this is `loom hotspots`
    //    made actionable with a threshold + remedy).
    for (path, iids) in &intents_on_file {
        let distinct: HashSet<&&str> = iids.iter().collect();
        if distinct.len() >= TANGLE_INTENTS {
            // Adjudicated: a decision note on the FILE newer than its newest
            // claim ("loom note add --file … --kind decision") says the
            // cohabitation is deliberate. A new claim re-opens it.
            if let Some(note) = adjudicated(
                cf_id_of.get(path).copied().unwrap_or(""),
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
                evidence: format!("intents: {}", names.join(" · ")),
                remedy: format!(
                    "a code split is a redesign — propose it so it gets proven before it becomes work: `loom hypothesis add --name \"split {path}\" --claim \"{path} serves {n} unrelated intents\" --proposal \"<the split, along intent lines>\" --predicted-outcome \"each intent grounds in its own module; this finding disappears\"` with a --target per owning intent; if the cohabitation is DELIBERATE, record it: `loom note add --file {path} --kind decision --text \"<why these intents share a home>\"` resolves this finding (a new claim re-opens it)",
                    n = distinct.len(),
                ),
                teaching: teaching_for("tangled_file"),
            });
        }
    }

    // 5. The measuring stick, unused — the normative plane only measures where
    //    someone thought to apply a rule. Surface every rule × intent-with-code
    //    pairing that was never considered (no GOVERNS edge of ANY state —
    //    `independent` records "considered, doesn't apply" and silences this).
    //
    //    HIERARCHY-AWARE: a verdict INHERITS DOWN the tree. A rule held against
    //    a component covers that component's descendants (a child can still get
    //    its own, more specific edge). Measuring at the highest altitude where
    //    the evidence is honest is the *encouraged* strategy — without
    //    inheritance this smell punished it by re-flagging every leaf, inviting
    //    a busywork sweep of vacuous per-leaf verdicts.
    let considered: HashSet<(&str, &str)> = governs
        .iter()
        .map(|g| (g.rule_id.as_str(), g.intent_id.as_str()))
        .collect();
    let parent_of: HashMap<&str, &str> = hierarchy
        .iter()
        .map(|(p, c)| (c.as_str(), p.as_str()))
        .collect();
    // Considered directly OR via any ancestor's verdict on the same rule.
    // The tree is insert-enforced acyclic; the visited set is belt-and-braces.
    let considered_up = |rule_id: &str, intent_id: &str| -> bool {
        let mut cur = Some(intent_id);
        let mut visited: HashSet<&str> = HashSet::new();
        while let Some(id) = cur {
            if !visited.insert(id) {
                return false;
            }
            if considered.contains(&(rule_id, id)) {
                return true;
            }
            cur = parent_of.get(id).copied();
        }
        false
    };
    for r in rules {
        let unmeasured: Vec<&crate::types::Intent> = intents
            .iter()
            .filter(|i| {
                i.status != "deprecated"
                    && files_of.contains_key(i.id.as_str()) // has real code to measure
                    && !considered_up(&r.id, &i.id)
            })
            .collect();
        if unmeasured.is_empty() {
            continue;
        }
        let sample: Vec<String> = unmeasured
            .iter()
            .take(3)
            .map(|i| format!("{} ({})", i.name, i.id))
            .collect();
        smells.push(Smell {
            kind: "unmeasured_intents".into(),
            score: unmeasured.len() as f64,
            summary: format!(
                "rule '{}' has never been held against {} intent(s) that have code (neither directly nor via an ancestor's verdict)",
                r.name,
                unmeasured.len()
            ),
            evidence: format!("e.g. {}", sample.join(" · ")),
            remedy: format!(
                "measure at the highest HONEST altitude: loom rule verdict {} <component> --status passing|failing|independent covers the component's descendants too (independent = measured, rule doesn't apply); drop to a leaf only where the rule has specific bite",
                r.id
            ),
            teaching: teaching_for("unmeasured_intents"),
        });
    }

    // 6. Undeclared coupling — the physical plane contradicts the semantic:
    //    file A statically imports file B, but the intents owning A and B have
    //    no recorded relationship. The strongest split-brain detector loom has,
    //    because it is grounded in the code itself, not in testimony.
    {
        let mut pair_files: HashMap<(String, String), Vec<String>> = HashMap::new();
        for cf in &snapshot.codefiles {
            let Some(owners_a) = intents_on_file.get(cf.path.as_str()) else {
                continue;
            };
            for target in &cf.imports {
                let Some(owners_b) = intents_on_file.get(target.as_str()) else {
                    continue;
                };
                for a in owners_a {
                    for b in owners_b {
                        if a == b || linked.contains(&(*a, *b)) {
                            continue;
                        }
                        let key = if a < b {
                            (a.to_string(), b.to_string())
                        } else {
                            (b.to_string(), a.to_string())
                        };
                        let example = format!("{} → {}", cf.path, target);
                        let entry = pair_files.entry(key).or_default();
                        if !entry.contains(&example) {
                            entry.push(example);
                        }
                    }
                }
            }
        }
        for ((a, b), examples) in pair_files {
            let (na, nb) = (
                name_of.get(a.as_str()).copied().unwrap_or(&a),
                name_of.get(b.as_str()).copied().unwrap_or(&b),
            );
            smells.push(Smell {
                kind: "undeclared_coupling".into(),
                score: 4.0 + examples.len() as f64,
                summary: format!(
                    "code of '{}' imports code of '{}' but no relationship is recorded",
                    na, nb
                ),
                evidence: format!("imports: {}", examples.join(", ")),
                remedy: format!(
                    "loom edge explore {} {}  → the code says they touch; ground the contract (or untangle the import)",
                    a, b
                ),
                teaching: teaching_for("undeclared_coupling"),
            });
        }
    }

    // 6b. Layering violation — the declared order judging the import graph:
    //     code owned by a LOWER layer imports code owned by a HIGHER layer.
    //     Direction always existed in the physical plane (imports are
    //     directed); what was missing is the normative input — a violation
    //     only exists relative to a DECLARED order (`loom layer order`,
    //     top layer first; undeclared layers are exempt). Crucially, a
    //     recorded RELATES_TO edge does NOT excuse direction: undeclared_
    //     coupling asks "is the contact declared?", this asks "does the
    //     dependency point the right way?" — independent questions.
    {
        let layer_rank: HashMap<String, usize> = super::meta::get_layer_order(db)?
            .into_iter()
            .enumerate()
            .map(|(rank, layer)| (layer, rank))
            .collect();
        if !layer_rank.is_empty() {
            let layer_of: HashMap<&str, &str> = intents
                .iter()
                .map(|i| (i.id.as_str(), i.layer.as_str()))
                .collect();
            // Ordered pair (lower importer, higher imported) → example imports.
            let mut pair_files: HashMap<(String, String), Vec<String>> = HashMap::new();
            for cf in &snapshot.codefiles {
                let Some(owners_a) = intents_on_file.get(cf.path.as_str()) else {
                    continue;
                };
                for target in &cf.imports {
                    let Some(owners_b) = intents_on_file.get(target.as_str()) else {
                        continue;
                    };
                    for a in owners_a {
                        for b in owners_b {
                            let (Some(&ra), Some(&rb)) = (
                                layer_of.get(*a).and_then(|d| layer_rank.get(*d)),
                                layer_of.get(*b).and_then(|d| layer_rank.get(*d)),
                            ) else {
                                continue; // undeclared layer — exempt
                            };
                            // Bigger rank = deeper layer; flag deep → shallow.
                            if a == b || ra <= rb {
                                continue;
                            }
                            let example = format!("{} → {}", cf.path, target);
                            let entry = pair_files
                                .entry((a.to_string(), b.to_string()))
                                .or_default();
                            if !entry.contains(&example) {
                                entry.push(example);
                            }
                        }
                    }
                }
            }
            for ((a, b), examples) in pair_files {
                let (na, nb) = (
                    name_of.get(a.as_str()).copied().unwrap_or(&a),
                    name_of.get(b.as_str()).copied().unwrap_or(&b),
                );
                let (da, db_) = (
                    layer_of.get(a.as_str()).copied().unwrap_or(""),
                    layer_of.get(b.as_str()).copied().unwrap_or(""),
                );
                // Adjudicated: a decision note on the IMPORTING (lower)
                // intent newer than its newest grounding says the
                // up-dependency is deliberate; a new grounding re-opens it.
                if let Some(note) = adjudicated(
                    a.as_str(),
                    newest_grounding.get(a.as_str()).copied().unwrap_or(""),
                ) {
                    adjudicated_out.push(AdjudicatedSmell {
                        kind: "layering_violation".into(),
                        summary: format!(
                            "'{na}' ({da}) depends on '{nb}' ({db_}) against the declared layer order"
                        ),
                        ruling: note.text.clone(),
                        ruled_by: note.author.clone(),
                        ruled_at: note.created_at.clone(),
                        reopens_when: "a new grounding lands on the importing intent".into(),
                        teaching: teaching_for("layering_violation"),
                    });
                    continue;
                }
                smells.push(Smell {
                    kind: "layering_violation".into(),
                    score: 6.0 + examples.len() as f64,
                    summary: format!(
                        "'{na}' ({da}) depends on '{nb}' ({db_}) against the declared layer order"
                    ),
                    evidence: format!(
                        "`loom layer order` puts '{da}' below '{db_}', but the dependency points UP: {} (a recorded relationship does not excuse direction)",
                        examples.join(", ")
                    ),
                    remedy: format!(
                        "invert the dependency: whatever '{da}' code reaches up to use belongs at or below '{da}' — move it down (or extract it into a lower shared module) so '{db_}' imports it instead of being imported; if the ARCHITECTURE changed, redeclare it: `loom layer order <top> … <bottom>`; if this up-dependency is DELIBERATE, record the call: `loom note add --intent {a} --kind decision --text \"<why this layer may reach up>\"` resolves this finding (a new grounding re-opens it)"
                    ),
                    teaching: teaching_for("layering_violation"),
                });
            }
        }
    }

    // 7. Recurrent trouble — the graph's memory of regressions: targets whose
    //    transition history keeps returning to failing / needs_change. A spot
    //    that broke twice will break a third time; it needs redesign, not
    //    another patch.
    //
    //    TERMINAL STATE: a kind=decision note on the target that is NEWER than
    //    its last regression refutes the finding ("redesigned/resolved, here's
    //    why") — the append-only history stays intact, but an addressed
    //    recurrence stops nagging. A regression AFTER the decision re-flags.
    {
        let mut trouble: HashMap<(String, String), usize> = HashMap::new();
        let mut last_trouble: HashMap<(String, String), String> = HashMap::new();
        let mut trouble_notes: HashMap<(String, String), Vec<&crate::types::Note>> = HashMap::new();
        for n in all_notes {
            if n.kind == "transition"
                && (n.text.ends_with("→ failing") || n.text.ends_with("→ needs_change"))
            {
                let key = (n.target_kind.clone(), n.target_id.clone());
                *trouble.entry(key.clone()).or_insert(0) += 1;
                trouble_notes.entry(key.clone()).or_default().push(n);
                let e = last_trouble.entry(key).or_default();
                if n.created_at > *e {
                    *e = n.created_at.clone();
                }
            }
        }
        let edge_label: HashMap<&str, String> = {
            let mut m: HashMap<&str, String> = HashMap::new();
            for e in relates {
                m.insert(e.id.as_str(), format!("{} × {}", e.from_name, e.to_name));
            }
            for g in governs {
                m.insert(
                    g.id.as_str(),
                    format!("{} → {}", g.rule_name, g.intent_name),
                );
            }
            m
        };
        for ((kind, id), count) in trouble {
            if count < 2 {
                continue;
            }
            let label = if kind == "intent" {
                name_of.get(id.as_str()).copied().unwrap_or(&id).to_string()
            } else {
                edge_label
                    .get(id.as_str())
                    .cloned()
                    .unwrap_or_else(|| id.clone())
            };
            // The last regression: guaranteed present (filled in the same
            // pass that counted `trouble`); doubles as the adjudication
            // anchor and the evidence timestamp.
            let last = last_trouble
                .get(&(kind.clone(), id.clone()))
                .map(String::as_str)
                .unwrap_or("");
            let mut recent = trouble_notes
                .get(&(kind.clone(), id.clone()))
                .cloned()
                .unwrap_or_default();
            recent.sort_by(|a, b| {
                b.created_at
                    .cmp(&a.created_at)
                    .then_with(|| b.text.cmp(&a.text))
            });
            let recent_detail = recent
                .iter()
                .take(3)
                .map(|n| format!("{} {} by {}", n.created_at, n.text, n.author))
                .collect::<Vec<_>>()
                .join(" · ");
            let history_cmd = if kind == "intent" {
                format!("loom note list --intent {id} --kind transition --limit 0")
            } else {
                format!("loom note list --edge {id} --kind transition --limit 0")
            };
            // Addressed: a decision note recorded after the last regression.
            if let Some(d) = last_decision.get(id.as_str()) {
                if d.created_at.as_str() > last {
                    adjudicated_out.push(AdjudicatedSmell {
                        kind: "recurrent_trouble".into(),
                        summary: format!("'{}' has regressed {} times", label, count),
                        ruling: d.text.clone(),
                        ruled_by: d.author.clone(),
                        ruled_at: d.created_at.clone(),
                        reopens_when:
                            "another failing/needs_change transition lands after the ruling".into(),
                        teaching: recurrent_teaching(&kind, &id),
                    });
                    continue;
                }
            }
            smells.push(Smell {
                kind: "recurrent_trouble".into(),
                score: 2.0 * count as f64,
                summary: format!(
                    "'{}' has regressed {} times (transitions to failing/needs_change)",
                    label, count
                ),
                evidence: format!(
                    "{count} transition(s) to failing/needs_change, the last at {last}; recent regressions: {recent_detail}; full history: `{history_cmd}`"
                ),
                remedy: format!(
                    "recurring breakage means the criterion or the design is wrong — propose the redesign instead of patching again: `loom hypothesis add --name \"…\" --claim \"<what keeps regressing and the structural why>\" --proposal \"<the redesign>\" --predicted-outcome \"<no failing/needs_change transition after the next N syncs>\"{target}` (proven → adopted → planned intents); once addressed, `loom note add{nt} --kind decision --text \"<what was redesigned and why it won't recur>\"` resolves this finding (a decision newer than the last regression; history stays intact)",
                    target = if kind == "intent" { format!(" --target {id}") } else { String::new() },
                    nt = if kind == "intent" { format!(" --intent {id}") } else { format!(" --edge {id}") },
                ),
                teaching: recurrent_teaching(&kind, &id),
            });
        }
    }

    // 8. Hypothesis accumulation — the pre-decision plane turning into a note
    //    dump. Proposed hypotheses are intentionally optional and non-gating
    //    while small/fresh, but a stale or swollen queue means agents are adding
    //    redesign ideas faster than they prove, reject, or adopt them. The
    //    remedy is not a decision note; the terminal states live on the
    //    hypothesis state machine itself.
    {
        let proposed = super::hypothesis::list_hypotheses(db, Some("proposed"))?;
        if !proposed.is_empty() {
            let now = chrono::Utc::now();
            let stale: Vec<&crate::types::Hypothesis> = proposed
                .iter()
                .filter(|h| {
                    chrono::DateTime::parse_from_rfc3339(&h.created_at)
                        .map(|created| {
                            now.signed_duration_since(created.with_timezone(&chrono::Utc))
                                .num_days()
                                >= HYPOTHESIS_STALE_DAYS
                        })
                        .unwrap_or(false)
                })
                .collect();
            if proposed.len() >= HYPOTHESIS_BACKLOG_LIMIT || !stale.is_empty() {
                let targeted: HashSet<String> = super::targets::list_all_targets(db)?
                    .into_iter()
                    .map(|t| t.hypothesis_id)
                    .collect();
                let untargeted = proposed
                    .iter()
                    .filter(|h| !targeted.contains(h.id.as_str()))
                    .count();
                let oldest = proposed
                    .iter()
                    .min_by(|a, b| {
                        a.created_at
                            .cmp(&b.created_at)
                            .then_with(|| a.name.cmp(&b.name))
                    })
                    .expect("proposed is not empty");
                let sample: Vec<&str> = proposed.iter().take(5).map(|h| h.name.as_str()).collect();
                let stale_names: Vec<&str> =
                    stale.iter().take(5).map(|h| h.name.as_str()).collect();
                let stale_detail = if stale_names.is_empty() {
                    "none".to_string()
                } else {
                    stale_names.join(" · ")
                };
                smells.push(Smell {
                    kind: "hypothesis_accumulation".into(),
                    score: proposed.len() as f64 + 3.0 * stale.len() as f64,
                    summary: format!(
                        "{} proposed hypothesis(es) are waiting for proof; {} stale, {} untargeted",
                        proposed.len(),
                        stale.len(),
                        untargeted
                    ),
                    evidence: format!(
                        "{} proposed hypothesis(es), {} older than {}d, {} without TARGETS; oldest is '{}' created at {}; examples: {}; stale examples: {}",
                        proposed.len(),
                        stale.len(),
                        HYPOTHESIS_STALE_DAYS,
                        untargeted,
                        oldest.name,
                        oldest.created_at,
                        sample.join(" · "),
                        stale_detail
                    ),
                    remedy: format!(
                        "drain the pre-decision plane: `loom next --mode prove` then `loom hypothesis prove <id> --verdict supported|refuted --evidence \"…\"`; for supported claims, adopt or reject them (`loom hypothesis adopt|reject`); for untargeted claims, add TARGETS first (`loom hypothesis target <id> <intent>`). Green requires fewer than {limit} proposed hypotheses and none older than {days}d.",
                        limit = HYPOTHESIS_BACKLOG_LIMIT,
                        days = HYPOTHESIS_STALE_DAYS,
                    ),
                    teaching: teaching_for("hypothesis_accumulation"),
                });
            }
        }
    }

    // 9. Happy path only — the behavioral vantage point: a feature group that
    //    declared its sunny-day intent (aspect=happy) but never realized and
    //    proved what failure or degradation look like. The happy aspect is the
    //    trigger; sad/fallback only clear the smell once they are implemented,
    //    grounded, and directly proven. The LLM still decides whether
    //    sad/fallback are real requirements here or honestly N/A (record that
    //    as a decision note).
    {
        let mut child_aspects: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut satisfied_aspects: HashMap<&str, HashSet<&str>> = HashMap::new();
        let mut newest_aspect_child: HashMap<&str, &str> = HashMap::new();
        let by_id: HashMap<&str, &crate::types::Intent> =
            intents.iter().map(|i| (i.id.as_str(), i)).collect();
        let passed_validation_ids: HashSet<&str> = snapshot
            .validations
            .iter()
            .filter(|v| v.last_result == "passed")
            .map(|v| v.id.as_str())
            .collect();
        let directly_proven_intents: HashSet<&str> = snapshot
            .validates
            .iter()
            .filter(|e| passed_validation_ids.contains(e.validation_id.as_str()))
            .map(|e| e.intent_id.as_str())
            .collect();
        for (p, c) in hierarchy {
            let Some(child) = by_id.get(c.as_str()) else {
                continue;
            };
            if child.aspect.is_empty() {
                continue;
            }
            child_aspects
                .entry(p.as_str())
                .or_default()
                .insert(child.aspect.as_str());
            if matches!(child.aspect.as_str(), "sad" | "fallback")
                && child.lifecycle == "implemented"
                && files_of.contains_key(child.id.as_str())
                && directly_proven_intents.contains(child.id.as_str())
            {
                satisfied_aspects
                    .entry(p.as_str())
                    .or_default()
                    .insert(child.aspect.as_str());
            }
            let e = newest_aspect_child.entry(p.as_str()).or_default();
            if child.created_at.as_str() > *e {
                *e = &child.created_at;
            }
        }
        for (parent_id, aspects) in &child_aspects {
            if !aspects.contains("happy") {
                continue;
            }
            let satisfied = satisfied_aspects.get(parent_id);
            let missing: Vec<&str> = ["sad", "fallback"]
                .iter()
                .filter(|a| !satisfied.is_some_and(|s| s.contains(*a)))
                .copied()
                .collect();
            if missing.is_empty() {
                continue;
            }
            // Adjudicated: a decision note on the parent newer than its newest
            // aspect-carrying child records why the missing path is N/A. A new
            // aspect-tagged child re-opens the question.
            let pname = name_of.get(parent_id).copied().unwrap_or(parent_id);
            if let Some(note) = adjudicated(
                parent_id,
                newest_aspect_child.get(parent_id).copied().unwrap_or(""),
            ) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "happy_path_only".into(),
                    summary: format!(
                        "'{}' declares a happy path but no realized+proven {} behavior",
                        pname,
                        missing.join("/")
                    ),
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "a new aspect-tagged child lands under this intent".into(),
                    teaching: teaching_for("happy_path_only"),
                });
                continue;
            }
            smells.push(Smell {
                kind: "happy_path_only".into(),
                score: 2.0 + 2.0 * missing.len() as f64,
                summary: format!(
                    "'{}' declares a happy path but no realized+proven {} behavior",
                    pname,
                    missing.join("/")
                ),
                evidence: format!(
                    "children carry aspects {{{}}}; realized+proven sad/fallback aspects {{{}}} — failure/degradation behavior is not implemented, grounded, and directly proven",
                    {
                        let mut v: Vec<&str> = aspects.iter().copied().collect();
                        v.sort();
                        v.join(", ")
                    },
                    {
                        let mut v: Vec<&str> = satisfied
                            .map(|s| s.iter().copied().collect())
                            .unwrap_or_default();
                        v.sort();
                        v.join(", ")
                    }
                ),
                remedy: format!(
                    "realize and prove the missing path(s): loom intent add --aspect sad --level feature … then loom edge hierarchy {parent_id} <child>, ground it with `loom edge implement`, and attach a passed validation; or record why it's N/A: loom note add --intent {parent_id} --kind decision --text \"<why no {m} path>\" (resolves this finding; a new aspect-tagged child re-opens it)",
                    m = missing.join("/")
                ),
                teaching: teaching_for("happy_path_only"),
            });
        }
    }

    // 10. Unused rule — a measuring stick connected to nothing at all.
    let used: HashSet<&str> = governs.iter().map(|g| g.rule_id.as_str()).collect();
    for r in rules {
        if !used.contains(r.id.as_str()) {
            smells.push(Smell {
                kind: "unused_rule".into(),
                score: 5.0,
                summary: format!("rule '{}' governs nothing", r.name),
                evidence: "a quality rule with zero GOVERNS edges measures nothing".into(),
                remedy: format!(
                    "loom rule verdict {} <intent-id> --status passing|failing|independent --criterion … --evidence … (the verdict creates the edge and measures it in one step; independent = the rule does not apply) — or delete it if it was a mistake",
                    r.id
                ),
                teaching: teaching_for("unused_rule"),
            });
        }
    }

    // 11. Vocab drift — the registry policing itself: two registered terms
    //     that read like the same word (edit distance / containment). The
    //     vocabulary's value is forced collision; synonym terms split the
    //     keyspace and silently halve the signal. Detection-and-merge is the
    //     designed governance — never a closed list.
    {
        let terms = super::vocab::list_vocab_terms(db)?;
        let counts = super::vocab::tag_counts(intents)?;
        for i in 0..terms.len() {
            for j in (i + 1)..terms.len() {
                let (a, b) = (&terms[i], &terms[j]);
                if !super::vocab::terms_look_alike(&a.name, &b.name) {
                    continue;
                }
                let (ua, ub) = (
                    counts.get(&a.name).copied().unwrap_or(0),
                    counts.get(&b.name).copied().unwrap_or(0),
                );
                // Keep the better-established term; merge the other into it.
                let (keep, drop) = if ua >= ub { (a, b) } else { (b, a) };
                smells.push(Smell {
                    kind: "vocab_drift".into(),
                    score: 3.0 + (ua + ub) as f64,
                    summary: format!(
                        "vocab terms '{}' and '{}' read like the same word — split keyspace halves collision signal",
                        a.name, b.name
                    ),
                    evidence: format!(
                        "'{}' tags {} intent(s), '{}' tags {} intent(s); intents split across synonym terms never collide in duplicate detection",
                        a.name, ua, b.name, ub
                    ),
                    remedy: format!(
                        "loom vocab merge {} {}  → retags every intent and deletes '{}' (one sweep, nothing to re-inspect); if they are genuinely distinct concepts the NAMES must stop reading alike — register a sharper term (`loom vocab add`), retag its intents (`loom intent tag`), then merge the look-alike away",
                        drop.name, keep.name, drop.name
                    ),
                    teaching: teaching_for("vocab_drift"),
                });
            }
        }
    }

    // 12. Unjourneyed surface — the consumer plane's completeness check: a
    //     user_visible intent with real code that NO PASSED saga exercises
    //     end-to-end. The product claims a consumer can see/feel it; no consumer
    //     journey ever proves it. Visibility is the key — this smell is what
    //     makes the `user_visible` ruling load-bearing outside the align
    //     interview.
    //
    //     Two regimes, so a journey-less repo isn't flooded:
    //     - ZERO passed sagas → ONE aggregate finding on the root intent
    //       ("no proven consumer journey at all"), adjudicated by a decision
    //       note on the root newer than the newest user_visible intent.
    //     - ≥1 passed saga → per-intent findings; the instrument is in use, so
    //       each gap is meaningful. Adjudicated by a decision note on the intent
    //       newer than its last redefinition (updated_at).
    //
    //     Coverage propagates BOTH ways through the tree: a step bound at
    //     component altitude exercises the features the journey runs through,
    //     and a journeyed leaf suppresses its ancestors' own findings
    //     (unjourneyed SIBLINGS still fire individually).
    {
        let all_saga_ids: HashSet<&str> = snapshot
            .validations
            .iter()
            .filter(|v| v.validation_type == "saga")
            .map(|v| v.id.as_str())
            .collect();
        let passed_saga_ids: HashSet<&str> = snapshot
            .validations
            .iter()
            .filter(|v| v.validation_type == "saga" && v.last_result == "passed")
            .map(|v| v.id.as_str())
            .collect();
        let journeyed: HashSet<&str> = snapshot
            .validates
            .iter()
            .filter(|e| passed_saga_ids.contains(e.validation_id.as_str()))
            .map(|e| e.intent_id.as_str())
            .collect();
        let mut covered: HashSet<&str> = journeyed.clone();
        // Ancestors of journeyed intents (parent_of comes from section 5;
        // the tree is insert-enforced acyclic, visited is belt-and-braces).
        for id in &journeyed {
            let mut cur = *id;
            let mut visited: HashSet<&str> = HashSet::new();
            while let Some(p) = parent_of.get(cur) {
                if !visited.insert(cur) {
                    break;
                }
                covered.insert(p);
                cur = p;
            }
        }
        // Descendants of journeyed intents.
        let mut children_of: HashMap<&str, Vec<&str>> = HashMap::new();
        for (p, c) in hierarchy {
            children_of.entry(p.as_str()).or_default().push(c.as_str());
        }
        let mut stack: Vec<&str> = journeyed.iter().copied().collect();
        let mut walked: HashSet<&str> = HashSet::new();
        while let Some(id) = stack.pop() {
            if !walked.insert(id) {
                continue;
            }
            if let Some(kids) = children_of.get(id) {
                for k in kids {
                    covered.insert(k);
                    stack.push(k);
                }
            }
        }
        let candidates: Vec<&crate::types::Intent> = intents
            .iter()
            .filter(|i| {
                i.visibility == "user_visible"
                    && i.status != "deprecated"
                    && i.abstraction_level != "system" // system grounds to manifests; journeys live below
                    && files_of.contains_key(i.id.as_str()) // real code to exercise
                    && !covered.contains(i.id.as_str())
            })
            .collect();

        if passed_saga_ids.is_empty() {
            if !candidates.is_empty() {
                // The aggregate target: the system root (or any root) — "this
                // product has no proven consumer surface" is a claim about the root.
                let is_child: HashSet<&str> = hierarchy.iter().map(|(_, c)| c.as_str()).collect();
                let mut roots: Vec<&crate::types::Intent> = intents
                    .iter()
                    .filter(|i| i.status != "deprecated" && !is_child.contains(i.id.as_str()))
                    .collect();
                roots.sort_by_key(|i| (i.abstraction_level != "system", i.name.clone()));
                if let Some(root) = roots.first() {
                    let newest_uv = intents
                        .iter()
                        .filter(|i| i.visibility == "user_visible")
                        .map(|i| i.created_at.as_str())
                        .max()
                        .unwrap_or("");
                    let newest_saga_binding = snapshot
                        .validates
                        .iter()
                        .filter(|e| all_saga_ids.contains(e.validation_id.as_str()))
                        .map(|e| e.created_at.as_str())
                        .max()
                        .unwrap_or("");
                    let newest_consumer_surface = std::cmp::max(newest_uv, newest_saga_binding);
                    if let Some(note) = adjudicated(root.id.as_str(), newest_consumer_surface) {
                        adjudicated_out.push(AdjudicatedSmell {
                            kind: "unjourneyed_surface".into(),
                            summary: format!(
                                "no passed consumer journey — {} user_visible intent(s) never exercised end-to-end",
                                candidates.len()
                            ),
                            ruling: note.text.clone(),
                            ruled_by: note.author.clone(),
                            ruled_at: note.created_at.clone(),
                            reopens_when: "a new user_visible intent or saga binding lands after the ruling (or a first saga passes — per-intent gaps become visible)".into(),
                            teaching: teaching_for("unjourneyed_surface"),
                        });
                    } else {
                        let sample: Vec<&str> =
                            candidates.iter().take(3).map(|i| i.name.as_str()).collect();
                        smells.push(Smell {
                            kind: "unjourneyed_surface".into(),
                            score: 3.0 + candidates.len() as f64,
                            summary: format!(
                                "no passed consumer journey — {} user_visible intent(s) are never exercised end-to-end",
                                candidates.len()
                            ),
                            evidence: format!(
                                "the product claims these are consumer-visible, but no passed saga touches any intent: e.g. {}",
                                sample.join(" · ")
                            ),
                            remedy: format!(
                                "narrate the first consumer journey: write the saga YAML (each step binds to the intent it exercises) and `loom saga add <spec.yaml>` (steps may spawn missing intents with --spawn-missing); if this product exposes NO consumer-reachable surface, record the call: `loom note add --intent {} --kind decision --text \"no consumer surface: <why>\"` resolves this finding (a new user_visible intent re-opens it)",
                                root.id
                            ),
                            teaching: teaching_for("unjourneyed_surface"),
                        });
                    }
                }
            }
        } else {
            for i in candidates {
                if let Some(note) = adjudicated(i.id.as_str(), i.updated_at.as_str()) {
                    adjudicated_out.push(AdjudicatedSmell {
                        kind: "unjourneyed_surface".into(),
                        summary: format!(
                            "'{}' is user_visible but no passed journey exercises it",
                            i.name
                        ),
                        ruling: note.text.clone(),
                        ruled_by: note.author.clone(),
                        ruled_at: note.created_at.clone(),
                        reopens_when: "the intent is redefined after the ruling".into(),
                        teaching: teaching_for("unjourneyed_surface"),
                    });
                    continue;
                }
                smells.push(Smell {
                    kind: "unjourneyed_surface".into(),
                    score: if i.abstraction_level == "component" { 5.0 } else { 4.0 },
                    summary: format!(
                        "'{}' is user_visible but no passed consumer journey exercises it",
                        i.name
                    ),
                    evidence: format!(
                        "a {}-level intent ruled user_visible, grounded in code, reached by no passed saga (directly or via the tree)",
                        i.abstraction_level
                    ),
                    remedy: format!(
                        "extend a journey (or narrate a new one) with a step bound to this intent, then `loom saga add <spec.yaml>` + `loom saga run <name>`; if this surface is not consumer-reachable after all, the ruling is wrong — `loom intent confirm {id} --visibility internal`; if it IS consumer-visible but honestly un-journeyable, record the call: `loom note add --intent {id} --kind decision --text \"<why no journey>\"` resolves this finding (a redefinition re-opens it)",
                        id = i.id
                    ),
                    teaching: teaching_for("unjourneyed_surface"),
                });
            }
        }
    }

    // 14. Symbol accountability — raw symbol coverage is noisy, but public or
    // risky-file symbols without precise ownership are real accountability
    // gaps. This detector consumes the same structural instrument that
    // `loom coverage` renders in detail.
    {
        let report = super::symbol_accountability::symbol_accountability_from_parts_with_notes(
            &snapshot.codefiles,
            intents,
            implements,
            all_notes,
        );
        if !report.actionable_symbol_gaps.is_empty() {
            let examples: Vec<String> = report
                .actionable_symbol_gaps
                .iter()
                .take(5)
                .map(|gap| format!("{} @ {}:{}", gap.label, gap.path, gap.line_start))
                .collect();
            smells.push(Smell {
                kind: "symbol_accountability_gap".into(),
                score: 6.0 + report.actionable_symbol_gaps.len() as f64,
                summary: format!(
                    "{} open actionable symbol gap(s): behavior-significant symbols lack precise ownership",
                    report.actionable_symbol_gaps.len()
                ),
                evidence: format!(
                    "symbol accountability: {} required, {} grounded, {} accepted, {} adjudicated, {} raw gap(s), {} open gap(s). Examples: {}",
                    report.summary.required,
                    report.summary.grounded,
                    report.summary.accepted,
                    report.summary.adjudicated,
                    report.summary.raw_actionable_gaps,
                    report.summary.actionable_gaps,
                    examples.join(" · ")
                ),
                remedy: "Use `loom coverage --json` → actionable_symbol_gaps. For each top gap, inspect `loom codefile show <path>`, then refine the right IMPLEMENTS locator, split/add the behavior intent, or record a current decision note on the file/owning intent accepting broad ownership.".into(),
                teaching: teaching_for("symbol_accountability_gap"),
            });
        } else if let Some(gap) = report
            .adjudicated_symbol_gaps
            .iter()
            .max_by_key(|gap| gap.ruled_at.as_str())
        {
            adjudicated_out.push(AdjudicatedSmell {
                kind: "symbol_accountability_gap".into(),
                summary: format!(
                    "{} raw symbol gap(s) accepted by current decision notes",
                    report.summary.raw_actionable_gaps
                ),
                ruling: gap.ruling.clone(),
                ruled_by: gap.ruled_by.clone(),
                ruled_at: gap.ruled_at.clone(),
                reopens_when: gap.reopens_when.clone(),
                teaching: teaching_for("symbol_accountability_gap"),
            });
        }
    }

    smells.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.summary.cmp(&b.summary))
            .then_with(|| a.evidence.cmp(&b.evidence))
            .then_with(|| a.remedy.cmp(&b.remedy))
    });
    adjudicated_out.sort_by(|a, b| {
        a.ruled_at
            .cmp(&b.ruled_at)
            .then_with(|| a.summary.cmp(&b.summary))
    });
    // The instrument's own coverage: registered tags are the strongest
    // duplicated_responsibility signal, so the report carries how much of the
    // coded surface has that high-signal coverage.
    let coded_intents = intents
        .iter()
        .filter(|i| files_of.contains_key(i.id.as_str()))
        .count();
    let tagged_coded_intents = intents
        .iter()
        .filter(|i| {
            files_of.contains_key(i.id.as_str())
                && discovery
                    .tags_by_intent
                    .get(i.id.as_str())
                    .is_some_and(|t| !t.is_empty())
        })
        .count();
    let coded_layers = intents
        .iter()
        .filter(|i| files_of.contains_key(i.id.as_str()) && !i.layer.is_empty())
        .map(|i| i.layer.as_str())
        .collect::<HashSet<_>>()
        .len();
    let declared_layers = super::meta::get_layer_order(db)?.len();
    Ok(SmellReport {
        open: smells,
        adjudicated: adjudicated_out,
        coded_intents,
        tagged_coded_intents,
        coded_layers,
        declared_layers,
    })
}
