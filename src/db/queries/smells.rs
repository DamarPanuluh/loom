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

// The `detect_*` helpers below were extracted from `compute_smells_from_parts`
// (one function per smell). Each inherently consumes several pre-computed graph
// planes, so some exceed clippy's 7-argument guideline — bundling them into a
// context struct would only trade arg-count for field-count. Each helper is also
// doc-numbered to match its position in the orchestrator, so its leading "N."
// reads as a markdown list item to clippy's doc linter though it is deliberate
// ordering. Both are intentional for this detector module.
#![allow(clippy::too_many_arguments, clippy::doc_lazy_continuation)]

use anyhow::Result;
use serde::{ser::SerializeStruct, Serialize};
use std::collections::{HashMap, HashSet};

use super::snapshot::{DiscoverySnapshot, QuerySnapshot};

pub(crate) const KIND_ARCH_VERDICT_CONTRADICTS: &str = "architecture_verdict_contradicts_layering";
use crate::types::{Hypothesis, Note, TargetsEdge, VocabTerm};

mod consumer;
mod coupling;
mod lifecycle;
mod normative;
mod physical;
mod semantic;

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
/// Lexical duplicate detectors ignore words carried by more than this many
/// intents (or the scale-adjusted cap below). A word that common is vocabulary
/// background, not evidence that two behaviors are the same responsibility.
pub const LEXICAL_SIGNAL_TOKEN_CARRIER_FLOOR: usize = 8;
/// Proposed hypotheses are useful pressure, but a large queue means the
/// pre-decision plane is becoming a note dump instead of a proof pipeline.
pub const HYPOTHESIS_BACKLOG_LIMIT: usize = 10;
/// A proposed hypothesis older than this is stale enough to surface even when
/// the queue is small. Non-RFC3339 timestamps (old tests/imports) are ignored.
pub const HYPOTHESIS_STALE_DAYS: i64 = 14;

fn rfc3339_after(candidate: &str, anchor: &str) -> bool {
    let Ok(candidate) = chrono::DateTime::parse_from_rfc3339(candidate) else {
        return false;
    };
    if anchor.is_empty() {
        return true;
    }
    let Ok(anchor) = chrono::DateTime::parse_from_rfc3339(anchor) else {
        return true;
    };
    candidate > anchor
}
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
/// A clone group must span symbols at least this many lines long — below this,
/// identical bodies are boilerplate (trivial getters, single match arms), not
/// a copy-paste worth flagging.
pub const MIN_CLONE_LINES: usize = 5;
/// How many entries a finding's evidence list shows before collapsing the rest
/// into "… and N more". The finding's summary always states the true total, so
/// a high-cardinality smell (a 40-copy clone, a file serving 30 intents) can't
/// flood the reader's context with a wall of locations.
const EVIDENCE_LIST_CAP: usize = 8;

/// Join `items` with `sep`, showing at most [`EVIDENCE_LIST_CAP`] and appending
/// "… and N more" when truncated. Shared by every list-shaped smell evidence.
fn capped_join<S: AsRef<str>>(items: &[S], sep: &str) -> String {
    let n = items.len();
    let mut shown: Vec<String> = items
        .iter()
        .take(EVIDENCE_LIST_CAP)
        .map(|s| s.as_ref().to_string())
        .collect();
    if n > EVIDENCE_LIST_CAP {
        shown.push(format!("… and {} more", n - EVIDENCE_LIST_CAP));
    }
    shown.join(sep)
}
/// Behavioral symbols above this span are large enough to inspect for splitting.
/// Deliberately high: this is a coarse snapshot signal, not real complexity.
pub const LARGE_BEHAVIORAL_SYMBOL_LINES: usize = 200;
/// Complexity thresholds are deliberately high and advisory: the metric is a
/// deterministic suspicion, not a proof that extraction is correct.
pub const COMPLEX_SYMBOL_CYCLOMATIC: usize = 18;
pub const COMPLEX_SYMBOL_COGNITIVE: usize = 30;
pub const DEEPLY_NESTED_SYMBOL_DEPTH: usize = 5;
pub const MANY_EXIT_PATHS: usize = 8;
pub const MANY_ARGUMENTS: usize = 7;
pub const MANY_AWAITS: usize = 8;
/// Files whose physical extent crosses this are god-files — large enough that
/// the per-symbol `large_behavioral_symbol` detector fans out into many
/// adjudicable per-symbol findings that each get ruled away, never surfacing
/// the irreducible "this whole file is too big". `oversized_file` keys on the
/// file's total physical extent (independent of impl/test classification), so
/// it survives the per-symbol adjudication path. Deliberately high: this is a
/// coarse god-file signal, and the finding is a suspicion to inspect, not a
/// violation. Extent is a lower bound on LOC (the graph stores no line count —
/// it's the last symbol's end line), so the threshold sits well below the
/// 6734-line god-file that motivated it.
pub const OVERSIZED_FILE_LINES: usize = 2000;

/// Size/LOC smells are ADVISORY, not gating: line count is a coarse proxy, and
/// whether a large file/function should split is a case-by-case judgment. These
/// kinds are surfaced for the LLM to inspect (`loom smells`) but partitioned out
/// of the gating `open` set — they never block green. A genuinely-deliberate
/// large unit needs no decision note; the flag is just a prompt to look.
pub const SIZE_ADVISORY_KINDS: &[&str] = &[
    "oversized_file",
    "large_behavioral_symbol",
    "complex_symbol",
    "hub_file",
];
/// Dischargeable metadata-completeness DEBT — "the detector is under-armed because
/// metadata is incomplete", NOT a code defect. Surfaced by `loom smells` so the gap
/// stays visible, but partitioned out of the gating `open` set: a hard gate here
/// pressures the driver to launder it away with a `--kind decision` ruling rather
/// than discharge it honestly (tag the intents / enrich the vocab). Debt is paid
/// down over time; it must never force a green-or-launder choice on the maturity ladder.
pub const DEBT_KINDS: &[&str] = &["duplicate_detection_unarmed"];
/// Repeated string-contract detection ignores short labels and implementation
/// tokens. These floors are intentionally conservative for the first pass.
pub const MIN_STRING_CONTRACT_CHARS: usize = 24;
pub const MIN_STRING_CONTRACT_TOKENS: usize = 4;

/// Aspect families for the `happy_path_only` audit: a TRIGGER aspect implies its
/// REQUIRED sibling aspects must also exist (and, in the gating detector, be
/// realized+grounded+proven). Two families share one detector — the behavioral
/// family (happy → sad/fallback) and the UI-state family (populated →
/// empty/error). `loading` is a recognized UI state but deliberately NOT
/// required: a screen can legitimately have no distinct loading view, so it
/// never triggers and is never demanded. Aspect stays an OPEN vocabulary at
/// write time — this table only drives the coverage smell, it does not validate
/// the field. Shared verbatim by the `stats.rs` completeness-gaps report so the
/// two never diverge on which states a parent owes.
pub const ASPECT_FAMILIES: &[(&str, &[&str])] = &[
    ("happy", &["sad", "fallback"]),
    ("populated", &["empty", "error"]),
];

/// One derived finding, with the exact remedy that resolves it.
#[derive(Debug, Clone)]
pub struct Smell {
    /// twin_intents | duplicated_responsibility | overlapping_ownership
    /// | scattered_intent | tangled_file | unmeasured_intents
    /// | undeclared_coupling | layering_violation | recurrent_trouble
    /// | unused_rule | happy_path_only | vocab_drift | duplicate_detection_unarmed
    /// | hypothesis_accumulation | symbol_accountability_gap | dependency_cycle
    /// | intent_island | transitive_layering_violation | cochange_coupling
    /// | nonlocal_proof | code_clone | large_behavioral_symbol | oversized_file
    /// | string_contract_duplicate | panic_marker_risk | shotgun_surgery
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

impl Smell {
    /// Stable finding identity used by `loom note add --smell`.
    ///
    /// Most detector remedies already embed the exact `--smell` key; relationship
    /// findings resolve through `loom edge explore`, so their key is derived from
    /// the same ordered pair the detector printed.
    pub fn id(&self) -> String {
        smell_identity(&self.kind, &self.remedy)
    }

    /// Intent ids named by the finding identity/remedy, when the detector exposes
    /// them. Pair findings return both endpoints in detector order.
    pub fn intent_ids(&self) -> Vec<String> {
        smell_intent_ids(&self.kind, &self.remedy)
    }
}

impl Serialize for Smell {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("Smell", 8)?;
        state.serialize_field("id", &self.id())?;
        state.serialize_field("intent_ids", &self.intent_ids())?;
        state.serialize_field("kind", &self.kind)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("summary", &self.summary)?;
        state.serialize_field("evidence", &self.evidence)?;
        state.serialize_field("remedy", &self.remedy)?;
        state.serialize_field("teaching", &self.teaching)?;
        state.end()
    }
}

fn smell_identity(kind: &str, remedy: &str) -> String {
    if let Some(id) = quoted_arg_after(remedy, "--smell") {
        return id;
    }
    if let Some((a, b)) = edge_explore_ids_from_text(remedy) {
        return format!("{kind}:{a}:{b}");
    }
    if let Some(intent) = arg_after(remedy, "--intent") {
        return format!("{kind}:{intent}");
    }
    format!("{kind}:{}", stable_hex(remedy))
}

fn smell_intent_ids(kind: &str, remedy: &str) -> Vec<String> {
    if matches!(
        kind,
        "undeclared_coupling"
            | "cochange_coupling"
            | "duplicated_responsibility"
            | "twin_intents"
            | "overlapping_ownership"
    ) {
        if let Some((a, b)) = edge_explore_ids_from_text(remedy) {
            return vec![a, b];
        }
    }
    arg_after(remedy, "--intent").into_iter().collect()
}

fn edge_explore_ids_from_text(text: &str) -> Option<(String, String)> {
    let tokens = shellish_tokens(text);
    tokens
        .windows(5)
        .find(|w| w[0] == "loom" && w[1] == "edge" && w[2] == "explore")
        .map(|w| (w[3].clone(), w[4].clone()))
        .filter(|(_, b)| !b.is_empty())
}

fn arg_after(text: &str, flag: &str) -> Option<String> {
    let tokens = shellish_tokens(text);
    tokens
        .windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
        .filter(|s| !s.is_empty())
}

fn quoted_arg_after(text: &str, flag: &str) -> Option<String> {
    let pos = text.find(flag)?;
    let rest = text[pos + flag.len()..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return arg_after(text, flag);
    }
    let body = &rest[quote.len_utf8()..];
    let end = body.find(quote)?;
    Some(body[..end].to_string()).filter(|s| !s.is_empty())
}

fn shellish_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in text.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '"' || ch == '\'' || ch == '`' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn stable_hex(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for b in text.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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

/// What the instrument actually measured: open suspicions AND the surfaced debt
/// buckets with their rulings. Hardened/Production-ready gating consumes `open`;
/// the stricter Excellent certificate also consumes `debt` and `advisory` counts.
/// A finding can be known without making the codebase excellent: accepted or
/// deferred real debt remains visible until fixed, disproven, or ruled deliberate design.
#[derive(Debug, Clone, Serialize)]
pub struct SmellReport {
    pub open: Vec<Smell>,
    /// Dischargeable metadata-completeness debt (see DEBT_KINDS): surfaced by
    /// `loom smells`, excluded from Production-ready `open`, but counted by the
    /// Excellent certificate so the detector being under-armed is not hidden.
    pub debt: Vec<Smell>,
    /// Size/LOC flags (oversized_file, large_behavioral_symbol): coarse signals
    /// for the LLM to inspect case-by-case. Surfaced by `loom smells`, excluded
    /// from Production-ready `open`, but counted as Excellence debt.
    pub advisory: Vec<Smell>,
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

/// The per-smell teaching corpus — a flat kind→(principle, inspect, avoid,
/// done_when) data table, in the same const-table idiom as guide.rs's
/// `ROLE_DISCIPLINE`. `teaching_for` re-inflates a row into the owned
/// `SmellTeaching` the renderer wants. `recurrent_trouble` is computed
/// (it formats a selector), so it stays out of the table.
#[allow(clippy::type_complexity)]
const TEACHING_TABLE: &[(&str, &str, &[&str], &[&str], &str)] = &[
    ("twin_intents", "Similar wording is a suspicion, not proof; inspect both meanings and code before merging or declaring independence.", &["read both intent criteria and descriptions", "inspect each intent's groundings before recording the edge verdict", "use `loom edge explore <a> <b>` only after evidence is checked"], &["do not merge or mark independent from name similarity alone"], "a RELATES_TO verdict explains the real relationship, or a proven merge hypothesis replaces one responsibility with one intent"),
    ("duplicated_responsibility", "Duplicate responsibility hides when unrelated files implement the same idea; tags and lexical fallback only point to where inspection must happen.", &["compare both intents' criteria, tags, and grounded code", "check whether the two implementations should share one owner or remain explicitly independent", "record the result with `loom edge explore <a> <b>`"], &["do not treat a tag or token collision as proof without reading the code"], "the pair has a grounded/independent relationship, or a proven merge hypothesis removes the duplicated ownership"),
    ("duplicate_detection_unarmed", "A quiet duplicate audit is weak when coded intents lack registered vocabulary tags; the lexical fallback is not equivalent to bounded terms.", &["`loom vocab suggest` — candidate terms mined from THIS graph's own intents (loom can't know your codebase's vocabulary), ranked by collision potential", "register the ones that name a real shared responsibility (`loom vocab add <term> --why …`) and tag coded intents with them (`loom intent tag add <intent> <term>`)", "`loom smells` after tagging to re-run duplicate detection"], &["do not accept a no-duplicate result while most coded intents are untagged"], "coded intents are tagged enough for duplicate detection, or a root decision records why the remaining blind spot is accepted"),
    ("overlapping_ownership", "Two intents claiming the same file need an explicit ownership contract; shared code is physical evidence of a relationship.", &["`loom codefile show <path>` for the shared file", "read the shared file once and decide what each intent owns", "`loom edge explore <a> <b>` to ground or refute the relationship"], &["do not leave shared-file ownership implicit"], "the intents have a grounded relationship, an independent verdict, or one grounding is moved to the correct owner"),
    ("scattered_intent", "A scattered intent usually means the graph intent is too broad; split intent meaning before proposing code movement.", &["read the directory clusters in the evidence", "`loom intent show <intent>` to inspect all groundings", "look for cohesive child responsibilities along the file clusters"], &["do not start a code refactor before proving the graph split or design problem"], "groundings are moved to cohesive child intents, or a newer decision explains why this spread is deliberate"),
    ("tangled_file", "A tangled file may be deliberate coordination or a real split candidate; code splitting is redesign work and should be proven first.", &["`loom codefile show <path>`", "read the listed intent owners and the shared transaction/module boundary", "if splitting is needed, propose it through `loom hypothesis add`"], &["do not split a coordinator file just to silence the smell"], "cohabitation has a current decision note naming the shared boundary that makes these intents one home (not a generic 'cohesive'), or an adopted/proven hypothesis restructures ownership"),
    ("unmeasured_intents", "A quality rule only matters where it has been honestly held against coded behavior; independent is a valid measured result.", &["`loom next --mode quality`", "measure at the highest honest component altitude before dropping to leaves", "record passing, failing, or independent with concrete evidence"], &["do not stamp broad rules across leaves with vacuous evidence"], "the rule has GOVERNS verdicts directly or via honest ancestor coverage for every coded intent it should measure"),
    ("undeclared_coupling", "Static imports are executable evidence that two owned responsibilities touch; the semantic graph must either declare or remove that coupling.", &["read the importing and imported files named in evidence", "inspect the two owning intents' criteria", "`loom edge explore <a> <b>` to ground the contract or record the issue"], &["do not add a relationship without naming the actual call/import contract"], "the coupling is grounded with evidence, marked as an issue to untangle, or the import is removed"),
    ("cochange_coupling", "Files that keep changing together are coupled even when neither imports the other — git history reveals the hidden contract static analysis misses. It is a SUGGESTION to investigate, not a defect: confirm the relationship or explain why the co-change is incidental.", &["read both intents' criteria and the files named in evidence", "decide if they share a hidden contract (serializer/deserializer, schema/consumer, code/fixture) or just co-changed in a wide refactor", "`loom edge explore <a> <b>` to ground the relationship or mark it independent"], &["do not treat co-change as proof of a relationship without reading the code; a wide refactor couples files incidentally"], "the pair has a grounded or independent RELATES_TO verdict (this is advisory — it never gates phase=complete)"),
    ("shotgun_surgery", "When one owned behavior repeatedly changes with many unrelated owned files, the boundary may be too broad, too central, or missing explicit relationships; git history exposes change pressure the static graph cannot.", &["read the named intent and the co-changing files in evidence", "decide whether the changes are one hidden contract, a wide mechanical refactor pattern, or a responsibility that should be split/reowned", "`loom edge explore <a> <b>` for real hidden contracts; use a decision note when the co-change is incidental"], &["do not split a stable coordinator just because it is central; prove the maintenance pain first", "do not add relationship edges from history alone without reading the current code"], "the hidden relationships are grounded/refuted, the broad responsibility is split through a proven hypothesis, or a decision note records why the recurring wide co-change is incidental (this is advisory — it never gates phase=complete)"),
    ("nonlocal_proof", "A passing LINKED validation is not a test that EXERCISES the intent's grounded code. The `proven` axis counts the link, so a leaf proven only by a test living in OTHER files reads green while its own code may have no direct test — partial-coverage overstatement. It is a SUGGESTION to investigate, not a defect.", &["read the intent's grounded file(s) and ask whether the named test actually drives that code path", "`loom intent show <intent>` for its groundings; `loom validation list` for the proof's command", "if the real proof is an e2e/subprocess check, record it as an `assertion`/`saga` validation — this advisory defers to those (it only judges `test`-type proofs it can statically locate)"], &["do not read a green `proven` axis as coverage of the grounded code; a co-grounded test can pass without touching this file"], "the grounded code has a directly-exercising test, the IMPLEMENTS locator is corrected, or a decision note records why the existing proof suffices (this is advisory — it never gates phase=complete)"),
    ("code_clone", "Structurally identical code in unrelated files is duplicated logic the intent-level detectors are blind to — they need shared tags, a shared file, or an import; copy-paste with renamed identifiers in disjoint code has none. It is a SUGGESTION to investigate, not a defect: a clone can be legitimate (generated code, deliberately independent copies that must not be coupled). Detection is not the decision — the same shape_hash flags both a coincidental dispatch shim (leave it) and a real five-copy utility (dedupe it); only reading the code tells them apart.", &["read the symbol in each listed location and decide whether they are one responsibility implemented twice or coincidentally similar", "`loom intent show <intent>` / `loom codefile show <path>` to see who owns each copy", "if both copies are owned, `loom edge explore <a> <b>` — a structural clone is evidence for a `duplicated_responsibility` merge", "real dup you are not deduping this pass? capture it as work, not a note: `loom hypothesis add` — the clone is the claim, the shape collapsing to one definition is the predicted outcome; `adopt --spawned` makes it a planned refactor"], &["do not refactor to dedupe before confirming the copies should share one owner — deliberately independent copies exist", "do not treat a short similar body as proof; the size floor already filters trivia, but read before merging", "do not bury a real-but-deferred dedupe in a decision note — use a hypothesis when there is real work to do; use a file decision only when the copies are deliberately independent"], "the owning intents have a grounded or independent RELATES_TO verdict, the duplication is removed, a refactor hypothesis tracks a deliberately deferred dedupe, or a decision note marks copies that must stay independent (this is advisory — it never gates phase=complete)"),
    ("string_contract_duplicate", "Repeated user-facing or contract strings drift when each copy becomes its own tiny source of truth. It is a suspicion to answer, not an automatic defect: exact repetition can be deliberate when the words belong to independent surfaces.", &["read each listed symbol and decide whether the repeated text is one contract, help/error message, or command example", "check whether one constant/helper should own the wording or whether independent copies are intentional", "`loom intent show <intent>` / `loom codefile show <path>` when the repeated strings are owned by mapped behavior"], &["do not centralize wording before confirming the copies must change together", "do not flag tiny labels, enum values, import paths, fixtures, or test strings as product contracts"], "the wording has one source of truth, the copies are intentionally independent and documented with a current decision note, or the owning intents have an explored relationship"),
    ("large_behavioral_symbol", "A very large function, method, def, or impl is usually carrying multiple decisions; span is only a suspicion, so inspect the behavior before splitting.", &["read the named symbol from top to bottom and identify distinct phases, modes, or responsibilities", "check whether the current intent ownership is broad because the behavior is broad, or because the code needs smaller internal boundaries", "look for validation gaps before extracting helpers so behavior stays pinned"], &["do not split a deliberately linear workflow just to satisfy the threshold", "do not extract helpers that hide the same branchy behavior behind vague names"], "the behavior is split into smaller named units, or a current decision note gives a finding-specific reason this symbol resists extraction — the decomposition considered + why it is wrong here, not a restatement of its size"),
    ("complex_symbol", "A symbol with high branch count, nesting, exits, arguments, or awaits concentrates decision pressure even when it is not physically huge. The metric is an inspection router, not a refactor verdict.", &["read the named symbol and map its branches to phases, modes, input states, and failure paths", "check whether the branchiness belongs in smaller named helpers, child intents, a strategy table, or explicit validations", "inspect direct tests for the riskiest branches before changing structure"], &["do not flatten meaningful domain branching into vague helpers just to reduce a number", "do not treat generated dispatch or parser tables as defects without reading their role"], "the risky decisions are named/proven in smaller units, or a current decision explains why this exact control-flow shape is deliberate"),
    ("hub_file", "A heavily imported file is a dependency hub: small changes can ripple through many responsibilities. Centrality can be deliberate, but it should be visible.", &["read the hub file's public surface and imports", "check whether it is stable shared vocabulary/config or an accidental utility grab-bag", "inspect owning intents before splitting or moving code"], &["do not split stable primitives only because they are popular", "do not hide broad dependency reach by adding graph edges without naming the contract"], "the hub is deliberately stable/shared, or broad utility code is split/reowned so imports point at narrower modules"),
    ("oversized_file", "A file large enough to be a god-file usually concentrates unrelated responsibilities; physical size is only a suspicion, so inspect the file before splitting.", &["read the file's table of contents (its top-level symbols) and ask whether they are one cohesive module or several mashed together", "check whether the size is incidental (one large generated/protocol block) or structural (many intents ground here)", "prefer splitting along intent/module lines so each new file owns one responsibility"], &["do not split a file that is large for a single deliberate reason (a protocol, a big match, generated code) just to beat the threshold", "do not move the problem: a mechanical split that leaves the same intents co-owning the new files just scatters the god-file"], "the file is split along intent/module lines so each new file owns one responsibility, or a current decision note gives a finding-specific reason this file must stay whole — the split considered + why it is wrong here, not a restatement of its size"),
    ("panic_marker_risk", "A panic/unwrap/expect/todo marker in non-test behavior can turn an expected sad path into a process abort or unfinished path; it needs an explicit boundary decision.", &["read the symbol and identify whether each marker is on trusted setup code, impossible state, or user/repo input", "check whether the owning intent has a sad/fallback validation for the marker's failure mode", "replace accidental aborts with typed errors or record why the abort is the contract"], &["do not blindly replace every unwrap; first classify invariant versus recoverable failure", "do not accept todo/unimplemented in implemented behavior without an explicit needs_change or decision"], "recoverable failures are handled/proven, unfinished markers are removed or moved to planned work, or a current file decision explains why the abort marker is deliberate"),
    ("layering_violation", "A recorded relationship does not excuse dependency direction; layer order judges whether imports point the right way.", &["`loom layer list`", "read the upward import named in evidence", "decide whether to invert, extract lower shared code, redeclare layers, or record a deliberate exception"], &["do not silence an up-dependency by adding RELATES_TO; direction is a separate norm"], "the dependency points down, the layer order is corrected, or a current decision on the importing intent justifies the exception"),
    (KIND_ARCH_VERDICT_CONTRADICTS, "A passing architecture-category rule and an open layering violation on the same intent contradict each other — one is wrong. Governance and the mechanical layer check must agree.", &["read the layering_violation: does the dependency really point up the declared order?", "read the architecture rule's recorded evidence: did it actually check dependency direction?"], &["do not leave a green architecture verdict standing over a known layering violation"], "the architecture verdict is re-recorded to match reality, the layer order is corrected, or a decision note justifies the exception"),
    ("happy_path_only", "The non-sunny states of a behavior are real only when realized, grounded, and proven; naming the trigger (a 'happy' behavior or a 'populated' UI state) without its required siblings is not enough. Two families: behavioral happy → sad/fallback, and UI-state populated → empty/error (loading is recognized but not required).", &["inspect the parent's aspect-tagged children for the triggered family", "check lifecycle, IMPLEMENTS groundings, and passed validations for each required sibling (sad/fallback, or empty/error)", "add or prove the missing path/state, or record why it is not applicable"], &["do not clear the debt with planned or unproven child intents"], "the required sibling paths/states are implemented, grounded, and directly proven, or a current decision explains why they are not required"),
    ("unused_rule", "A rule connected to nothing is not a quality bar; it is dormant policy text.", &["`loom rule list`", "find the highest honest intent surface the rule should govern", "apply it with `loom rule verdict` or delete it if it was a mistake"], &["do not keep unused rules as implied standards"], "the rule governs at least one relevant intent, or it is removed as unused policy"),
    ("vocab_drift", "Near-synonym vocabulary terms split the collision signal that duplicate detection depends on.", &["`loom vocab list`", "compare the two term definitions and their tagged intents", "merge synonyms or rename/retag to make the distinction sharp"], &["do not let agents choose between look-alike terms for the same concept"], "the look-alike terms are merged, or the remaining terms have names and definitions that no longer collide"),
    ("unjourneyed_surface", "User-visible code needs passed consumer-journey proof; per-leaf tests and declared-but-unrun sagas do not prove the composed experience.", &["`loom saga list`", "inspect whether a passed saga step binds to this intent or its relevant tree path", "add and pass a saga, mark visibility internal, or record why no journey can exercise it"], &["do not treat user_visible as proven by unit coverage alone"], "a passed consumer saga covers the surface through the tree, or a current ruling explains why it is not consumer-reachable"),
    ("hypothesis_accumulation", "A hypothesis is a falsifiable proof item, not long-term memory; accumulation teaches the next LLM to prove, reject, or adopt instead of stockpiling ideas.", &["`loom next --mode prove` for the highest-blast-radius proposal", "`loom hypothesis list --status proposed` to batch the backlog", "target untargeted hypotheses before proving, or reject them if they are not actionable"], &["do not add another hypothesis when existing proposals are unproven", "do not convert speculative notes into planned work before proof"], "the proposed backlog is below the threshold and no proposed hypothesis is stale; each old idea is supported, refuted, adopted, or rejected with evidence"),
    ("symbol_accountability_gap", "Behavior-significant symbols should be owned, accepted, or turned into explicit work; raw helper coverage is not the target.", &["`loom coverage --json` and read actionable_symbol_gaps", "`loom codefile show <path>` for each top gap before changing graph ownership", "decide whether the symbol needs a precise locator, a split intent, or a substantive decision note accepting broad ownership"], &["do not chase 100% raw symbol coverage", crate::db::queries::symbol_accountability::ANTI_CREATE_INTENTS_PER_HELPER, crate::db::queries::symbol_accountability::ANTI_BULK_GROUND_SYMBOLS], "actionable symbol gaps are grounded with precise locators, accepted with a current substantive decision note, or converted into real intent split/build work"),
    ("dependency_cycle", "RELATES_TO is semantically undirected, so a relationship belongs in ONE row. Two GROUNDED rows for the same pair (a→b and b→a both carry a verdict) store the relationship twice: it double-counts in degree/centrality and skews `loom next` ranking, and the two verdicts can silently disagree. Exactly one direction is the redundant/incidental one to retire — unless the mutuality is a deliberate peer contract.", &["`loom edge show rt:<a>:<b>` and `loom edge show rt:<b>:<a>` — compare the two verdicts and criteria", "read both intents' criteria; decide which way the dependency actually runs", "check whether the two are really one responsibility (a merge) or genuine mutual peers (a deliberate contract)"], &["do not keep both directions as the default; do not 'fix' an uninspected saga round-trip flow as if it were graph-hygiene debt (those edges are never flagged here — only deliberately grounded pairs are)"], "the redundant direction is marked independent (or the two intents merged), or a current decision note on the smaller-id intent records why both directions deliberately hold"),
    ("transitive_layering_violation", "The direct layering check exempts unlayered intents, so an illegal up-the-order dependency can hide by routing THROUGH them — every single hop looks clean, but the composed path makes a deeper layer depend on a shallower one. Direction is violated end-to-end even though no one import is.", &["read the path in the evidence — the unlayered intermediate(s) are where the violation hides", "decide: should the intermediate carry a layer (arming the direct check), or is the end-to-end dependency itself wrong?", "fix the real direction (move shared code down / invert), or `loom layer order` the intermediate, or record a deliberate exception on the importing intent"], &["do not assume an all-clean-hops path is fine — the order is violated across the whole chain; do not silence it by adding a relationship"], "the end-to-end dependency points down (or the intermediate is layered so the direct check governs the hop), or a current decision note on the importing intent justifies it"),
    ("intent_island", "Every intent should reach a system-level purpose through HIERARCHY or RELATES_TO; an island has no such path, so nothing in the graph explains why it exists in this product.", &["read the island members and find which existing branch they belong under", "`loom edge hierarchy <parent> <child>` to attach the island to its real parent, or `loom edge explore <a> <b>` to ground a relationship into the connected graph", "if the island is a genuinely separate top-level purpose, add or confirm a system-level intent for it"], &["do not leave orphaned intents floating; do not attach an island to an unrelated parent just to silence the finding"], "every island member reaches a system-level root through HIERARCHY or RELATES_TO, or a current decision note records why the disconnected subgraph is intentional"),
];

fn teaching_for(kind: &str) -> SmellTeaching {
    // recurrent_trouble is computed, not data — keep it as a call.
    if kind == "recurrent_trouble" {
        return recurrent_teaching("edge", "<id>");
    }
    let (principle, inspect, avoid, done_when) = TEACHING_TABLE
        .iter()
        .find(|(k, ..)| *k == kind)
        .map(|(_, p, i, a, d)| (*p, *i, *a, *d))
        .unwrap_or((
            "This smell is a computed suspicion; inspect the named graph and code evidence before changing behavior.",
            &["read the evidence and run the remedy command with concrete evidence"][..],
            &["do not silence the finding without a structural fix or decision note"][..],
            "the finding is fixed or adjudicated through its remedy",
        ));
    SmellTeaching {
        principle: principle.into(),
        inspect: inspect.iter().map(|s| (*s).to_string()).collect(),
        avoid: avoid.iter().map(|s| (*s).to_string()).collect(),
        done_when: done_when.into(),
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

fn lexical_signal_tokens<'a>(
    intents: &'a [crate::types::Intent],
    toks: &HashMap<&'a str, HashSet<String>>,
) -> HashMap<&'a str, HashSet<String>> {
    let max_carriers = (intents.len() / 50).max(LEXICAL_SIGNAL_TOKEN_CARRIER_FLOOR);
    let mut carriers: HashMap<&str, usize> = HashMap::new();
    for set in toks.values() {
        for token in set {
            *carriers.entry(token.as_str()).or_insert(0) += 1;
        }
    }
    intents
        .iter()
        .map(|intent| {
            let filtered = toks
                .get(intent.id.as_str())
                .map(|set| {
                    set.iter()
                        .filter(|token| {
                            carriers.get(token.as_str()).copied().unwrap_or(0) <= max_carriers
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (intent.id.as_str(), filtered)
        })
        .collect()
}

pub struct SmellInputs<'a> {
    pub notes: &'a [Note],
    pub vocab_terms: &'a [VocabTerm],
    pub layer_order: &'a [String],
    pub proposed_hypotheses: &'a [Hypothesis],
    pub targets: &'a [TargetsEdge],
}

/// Adjudication lookup shared by the extracted per-detector helpers: a
/// kind=decision note keyed on the finding identity (`<kind>:<target>`) and
/// newer than `anchor` resolves the finding. Lifted from
/// `compute_smells_from_parts`' closure so each `detect_*` helper can call it.
fn adjudicate<'a>(
    last_decision: &HashMap<&str, &'a crate::types::Note>,
    kind: &str,
    target: &str,
    anchor: &str,
) -> Option<&'a crate::types::Note> {
    let finding_id = format!("{kind}:{target}");
    let note = last_decision
        .get(finding_id.as_str())
        .filter(|n| rfc3339_after(n.created_at.as_str(), anchor))
        .copied()?;
    let prior: Vec<(&str, &str)> = last_decision
        .iter()
        .map(|(target, note)| (*target, note.text.as_str()))
        .collect();
    crate::gate::green_adjudication_ruling_is_valid(note.text.as_str(), finding_id.as_str(), &prior)
        .then_some(note)
}

/// Shared lookup state for the detectors, built once per run. Lets each plane
/// dispatcher call its detectors without threading ~20 maps through the
/// orchestrator, keeping `compute_smells_from_parts` a thin sequence of plane
/// calls. Owns the derived maps; borrows the snapshot/inputs/discovery they
/// point into (all live for the whole call).
struct SmellCtx<'a> {
    snapshot: &'a QuerySnapshot,
    discovery: &'a DiscoverySnapshot,
    intents: &'a [crate::types::Intent],
    implements: &'a [crate::types::Implements],
    hierarchy: &'a [(String, String)],
    relates: &'a [crate::types::RelatesTo],
    rules: &'a [crate::types::QualityRule],
    governs: &'a [crate::types::Governs],
    notes: &'a [crate::types::Note],
    vocab_terms: &'a [crate::types::VocabTerm],
    proposed_hypotheses: &'a [crate::types::Hypothesis],
    targets: &'a [crate::types::TargetsEdge],
    layer_order: &'a [String],
    rule_kind: HashMap<&'a str, &'a str>,
    linked: HashSet<(&'a str, &'a str)>,
    files_of: HashMap<&'a str, HashSet<&'a str>>,
    intents_on_file: HashMap<&'a str, Vec<&'a str>>,
    name_of: HashMap<&'a str, &'a str>,
    signal_toks: HashMap<&'a str, HashSet<String>>,
    last_decision: HashMap<&'a str, &'a crate::types::Note>,
    newest_grounding: HashMap<&'a str, &'a str>,
    newest_claim: HashMap<&'a str, &'a str>,
    roots: Vec<&'a crate::types::Intent>,
    intents_by_level: HashMap<&'a str, Vec<&'a crate::types::Intent>>,
}

/// Build the shared lookup context — the former inline setup of
/// `compute_smells_from_parts`, lifted out so the orchestrator stays small.
fn build_smell_ctx<'a>(
    snapshot: &'a QuerySnapshot,
    inputs: &SmellInputs<'a>,
    discovery: &'a DiscoverySnapshot,
) -> SmellCtx<'a> {
    let intents = &snapshot.intents;
    let implements = &snapshot.implements;
    let rules = &snapshot.rules;
    let rule_kind: HashMap<&str, &str> = rules
        .iter()
        .map(|r| (r.id.as_str(), r.kind.as_str()))
        .collect();
    let linked: HashSet<(&str, &str)> = discovery
        .linked
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    // File ownership excludes deprecated intents (retire leaves IMPLEMENTS in
    // place; the ownership maps must match every detector's status filter).
    let active_ids: HashSet<&str> = intents
        .iter()
        .filter(|i| i.status != "deprecated")
        .map(|i| i.id.as_str())
        .collect();
    let mut files_of: HashMap<&str, HashSet<&str>> = HashMap::new();
    let intents_on_file: HashMap<&str, Vec<&str>> = discovery
        .intents_on_file
        .iter()
        .map(|(path, ids)| {
            (
                path.as_str(),
                ids.iter()
                    .map(|id| id.as_str())
                    .filter(|id| active_ids.contains(id))
                    .collect::<Vec<&str>>(),
            )
        })
        .collect();
    for im in implements {
        if !active_ids.contains(im.intent_id.as_str()) {
            continue;
        }
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
    let signal_toks = lexical_signal_tokens(intents, &toks);
    let mut last_decision: HashMap<&str, &crate::types::Note> = HashMap::new();
    for n in inputs.notes {
        if n.kind == "decision" && !n.target_id.is_empty() {
            let e = last_decision.entry(n.target_id.as_str()).or_insert(n);
            if n.created_at > e.created_at {
                *e = n;
            }
        }
    }
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
    let is_child: HashSet<&str> = snapshot.hierarchy.iter().map(|(_, c)| c.as_str()).collect();
    let mut roots: Vec<&crate::types::Intent> = intents
        .iter()
        .filter(|i| i.status != "deprecated" && !is_child.contains(i.id.as_str()))
        .collect();
    roots.sort_by_key(|i| (i.abstraction_level != "system", i.name.clone()));
    let mut intents_by_level: HashMap<&str, Vec<&crate::types::Intent>> = HashMap::new();
    for intent in intents
        .iter()
        .filter(|intent| intent.status != "deprecated")
    {
        intents_by_level
            .entry(intent.abstraction_level.as_str())
            .or_default()
            .push(intent);
    }
    SmellCtx {
        snapshot,
        discovery,
        intents,
        implements,
        hierarchy: &snapshot.hierarchy,
        relates: &snapshot.relates,
        rules,
        governs: &snapshot.governs,
        notes: inputs.notes,
        vocab_terms: inputs.vocab_terms,
        proposed_hypotheses: inputs.proposed_hypotheses,
        targets: inputs.targets,
        layer_order: inputs.layer_order,
        rule_kind,
        linked,
        files_of,
        intents_on_file,
        name_of,
        signal_toks,
        last_decision,
        newest_grounding,
        newest_claim,
        roots,
        intents_by_level,
    }
}

/// Snapshot + input reusing form for storage backends that can load the read
/// planes directly.
pub fn compute_smells_from_parts(
    snapshot: &QuerySnapshot,
    inputs: SmellInputs<'_>,
) -> Result<SmellReport> {
    let discovery = DiscoverySnapshot::from_query(snapshot)?;
    let ctx = build_smell_ctx(snapshot, &inputs, &discovery);
    let mut smells: Vec<Smell> = Vec::new();
    let mut adjudicated_out: Vec<AdjudicatedSmell> = Vec::new();

    // Run each plane's detectors. Order is cosmetic — `smells` is sorted below.
    semantic::detect_semantic_plane(&ctx, &mut smells, &mut adjudicated_out);
    physical::detect_physical_plane(&ctx, &mut smells, &mut adjudicated_out);
    coupling::detect_coupling_plane(&ctx, &mut smells, &mut adjudicated_out);
    normative::detect_normative_plane(&ctx, &mut smells)?;
    lifecycle::detect_lifecycle_plane(&ctx, &mut smells, &mut adjudicated_out);
    consumer::detect_consumer_plane(&ctx, &mut smells, &mut adjudicated_out);

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
    let coded_intents = ctx
        .intents
        .iter()
        .filter(|i| ctx.files_of.contains_key(i.id.as_str()))
        .count();
    let tagged_coded_intents = ctx
        .intents
        .iter()
        .filter(|i| {
            ctx.files_of.contains_key(i.id.as_str())
                && ctx
                    .discovery
                    .tags_by_intent
                    .get(i.id.as_str())
                    .is_some_and(|t| !t.is_empty())
        })
        .count();
    let coded_layers = ctx
        .intents
        .iter()
        .filter(|i| ctx.files_of.contains_key(i.id.as_str()) && !i.layer.is_empty())
        .map(|i| i.layer.as_str())
        .collect::<HashSet<_>>()
        .len();
    let declared_layers = ctx.layer_order.len();
    // LOC/size smells (oversized_file, large_behavioral_symbol) are ADVISORY: a
    // coarse flag for the LLM to inspect case-by-case, never a green gate.
    // Partition them out of `open` (which phase/ladder gating consumes) into
    // `advisory` (surfaced, non-gating) — see SIZE_ADVISORY_KINDS.
    // Three-way split: dischargeable metadata DEBT and size ADVISORIES are both
    // surfaced but kept OUT of `open` (the set phase/ladder gating consumes), so
    // neither blocks green — debt because hard-gating it pressures laundering,
    // advisories because they are coarse case-by-case prompts.
    let mut debt = Vec::new();
    let mut advisory = Vec::new();
    let mut open = Vec::new();
    for s in smells {
        if DEBT_KINDS.contains(&s.kind.as_str()) {
            debt.push(s);
        } else if SIZE_ADVISORY_KINDS.contains(&s.kind.as_str()) {
            advisory.push(s);
        } else {
            open.push(s);
        }
    }
    Ok(SmellReport {
        open,
        debt,
        advisory,
        adjudicated: adjudicated_out,
        coded_intents,
        tagged_coded_intents,
        coded_layers,
        declared_layers,
    })
}

/// `cochange_coupling` suggestions — the ADVISORY, git-derived counterpart to
/// `undeclared_coupling`. Intent pairs whose files keep changing together
/// (temporal coupling) but that have no recorded RELATES_TO relationship.
/// Computed OUTSIDE `compute_smells_from` (which is the green-gating, git-free
/// audit path): these are hints to investigate, never gate `phase=complete`,
/// and the git pass runs only in the `loom smells` command. Takes the raw
/// co-change maps (from `repo::git_cochange`) so the logic is testable without
/// a real repo. Empty when there's no history or nothing crosses the threshold.
pub fn cochange_suggestions(
    snapshot: &QuerySnapshot,
    pairs: &HashMap<(String, String), usize>,
    individual: &HashMap<String, usize>,
) -> Vec<Smell> {
    /// Co-change at least this many times before it's worth a look.
    const MIN_COCHANGE: usize = 3;
    /// And with at least this confidence: co-changes / min(individual changes).
    /// Filters two churny files that merely overlap from a real "they move
    /// together" contract.
    const MIN_CONFIDENCE: f64 = 0.5;
    if pairs.is_empty() {
        return Vec::new();
    }
    let active: HashSet<&str> = snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let name_of: HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.name.as_str()))
        .collect();
    let mut intents_on_file: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &snapshot.implements {
        intents_on_file
            .entry(im.codefile_path.as_str())
            .or_default()
            .push(im.intent_id.as_str());
    }
    // Already-related pairs (RELATES_TO any status + HIERARCHY), both directions.
    let mut linked: HashSet<(&str, &str)> = HashSet::new();
    for e in &snapshot.relates {
        linked.insert((e.from_id.as_str(), e.to_id.as_str()));
        linked.insert((e.to_id.as_str(), e.from_id.as_str()));
    }
    for (p, c) in &snapshot.hierarchy {
        linked.insert((p.as_str(), c.as_str()));
        linked.insert((c.as_str(), p.as_str()));
    }

    // Accumulate per intent-pair: strongest co-change count, confidence, and a
    // few example file pairs.
    let mut acc: HashMap<(String, String), (usize, f64, Vec<String>)> = HashMap::new();
    for ((fa, fb), &count) in pairs {
        if count < MIN_COCHANGE {
            continue;
        }
        let denom = (*individual.get(fa).unwrap_or(&count))
            .min(*individual.get(fb).unwrap_or(&count))
            .max(1);
        let confidence = count as f64 / denom as f64;
        if confidence < MIN_CONFIDENCE {
            continue;
        }
        let (Some(owners_a), Some(owners_b)) = (
            intents_on_file.get(fa.as_str()),
            intents_on_file.get(fb.as_str()),
        ) else {
            continue;
        };
        for a in owners_a {
            for b in owners_b {
                if a == b
                    || !active.contains(a)
                    || !active.contains(b)
                    || linked.contains(&(*a, *b))
                {
                    continue;
                }
                let key = if a < b {
                    (a.to_string(), b.to_string())
                } else {
                    (b.to_string(), a.to_string())
                };
                let entry = acc.entry(key).or_insert((0, 0.0, Vec::new()));
                entry.0 = entry.0.max(count);
                if confidence > entry.1 {
                    entry.1 = confidence;
                }
                let example = format!("{fa} ↔ {fb} ({count}×)");
                if entry.2.len() < 3 && !entry.2.contains(&example) {
                    entry.2.push(example);
                }
            }
        }
    }
    let mut out: Vec<Smell> = acc
        .into_iter()
        .map(|((a, b), (count, confidence, examples))| {
            let na = name_of.get(a.as_str()).copied().unwrap_or(a.as_str());
            let nb = name_of.get(b.as_str()).copied().unwrap_or(b.as_str());
            Smell {
                kind: "cochange_coupling".into(),
                score: count as f64 * confidence,
                summary: format!(
                    "'{na}' and '{nb}' keep changing together in git but have no recorded relationship"
                ),
                evidence: format!(
                    "co-change: {} · confidence {:.0}%",
                    examples.join(", "),
                    confidence * 100.0
                ),
                remedy: format!(
                    "loom edge explore {a} {b}  → history says they're coupled; ground the relationship or mark it independent"
                ),
                teaching: teaching_for("cochange_coupling"),
            }
        })
        .collect();
    out.sort_by(|x, y| {
        y.score
            .partial_cmp(&x.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.summary.cmp(&y.summary))
            .then_with(|| x.remedy.cmp(&y.remedy))
            .then_with(|| x.evidence.cmp(&y.evidence))
    });
    out
}

/// `shotgun_surgery` suggestions — the higher-level sibling of
/// `cochange_coupling`. Instead of serving one intent pair, it flags an intent
/// whose owned files repeatedly co-change with MANY unrelated owned files. This
/// is git-derived excellence signal: it does not block Production-ready, but it
/// keeps maintenance debt visible for the Excellent profile.
pub fn shotgun_surgery_suggestions(
    snapshot: &QuerySnapshot,
    pairs: &HashMap<(String, String), usize>,
    individual: &HashMap<String, usize>,
) -> Vec<Smell> {
    const MIN_COCHANGE: usize = 3;
    const MIN_CONFIDENCE: f64 = 0.45;
    const MIN_PARTNER_INTENTS: usize = 4;
    if pairs.is_empty() {
        return Vec::new();
    }

    let active: HashSet<&str> = snapshot.intents.iter().map(|i| i.id.as_str()).collect();
    let name_of: HashMap<&str, &str> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i.name.as_str()))
        .collect();
    let mut intents_on_file: HashMap<&str, Vec<&str>> = HashMap::new();
    for im in &snapshot.implements {
        intents_on_file
            .entry(im.codefile_path.as_str())
            .or_default()
            .push(im.intent_id.as_str());
    }
    let mut linked: HashSet<(&str, &str)> = HashSet::new();
    for e in &snapshot.relates {
        linked.insert((e.from_id.as_str(), e.to_id.as_str()));
        linked.insert((e.to_id.as_str(), e.from_id.as_str()));
    }
    for (p, c) in &snapshot.hierarchy {
        linked.insert((p.as_str(), c.as_str()));
        linked.insert((c.as_str(), p.as_str()));
    }

    #[derive(Default)]
    struct Acc {
        partners: HashSet<String>,
        files: HashSet<String>,
        best_count: usize,
        best_confidence: f64,
        examples: Vec<String>,
    }

    let mut acc: HashMap<String, Acc> = HashMap::new();
    for ((fa, fb), &count) in pairs {
        if count < MIN_COCHANGE {
            continue;
        }
        let denom = (*individual.get(fa).unwrap_or(&count))
            .min(*individual.get(fb).unwrap_or(&count))
            .max(1);
        let confidence = count as f64 / denom as f64;
        if confidence < MIN_CONFIDENCE {
            continue;
        }
        let (Some(owners_a), Some(owners_b)) = (
            intents_on_file.get(fa.as_str()),
            intents_on_file.get(fb.as_str()),
        ) else {
            continue;
        };
        for (owner, owner_file, partners, partner_file) in [
            (owners_a, fa.as_str(), owners_b, fb.as_str()),
            (owners_b, fb.as_str(), owners_a, fa.as_str()),
        ] {
            for a in owner {
                if !active.contains(a) {
                    continue;
                }
                for b in partners {
                    if a == b || !active.contains(b) || linked.contains(&(*a, *b)) {
                        continue;
                    }
                    let entry = acc.entry((*a).to_string()).or_default();
                    entry.partners.insert((*b).to_string());
                    entry.files.insert(owner_file.to_string());
                    entry.best_count = entry.best_count.max(count);
                    entry.best_confidence = entry.best_confidence.max(confidence);
                    let example = format!("{owner_file} ↔ {partner_file} ({count}×)");
                    if entry.examples.len() < 4 && !entry.examples.contains(&example) {
                        entry.examples.push(example);
                    }
                }
            }
        }
    }

    let mut out: Vec<Smell> = acc
        .into_iter()
        .filter(|(_, a)| a.partners.len() >= MIN_PARTNER_INTENTS)
        .map(|(intent_id, a)| {
            let name = name_of
                .get(intent_id.as_str())
                .copied()
                .unwrap_or(intent_id.as_str());
            let mut partner_names: Vec<&str> = a
                .partners
                .iter()
                .filter_map(|id| name_of.get(id.as_str()).copied())
                .collect();
            partner_names.sort();
            let mut files: Vec<String> = a.files.into_iter().collect();
            files.sort();
            let score = a.partners.len() as f64 * a.best_count as f64 * a.best_confidence;
            Smell {
                kind: "shotgun_surgery".into(),
                score,
                summary: format!(
                    "'{name}' changes with {} unrelated intent(s) in git history",
                    a.partners.len()
                ),
                evidence: format!(
                    "owned file(s): {} · partner intents: {} · examples: {} · best confidence {:.0}%",
                    files.join(", "),
                    partner_names.join(" · "),
                    a.examples.join(", "),
                    a.best_confidence * 100.0
                ),
                remedy: format!(
                    "inspect '{name}' and its co-changing files; ground real hidden contracts with `loom edge explore`, split/reown broad behavior through a proven hypothesis, or record incidental history with `loom note add --intent {intent_id} --kind decision --text \"<why this recurring wide co-change is incidental>\"`"
                ),
                teaching: teaching_for("shotgun_surgery"),
            }
        })
        .collect();
    out.sort_by(|x, y| {
        y.score
            .partial_cmp(&x.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.summary.cmp(&y.summary))
    });
    out
}

/// ADVISORY and never gates `phase=complete`.
pub fn proof_locality_suggestions(snapshot: &QuerySnapshot) -> Vec<Smell> {
    proof_locality_from_parts(
        &snapshot.intents,
        &snapshot.implements,
        &snapshot.validates,
        &snapshot.validations,
        &snapshot.codefiles,
        &snapshot.hierarchy,
    )
}

fn proof_locality_from_parts(
    intents: &[crate::types::Intent],
    implements: &[crate::types::Implements],
    validates: &[crate::types::ValidatesEdge],
    validations: &[crate::types::Validation],
    codefiles: &[crate::types::CodeFile],
    hierarchy: &[(String, String)],
) -> Vec<Smell> {
    const MAX_SUGGESTIONS: usize = 25;

    // Test-symbol index: every is_test symbol's name → its file. Empty means the
    // graph carries no symbol facts (pre-v8 / feature-light build) — the
    // instrument is unarmed, so flag nothing rather than everything.
    let mut test_symbols: Vec<(&str, &str)> = Vec::new(); // (symbol name, file path)
    for cf in codefiles {
        for f in &cf.symbol_facts {
            if f.is_test {
                test_symbols.push((f.name.as_str(), cf.path.as_str()));
            }
        }
    }
    if test_symbols.is_empty() {
        return Vec::new();
    }
    let all_paths: Vec<&str> = codefiles.iter().map(|c| c.path.as_str()).collect();

    let name_of: HashMap<&str, &str> = intents
        .iter()
        .map(|i| (i.id.as_str(), i.name.as_str()))
        .collect();
    let is_parent: HashSet<&str> = hierarchy.iter().map(|(p, _)| p.as_str()).collect();

    let mut grounded: HashMap<&str, HashSet<&str>> = HashMap::new();
    for im in implements {
        grounded
            .entry(im.intent_id.as_str())
            .or_default()
            .insert(im.codefile_path.as_str());
    }
    let val_by_id: HashMap<&str, &crate::types::Validation> =
        validations.iter().map(|v| (v.id.as_str(), v)).collect();
    let mut passing: HashMap<&str, Vec<&crate::types::Validation>> = HashMap::new();
    for e in validates {
        if let Some(v) = val_by_id.get(e.validation_id.as_str()) {
            if v.last_result == "passed" {
                passing.entry(e.intent_id.as_str()).or_default().push(v);
            }
        }
    }

    let mut out: Vec<Smell> = Vec::new();
    for i in intents {
        if i.status == "deprecated" {
            continue;
        }
        let lifecycle = if i.lifecycle.is_empty() {
            "implemented"
        } else {
            i.lifecycle.as_str()
        };
        if lifecycle != "implemented" || is_parent.contains(i.id.as_str()) {
            continue; // the proven axis counts implemented LEAVES only
        }
        let Some(g) = grounded.get(i.id.as_str()) else {
            continue; // not grounded → not realized → not our concern
        };
        let Some(vals) = passing.get(i.id.as_str()) else {
            continue; // not proven
        };
        // Exempt: any non-test proof exercises code we can't statically see.
        if vals.iter().any(|v| v.validation_type != "test") {
            continue;
        }

        // Locality is MODULE-level, not file-level: Rust keeps a module's tests
        // beside its code (in `mod.rs` / a `#[cfg(test)] mod tests`), not in the
        // same file. A test in the grounded code's own directory counts as local;
        // only a proof living in a DIFFERENT module is the overstatement we flag.
        let grounded_dirs: HashSet<&str> = g.iter().map(|p| parent_dir(p)).collect();
        let mut located_any = false;
        let mut local = false;
        for v in vals {
            let files = locate_test_proof(&v.command, &test_symbols, &all_paths);
            if files.is_empty() {
                continue; // unresolvable selector → unknown, not non-local
            }
            located_any = true;
            if files.iter().any(|p| grounded_dirs.contains(parent_dir(p))) {
                local = true;
                break;
            }
        }
        if located_any && !local {
            let mut gfiles: Vec<&str> = g.iter().copied().collect();
            gfiles.sort_unstable();
            let cmds: Vec<&str> = vals.iter().map(|v| v.command.as_str()).collect();
            out.push(Smell {
                kind: "nonlocal_proof".into(),
                score: gfiles.len() as f64,
                summary: format!(
                    "'{}' reads as proven, but its only test proof lives outside its grounded module",
                    name_of.get(i.id.as_str()).copied().unwrap_or(i.name.as_str())
                ),
                evidence: format!(
                    "grounded in [{}] · proven only by test(s) [{}] that resolve to OTHER modules — the grounded code may have no test in its own module (partial-coverage overstatement of the `proven` axis)",
                    gfiles.join(", "),
                    cmds.join("; ")
                ),
                remedy: format!(
                    "add a test that exercises {} and link it (`loom validation add … --type test` + `loom edge validates <validation> {}`), fix the IMPLEMENTS locator if the grounding is wrong, or accept it (`loom note add --intent {} --kind decision --text \"<why the existing proof covers this code>\"`)",
                    gfiles.first().copied().unwrap_or("the grounded file"),
                    i.id,
                    i.id
                ),
                teaching: teaching_for("nonlocal_proof"),
            });
        }
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.summary.cmp(&b.summary))
    });
    out.truncate(MAX_SUGGESTIONS);
    out
}

/// Resolve a `cargo test` command to the source files its selectors live in,
/// using the test-symbol index. Empty set = "can't tell" (opaque command, bare
/// `cargo test`, a script) — the caller treats that as UNKNOWN, never non-local.
fn locate_test_proof<'a>(
    command: &str,
    test_symbols: &[(&'a str, &'a str)],
    all_paths: &[&'a str],
) -> HashSet<&'a str> {
    let mut files: HashSet<&str> = HashSet::new();
    for sel in parse_cargo_test_selectors(command) {
        if sel.contains("::") {
            // module-path selector (a::b[::tests]) → a source-path fragment.
            let sel = sel.trim_end_matches("::");
            let modpath = sel.strip_suffix("::tests").unwrap_or(sel);
            let frag = modpath.replace("::", "/");
            for p in all_paths {
                if path_matches_module(p, &frag) {
                    files.insert(p);
                }
            }
        } else {
            // bare filter: cargo runs every test whose name CONTAINS the substring.
            for (name, path) in test_symbols {
                if name.contains(&sel) {
                    files.insert(path);
                }
            }
        }
    }
    files
}

/// Extract the test-name / module selectors from a `cargo test …` command,
/// dropping flags, their consumed values, the `--` separator, and shell
/// redirections. Non-cargo commands (scripts, binaries) yield nothing.
fn parse_cargo_test_selectors(command: &str) -> Vec<String> {
    if !command.contains("cargo test") {
        return Vec::new();
    }
    let mut selectors = Vec::new();
    let mut seen_test = false;
    let mut skip_next = false;
    for tok in command.split_whitespace() {
        if !seen_test {
            if tok == "test" {
                seen_test = true;
            }
            continue;
        }
        // Stop at a shell redirection / pipe — nothing past it is a selector.
        if tok.starts_with('>')
            || tok.starts_with("2>")
            || tok.starts_with("1>")
            || tok == "|"
            || tok == "&&"
            || tok == ";"
        {
            break;
        }
        if skip_next {
            skip_next = false;
            continue;
        }
        if tok == "--" {
            continue;
        }
        if tok.starts_with('-') {
            if matches!(
                tok,
                "--bin"
                    | "-p"
                    | "--package"
                    | "--example"
                    | "--test"
                    | "--features"
                    | "--manifest-path"
                    | "--target"
            ) {
                skip_next = true; // the next token is this flag's value, not a selector
            }
            continue;
        }
        selectors.push(tok.to_string());
    }
    selectors
}

/// A source path matches a module fragment when the fragment is a path segment
/// (dir `…/frag/…`, file `…/frag.rs`, or module file `…/frag/mod.rs`). Segment
/// matching avoids the over-match a bare `contains` gives a short module name.
fn path_matches_module(path: &str, frag: &str) -> bool {
    path.contains(&format!("/{frag}/"))
        || path.ends_with(&format!("/{frag}.rs"))
        || path == format!("{frag}.rs")
        || path.ends_with(&format!("/{frag}/mod.rs"))
}

/// The directory a file lives in — its module, for locality comparison.
/// `"src/db/queries/intent.rs"` → `"src/db/queries"`; a bare name → `""`.
fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

// ---------------------------------------------------------------------------
// deletion safety — resolve what a delete candidate SERVES before removing it
// ---------------------------------------------------------------------------

/// Prepended to the remedy of every finding that suggests REMOVING code. The
/// deletion-class remedies operate at the physical plane (a duplicated shape, a
/// repeated string); without this an LLM can delete a copy on shape alone,
/// bypassing the intent graph that is loom's whole reason to exist — and worse,
/// can read ABSENCE of a grounding as "dead" when loom's own coverage is
/// known-incomplete (`symbol_accountability_gap`). The per-site intent context
/// this points at is computed by `DeletionContext` and folded into the
/// finding's evidence.
pub(crate) const DELETION_SAFETY_PREAMBLE: &str = "DELETION SAFETY — removing a copy is irreversible, so resolve what each copy SERVES first (per-copy intent context is in this finding's evidence): KEEP the copy grounded to a live intent; an UNGROUNDED copy whose file or importers are owned is a COVERAGE GAP, not dead code (loom's map is known-incomplete) — ground it (`loom edge implement <intent> <file> --locator \"<symbol>\"`), do NOT delete it; a copy grounded only to a non-active intent, or with no surrounding intent at all, is a removal candidate ONLY if it should never have existed. Then ";

/// Lighter sibling for `string_contract_duplicate`, where the move is
/// CONSOLIDATION rather than deletion, but the same trap applies: fold copies
/// into one source only after you know which intent each copy serves.
pub(crate) const STRING_CONTRACT_SAFETY_PREAMBLE: &str = "DELETION SAFETY — before folding copies into one source, know which intent each copy SERVES (per-copy intent context is in this finding's evidence): a copy living in an UNGROUNDED-but-owned symbol is a coverage gap, not dead text — centralizing must not silently drop a surface it serves. Then ";

/// What the intent graph knows about ONE deletion candidate — a clone copy or a
/// repeated-string site. Absence of a grounding is NEVER evidence the code is
/// dead: loom's own coverage is known-incomplete, so an ungrounded symbol whose
/// file/importers are owned is a gap to fill, not a removal candidate.
#[derive(Debug, Clone)]
pub(crate) enum SiteIntent {
    /// A current IMPLEMENTS edge on a LIVE intent anchors this symbol (precise
    /// locator) or its whole file (file-level grounding). Entries are
    /// `"name" (lifecycle)`. Keep the copy whose grounding is live.
    Live(Vec<String>),
    /// The only groundings covering this symbol point at intent ids NOT in the
    /// active set — a retired/superseded intent OR a dangling edge. NOT a delete
    /// green-light: confirm which before removing.
    NonActiveGrounding,
    /// Nothing live grounds the symbol, but the file (or a file importing it) is
    /// owned by live intent(s): the grounding is MISSING, not absent — fill it.
    CoverageGap(Vec<String>),
    /// No live grounding and no surrounding intent. A removal candidate only if
    /// it should never have existed — and loom's coverage is incomplete.
    Isolated,
}

impl SiteIntent {
    /// One compact phrase for the evidence line.
    fn phrase(&self) -> String {
        match self {
            SiteIntent::Live(names) => format!("serves {}", names.join(", ")),
            SiteIntent::NonActiveGrounding => {
                "grounded only to a non-active intent (retired or dangling) — verify before removing"
                    .into()
            }
            SiteIntent::CoverageGap(surrounding) => format!(
                "no live grounding; file/importers owned by {} — coverage gap, ground it (don't delete)",
                surrounding.join(", ")
            ),
            SiteIntent::Isolated => "no live grounding and no surrounding intent".into(),
        }
    }
}

/// Per-(path, symbol) intent context for the deletion-class detectors, built
/// once per run. Whole-file (empty-locator) groundings cover every symbol in the
/// file; precise locators are matched with
/// `symbol_accountability::locator_covers_symbol`, the SAME word-boundary
/// primitive the accountability detector uses, so the two never disagree on what
/// "grounded" means. `snapshot.intents` is active-only, so a grounding whose
/// intent id is absent is retired-or-dangling (see `NonActiveGrounding`).
pub(crate) struct DeletionContext<'a> {
    active_by_id: HashMap<&'a str, &'a crate::types::Intent>,
    grounds_on_path: HashMap<&'a str, Vec<&'a crate::types::Implements>>,
    owners_of_path: HashMap<&'a str, Vec<&'a str>>,
    importer_owners: HashMap<&'a str, HashSet<&'a str>>,
}

impl<'a> DeletionContext<'a> {
    pub(crate) fn new(snapshot: &'a QuerySnapshot) -> Self {
        let active_by_id: HashMap<&str, &crate::types::Intent> = snapshot
            .intents
            .iter()
            .filter(|i| i.status != "deprecated")
            .map(|i| (i.id.as_str(), i))
            .collect();
        let mut grounds_on_path: HashMap<&str, Vec<&crate::types::Implements>> = HashMap::new();
        let mut owners_of_path: HashMap<&str, Vec<&str>> = HashMap::new();
        for im in &snapshot.implements {
            grounds_on_path
                .entry(im.codefile_path.as_str())
                .or_default()
                .push(im);
            if active_by_id.contains_key(im.intent_id.as_str()) {
                let owners = owners_of_path.entry(im.codefile_path.as_str()).or_default();
                if !owners.contains(&im.intent_id.as_str()) {
                    owners.push(im.intent_id.as_str());
                }
            }
        }
        // imported-path -> active intents owning a file that imports it. Imports
        // are stored as repo-relative paths matching codefile paths (the same key
        // `undeclared_coupling` resolves against).
        let mut importer_owners: HashMap<&str, HashSet<&str>> = HashMap::new();
        for cf in &snapshot.codefiles {
            let Some(owners) = owners_of_path.get(cf.path.as_str()).cloned() else {
                continue;
            };
            for target in &cf.imports {
                let entry = importer_owners.entry(target.as_str()).or_default();
                for o in &owners {
                    entry.insert(*o);
                }
            }
        }
        Self {
            active_by_id,
            grounds_on_path,
            owners_of_path,
            importer_owners,
        }
    }

    fn name_lifecycle(&self, id: &str) -> Option<String> {
        self.active_by_id.get(id).map(|i| {
            let lc = if i.lifecycle.is_empty() {
                "implemented"
            } else {
                i.lifecycle.as_str()
            };
            format!("\"{}\" ({lc})", i.name)
        })
    }

    /// Resolve the intent context for one symbol (`label`, canonical `name`) at
    /// `path`. A live direct grounding wins; otherwise surrounding intent (file
    /// owners + importer owners) makes it a coverage gap; a covering grounding to
    /// a non-active intent with no live surrounding is `NonActiveGrounding`; only
    /// the truly isolated symbol is a clean removal candidate.
    pub(crate) fn classify(&self, path: &str, label: &str, name: &str) -> SiteIntent {
        let mut live: Vec<String> = Vec::new();
        let mut non_active_cover = false;
        if let Some(grounds) = self.grounds_on_path.get(path) {
            for im in grounds {
                let covers = im.locator.trim().is_empty()
                    || crate::db::queries::symbol_accountability::locator_covers_symbol(
                        &im.locator,
                        label,
                        name,
                    );
                if !covers {
                    continue;
                }
                match self.name_lifecycle(im.intent_id.as_str()) {
                    Some(lbl) => {
                        if !live.contains(&lbl) {
                            live.push(lbl);
                        }
                    }
                    None => non_active_cover = true,
                }
            }
        }
        if !live.is_empty() {
            live.sort();
            return SiteIntent::Live(live);
        }
        let mut surrounding: Vec<String> = Vec::new();
        if let Some(owners) = self.owners_of_path.get(path) {
            for &id in owners {
                if let Some(lbl) = self.name_lifecycle(id) {
                    if !surrounding.contains(&lbl) {
                        surrounding.push(lbl);
                    }
                }
            }
        }
        if let Some(importers) = self.importer_owners.get(path) {
            for &id in importers {
                if let Some(lbl) = self.name_lifecycle(id) {
                    if !surrounding.contains(&lbl) {
                        surrounding.push(lbl);
                    }
                }
            }
        }
        if !surrounding.is_empty() {
            surrounding.sort();
            return SiteIntent::CoverageGap(surrounding);
        }
        if non_active_cover {
            return SiteIntent::NonActiveGrounding;
        }
        SiteIntent::Isolated
    }

    /// The `intent context — …` clause folded into a finding's evidence, one
    /// entry per deletion candidate. `sites` yields `(path, label, name)`.
    ///
    /// Deliberately not capped: unlike ordinary location evidence, each entry is
    /// a safety classification (`serves`, `coverage gap`, `isolated`, ...). If a
    /// high-copy clone hides a live/coverage-gap entry in an `… and N more` tail,
    /// the deletion gate has lost the exact signal it exists to preserve.
    pub(crate) fn clause<'b, I>(&self, sites: I) -> String
    where
        I: IntoIterator<Item = (&'b str, &'b str, &'b str)>,
    {
        let parts: Vec<String> = sites
            .into_iter()
            .map(|(path, label, name)| {
                format!(
                    "{path} '{label}': {}",
                    self.classify(path, label, name).phrase()
                )
            })
            .collect();
        format!("intent context — {}", parts.join(" · "))
    }
}

// ---------------------------------------------------------------------------
// code-clone advisory — structural duplication the intent graph can't see
// ---------------------------------------------------------------------------

/// `code_clone` suggestions — cross-file normalized structural clone detection,
/// the duplication the intent-level detectors are blind to by construction:
/// `twin_intents` needs shared wording, `duplicated_responsibility` needs shared
/// tags, `overlapping_ownership` needs a shared file, `undeclared_coupling`
/// needs an import — literally copy-pasted code in disjoint, untagged,
/// unimported files has none of those. The primitive is already paid for: every
/// `SymbolFact` carries a `shape_hash` (FNV-1a over normalized tree-sitter
/// tokens) that `loom sync` populates, so clone detection is GROUP BY structural
/// shape across files. Older/pre-upgrade facts fall back to `body_hash`, which
/// keeps exact-text detection armed until the next sync self-heals them.
///
/// Conservative by design (clone detection is famously noisy): a symbol is
/// skipped when its hash is empty (pre-v8 / feature-light build — the instrument
/// is unarmed for that fact), it is a test (fixtures legitimately repeat), its
/// span is below `MIN_CLONE_LINES` (trivial getters / single match arms), or its
/// file matches an ignore glob (generated/vendor/out-of-scope). Only groups
/// spanning ≥2 DISTINCT files are flagged — intra-file repetition is a different
/// concern (`tangled_file`). Computed OUTSIDE `compute_smells_from` — like
/// `cochange_suggestions` and `proof_locality_suggestions`, it is ADVISORY and
/// never gates `phase=complete`.
pub fn clone_suggestions(
    snapshot: &QuerySnapshot,
    ignore_patterns: &[glob::Pattern],
) -> Vec<Smell> {
    const MAX_SUGGESTIONS: usize = 25;

    let is_ignored = |path: &str| ignore_patterns.iter().any(|p| p.matches(path));

    // Group eligible symbols by shape_hash when available, falling back to the
    // exact body_hash for older graphs. Value: (file path, symbol fact).
    let mut by_hash: HashMap<(&str, &str), Vec<(&str, &crate::types::SymbolFact)>> = HashMap::new();
    for cf in &snapshot.codefiles {
        if is_ignored(cf.path.as_str()) {
            continue;
        }
        for f in &cf.symbol_facts {
            if f.is_test {
                continue;
            }
            let (hash_kind, hash) = if shape_hash_eligible(f) && !f.shape_hash.is_empty() {
                ("shape_hash", f.shape_hash.as_str())
            } else if !f.body_hash.is_empty() {
                ("body_hash", f.body_hash.as_str())
            } else {
                continue;
            };
            let span = f.line_end.saturating_sub(f.line_start) + 1;
            if span < MIN_CLONE_LINES {
                continue;
            }
            by_hash
                .entry((hash_kind, hash))
                .or_default()
                .push((cf.path.as_str(), f));
        }
    }

    let deletion_ctx = DeletionContext::new(snapshot);
    let mut out: Vec<Smell> = Vec::new();
    for ((hash_kind, hash), members) in by_hash {
        let distinct_files: HashSet<&str> = members.iter().map(|(p, _)| *p).collect();
        if distinct_files.len() < 2 {
            continue; // cross-file only — intra-file repetition is tangled_file's
        }
        let mut locs: Vec<(&str, &crate::types::SymbolFact)> = members.clone();
        locs.sort_by(|a, b| {
            a.0.cmp(b.0)
                .then_with(|| a.1.line_start.cmp(&b.1.line_start))
        });
        let span = locs
            .iter()
            .map(|(_, f)| f.line_end.saturating_sub(f.line_start) + 1)
            .max()
            .unwrap_or(0);
        let first_label = locs[0].1.label.as_str();
        let count = locs.len();
        let clone_phrase = if hash_kind == "shape_hash" {
            "matching code shape"
        } else {
            "identical code"
        };
        let summary = if count > 2 {
            format!(
                "{clone_phrase} in {count} locations: '{first_label}' (and {} others)",
                count - 1
            )
        } else {
            format!("{clone_phrase} in 2 locations: '{first_label}'")
        };
        let hash_label = if hash_kind == "shape_hash" {
            "normalized shape_hash"
        } else {
            "exact body_hash"
        };
        // Bound the per-finding site list: a high-copy clone otherwise floods
        // the reader with dozens of `path:lines 'label'` entries. The `count` in
        // the summary already states the true total.
        let sites: Vec<String> = locs
            .iter()
            .map(|(p, f)| format!("{}:{}-{} '{}'", p, f.line_start, f.line_end, f.label))
            .collect();
        let evidence = format!(
            "all share one {hash_label} ({} lines): {}",
            span,
            capped_join(&sites, " · ")
        );
        let intent_clause = deletion_ctx.clause(
            locs.iter()
                .map(|(p, f)| (*p, f.label.as_str(), f.name.as_str())),
        );
        let evidence = format!("{evidence} | {intent_clause}");
        out.push(Smell {
            kind: "code_clone".into(),
            score: span as f64 * count as f64,
            summary,
            evidence,
            remedy: format!(
                "{DELETION_SAFETY_PREAMBLE}read each copy and decide which of three it is: (1) coincidental shape (e.g. dispatch shims that match by accident) — leave it; (2) one responsibility copied — dedupe now, or if both copies are owned `loom edge explore <a> <b>` to ground or refute the relationship (a structural clone is evidence for a `duplicated_responsibility` merge); (3) a real dup you are DEFERRING — file it as tracked work, not a dead note: `loom hypothesis add` with the clone as the claim and the shape group collapsing to one definition as the predicted outcome, so `loom hypothesis adopt --spawned` turns it into a planned refactor the build/validate machinery owns. If the copies must stay deliberately independent, record that ruling with `loom note add --smell \"code_clone:{hash}\" --kind decision --text \"<why these copies must stay independent>\"`; the advisory moves to adjudicated until the clone's normalized shape changes"
            ),
            teaching: teaching_for("code_clone"),
        });
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.summary.cmp(&b.summary))
            // Equal score AND summary (two clone groups of the same size/shape)
            // otherwise fell back to HashMap iteration order — non-deterministic
            // across runs, churning diffs and breaking positional adjudication.
            // evidence/remedy carry the clone-specific locations: a stable break.
            .then_with(|| a.evidence.cmp(&b.evidence))
            .then_with(|| a.remedy.cmp(&b.remedy))
    });
    out.truncate(MAX_SUGGESTIONS);
    out
}

fn shape_hash_eligible(f: &crate::types::SymbolFact) -> bool {
    behavioral_symbol_kind(f.kind.as_str())
}

fn behavioral_symbol_kind(kind: &str) -> bool {
    matches!(kind, "def" | "fn" | "function" | "impl" | "method")
}

fn command_or_public_surface(path: &str, fact: &crate::types::SymbolFact) -> bool {
    path.starts_with("src/commands/")
        || path.starts_with("src/main.")
        || fact.visibility == "public"
        || fact.label.starts_with("pub ")
        || fact.label.starts_with("export ")
}

#[derive(Clone, Copy)]
struct StringContractLoc<'a> {
    path: &'a str,
    file_modified: &'a str,
    label: &'a str,
    line: usize,
    value: &'a str,
}

fn normalized_contract_string(value: &str) -> Option<String> {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = normalized.trim();
    if trimmed.len() < MIN_STRING_CONTRACT_CHARS {
        return None;
    }
    let tokens = trimmed
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .count();
    if tokens < MIN_STRING_CONTRACT_TOKENS || string_contract_is_noise(trimmed) {
        return None;
    }
    Some(trimmed.to_lowercase())
}

fn string_contract_is_noise(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("file://")
        || lower.starts_with("src/")
        || lower.ends_with(".rs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".js")
        || lower.ends_with(".py")
        || lower.ends_with(".json")
        || lower.ends_with(".yaml")
        || lower.ends_with(".yml")
        || lower.contains("::{")
        || lower.contains("use ")
}

fn short_contract_excerpt(value: &str) -> String {
    let mut out = value.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 90;
    if out.chars().count() > MAX {
        out = out.chars().take(MAX - 1).collect::<String>();
        out.push('…');
    }
    out
}

#[cfg(test)]
include!("smells/advisory_tests.inc");
#[cfg(test)]
include!("smells/source_fact_tests.inc");
#[cfg(test)]
include!("smells/graph_tests.inc");

#[cfg(test)]
mod evidence_cap_tests {
    use super::{capped_join, EVIDENCE_LIST_CAP};

    #[test]
    fn capped_join_bounds_a_high_cardinality_list() {
        // Under the cap: verbatim, no "more" tail.
        let few = ["a".to_string(), "b".to_string()];
        assert_eq!(capped_join(&few, " · "), "a · b");

        // Exactly at the cap: still verbatim.
        let at: Vec<String> = (0..EVIDENCE_LIST_CAP).map(|i| i.to_string()).collect();
        let joined = capped_join(&at, ", ");
        assert!(!joined.contains("more"), "{joined}");

        // Over the cap: show EVIDENCE_LIST_CAP entries + one "… and N more".
        let many: Vec<String> = (0..EVIDENCE_LIST_CAP + 5).map(|i| i.to_string()).collect();
        let joined = capped_join(&many, ", ");
        assert!(joined.ends_with("… and 5 more"), "{joined}");
        assert_eq!(
            joined.split(", ").count(),
            EVIDENCE_LIST_CAP + 1,
            "exactly the cap entries plus the tail: {joined}"
        );
        // Works for &str slices too (the tangled_file path passes Vec<&str>).
        let refs = ["x", "y"];
        assert_eq!(capped_join(&refs, "/"), "x/y");
    }
}
