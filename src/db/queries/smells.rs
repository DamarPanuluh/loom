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
use std::collections::{HashMap, HashSet, VecDeque};

use super::snapshot::{DiscoverySnapshot, QuerySnapshot};
use crate::types::{Hypothesis, Note, TargetsEdge, VocabTerm};
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
/// Behavioral symbols above this span are large enough to inspect for splitting.
/// Deliberately high: this is a coarse snapshot signal, not real complexity.
pub const LARGE_BEHAVIORAL_SYMBOL_LINES: usize = 200;
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
#[derive(Debug, Clone, Serialize)]
pub struct Smell {
    /// twin_intents | duplicated_responsibility | overlapping_ownership
    /// | scattered_intent | tangled_file | unmeasured_intents
    /// | undeclared_coupling | layering_violation | recurrent_trouble
    /// | unused_rule | happy_path_only | vocab_drift | duplicate_detection_unarmed
    /// | hypothesis_accumulation | symbol_accountability_gap | dependency_cycle
    /// | intent_island | transitive_layering_violation | cochange_coupling
    /// | nonlocal_proof | code_clone | large_behavioral_symbol
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
                "`loom vocab suggest` — candidate terms mined from THIS graph's own intents (loom can't know your codebase's vocabulary), ranked by collision potential".into(),
                "register the ones that name a real shared responsibility (`loom vocab add <term> --why …`) and tag coded intents with them (`loom intent tag add <intent> <term>`)".into(),
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
        "cochange_coupling" => SmellTeaching {
            principle: "Files that keep changing together are coupled even when neither imports the other — git history reveals the hidden contract static analysis misses. It is a SUGGESTION to investigate, not a defect: confirm the relationship or explain why the co-change is incidental.".into(),
            inspect: vec![
                "read both intents' criteria and the files named in evidence".into(),
                "decide if they share a hidden contract (serializer/deserializer, schema/consumer, code/fixture) or just co-changed in a wide refactor".into(),
                "`loom edge explore <a> <b>` to ground the relationship or mark it independent".into(),
            ],
            avoid: vec!["do not treat co-change as proof of a relationship without reading the code; a wide refactor couples files incidentally".into()],
            done_when: "the pair has a grounded or independent RELATES_TO verdict (this is advisory — it never gates phase=complete)".into(),
        },
        "shotgun_surgery" => SmellTeaching {
            principle: "When one owned behavior repeatedly changes with many unrelated owned files, the boundary may be too broad, too central, or missing explicit relationships; git history exposes change pressure the static graph cannot.".into(),
            inspect: vec![
                "read the named intent and the co-changing files in evidence".into(),
                "decide whether the changes are one hidden contract, a wide mechanical refactor pattern, or a responsibility that should be split/reowned".into(),
                "`loom edge explore <a> <b>` for real hidden contracts; use a decision note when the co-change is incidental".into(),
            ],
            avoid: vec![
                "do not split a stable coordinator just because it is central; prove the maintenance pain first".into(),
                "do not add relationship edges from history alone without reading the current code".into(),
            ],
            done_when: "the hidden relationships are grounded/refuted, the broad responsibility is split through a proven hypothesis, or a decision note records why the recurring wide co-change is incidental (this is advisory — it never gates phase=complete)".into(),
        },
        "nonlocal_proof" => SmellTeaching {
            principle: "A passing LINKED validation is not a test that EXERCISES the intent's grounded code. The `proven` axis counts the link, so a leaf proven only by a test living in OTHER files reads green while its own code may have no direct test — partial-coverage overstatement. It is a SUGGESTION to investigate, not a defect.".into(),
            inspect: vec![
                "read the intent's grounded file(s) and ask whether the named test actually drives that code path".into(),
                "`loom intent show <intent>` for its groundings; `loom validation list` for the proof's command".into(),
                "if the real proof is an e2e/subprocess check, record it as an `assertion`/`saga` validation — this advisory defers to those (it only judges `test`-type proofs it can statically locate)".into(),
            ],
            avoid: vec!["do not read a green `proven` axis as coverage of the grounded code; a co-grounded test can pass without touching this file".into()],
            done_when: "the grounded code has a directly-exercising test, the IMPLEMENTS locator is corrected, or a decision note records why the existing proof suffices (this is advisory — it never gates phase=complete)".into(),
        },
        "code_clone" => SmellTeaching {
            principle: "Structurally identical code in unrelated files is duplicated logic the intent-level detectors are blind to — they need shared tags, a shared file, or an import; copy-paste with renamed identifiers in disjoint code has none. It is a SUGGESTION to investigate, not a defect: a clone can be legitimate (generated code, deliberately independent copies that must not be coupled). Detection is not the decision — the same shape_hash flags both a coincidental dispatch shim (leave it) and a real five-copy utility (dedupe it); only reading the code tells them apart.".into(),
            inspect: vec![
                "read the symbol in each listed location and decide whether they are one responsibility implemented twice or coincidentally similar".into(),
                "`loom intent show <intent>` / `loom codefile show <path>` to see who owns each copy".into(),
                "if both copies are owned, `loom edge explore <a> <b>` — a structural clone is evidence for a `duplicated_responsibility` merge".into(),
                "real dup you are not deduping this pass? capture it as work, not a note: `loom hypothesis add` — the clone is the claim, the shape collapsing to one definition is the predicted outcome; `adopt --spawned` makes it a planned refactor".into(),
            ],
            avoid: vec![
                "do not refactor to dedupe before confirming the copies should share one owner — deliberately independent copies exist".into(),
                "do not treat a short similar body as proof; the size floor already filters trivia, but read before merging".into(),
                "do not bury a real-but-deferred dedupe in a decision note — use a hypothesis when there is real work to do; use a file decision only when the copies are deliberately independent".into(),
            ],
            done_when: "the owning intents have a grounded or independent RELATES_TO verdict, the duplication is removed, a refactor hypothesis tracks a deliberately deferred dedupe, or a decision note marks copies that must stay independent (this is advisory — it never gates phase=complete)".into(),
        },
        "string_contract_duplicate" => SmellTeaching {
            principle: "Repeated user-facing or contract strings drift when each copy becomes its own tiny source of truth. It is a suspicion to answer, not an automatic defect: exact repetition can be deliberate when the words belong to independent surfaces.".into(),
            inspect: vec![
                "read each listed symbol and decide whether the repeated text is one contract, help/error message, or command example".into(),
                "check whether one constant/helper should own the wording or whether independent copies are intentional".into(),
                "`loom intent show <intent>` / `loom codefile show <path>` when the repeated strings are owned by mapped behavior".into(),
            ],
            avoid: vec![
                "do not centralize wording before confirming the copies must change together".into(),
                "do not flag tiny labels, enum values, import paths, fixtures, or test strings as product contracts".into(),
            ],
            done_when: "the wording has one source of truth, the copies are intentionally independent and documented with a current decision note, or the owning intents have an explored relationship".into(),
        },
        "large_behavioral_symbol" => SmellTeaching {
            principle: "A very large function, method, def, or impl is usually carrying multiple decisions; span is only a suspicion, so inspect the behavior before splitting.".into(),
            inspect: vec![
                "read the named symbol from top to bottom and identify distinct phases, modes, or responsibilities".into(),
                "check whether the current intent ownership is broad because the behavior is broad, or because the code needs smaller internal boundaries".into(),
                "look for validation gaps before extracting helpers so behavior stays pinned".into(),
            ],
            avoid: vec![
                "do not split a deliberately linear workflow just to satisfy the threshold".into(),
                "do not extract helpers that hide the same branchy behavior behind vague names".into(),
            ],
            done_when: "the behavior is split into smaller named units, or a current decision note on the file explains why this large symbol is deliberate".into(),
        },
        "panic_marker_risk" => SmellTeaching {
            principle: "A panic/unwrap/expect/todo marker in non-test behavior can turn an expected sad path into a process abort or unfinished path; it needs an explicit boundary decision.".into(),
            inspect: vec![
                "read the symbol and identify whether each marker is on trusted setup code, impossible state, or user/repo input".into(),
                "check whether the owning intent has a sad/fallback validation for the marker's failure mode".into(),
                "replace accidental aborts with typed errors or record why the abort is the contract".into(),
            ],
            avoid: vec![
                "do not blindly replace every unwrap; first classify invariant versus recoverable failure".into(),
                "do not accept todo/unimplemented in implemented behavior without an explicit needs_change or decision".into(),
            ],
            done_when: "recoverable failures are handled/proven, unfinished markers are removed or moved to planned work, or a current file decision explains why the abort marker is deliberate".into(),
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
            principle: "The non-sunny states of a behavior are real only when realized, grounded, and proven; naming the trigger (a 'happy' behavior or a 'populated' UI state) without its required siblings is not enough. Two families: behavioral happy → sad/fallback, and UI-state populated → empty/error (loading is recognized but not required).".into(),
            inspect: vec![
                "inspect the parent's aspect-tagged children for the triggered family".into(),
                "check lifecycle, IMPLEMENTS groundings, and passed validations for each required sibling (sad/fallback, or empty/error)".into(),
                "add or prove the missing path/state, or record why it is not applicable".into(),
            ],
            avoid: vec!["do not clear the debt with planned or unproven child intents".into()],
            done_when: "the required sibling paths/states are implemented, grounded, and directly proven, or a current decision explains why they are not required".into(),
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
        "dependency_cycle" => SmellTeaching {
            principle: "RELATES_TO is semantically undirected, so a relationship belongs in ONE row. Two GROUNDED rows for the same pair (a→b and b→a both carry a verdict) store the relationship twice: it double-counts in degree/centrality and skews `loom next` ranking, and the two verdicts can silently disagree. Exactly one direction is the redundant/incidental one to retire — unless the mutuality is a deliberate peer contract.".into(),
            inspect: vec![
                "`loom edge show rt:<a>:<b>` and `loom edge show rt:<b>:<a>` — compare the two verdicts and criteria".into(),
                "read both intents' criteria; decide which way the dependency actually runs".into(),
                "check whether the two are really one responsibility (a merge) or genuine mutual peers (a deliberate contract)".into(),
            ],
            avoid: vec!["do not keep both directions as the default; do not 'fix' an uninspected saga round-trip flow as if it were graph-hygiene debt (those edges are never flagged here — only deliberately grounded pairs are)".into()],
            done_when: "the redundant direction is marked independent (or the two intents merged), or a current decision note on the smaller-id intent records why both directions deliberately hold".into(),
        },
        "transitive_layering_violation" => SmellTeaching {
            principle: "The direct layering check exempts unlayered intents, so an illegal up-the-order dependency can hide by routing THROUGH them — every single hop looks clean, but the composed path makes a deeper layer depend on a shallower one. Direction is violated end-to-end even though no one import is.".into(),
            inspect: vec![
                "read the path in the evidence — the unlayered intermediate(s) are where the violation hides".into(),
                "decide: should the intermediate carry a layer (arming the direct check), or is the end-to-end dependency itself wrong?".into(),
                "fix the real direction (move shared code down / invert), or `loom layer order` the intermediate, or record a deliberate exception on the importing intent".into(),
            ],
            avoid: vec!["do not assume an all-clean-hops path is fine — the order is violated across the whole chain; do not silence it by adding a relationship".into()],
            done_when: "the end-to-end dependency points down (or the intermediate is layered so the direct check governs the hop), or a current decision note on the importing intent justifies it".into(),
        },
        "intent_island" => SmellTeaching {
            principle: "Every intent should reach a system-level purpose through HIERARCHY or RELATES_TO; an island has no such path, so nothing in the graph explains why it exists in this product.".into(),
            inspect: vec![
                "read the island members and find which existing branch they belong under".into(),
                "`loom edge hierarchy <parent> <child>` to attach the island to its real parent, or `loom edge explore <a> <b>` to ground a relationship into the connected graph".into(),
                "if the island is a genuinely separate top-level purpose, add or confirm a system-level intent for it".into(),
            ],
            avoid: vec!["do not leave orphaned intents floating; do not attach an island to an unrelated parent just to silence the finding".into()],
            done_when: "every island member reaches a system-level root through HIERARCHY or RELATES_TO, or a current decision note records why the disconnected subgraph is intentional".into(),
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

/// Snapshot + input reusing form for storage backends that can load the read
/// planes directly.
pub fn compute_smells_from_parts(
    snapshot: &QuerySnapshot,
    inputs: SmellInputs<'_>,
) -> Result<SmellReport> {
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
    // File ownership must exclude deprecated intents: `retire_intent` leaves
    // their IMPLEMENTS edges in place, so without this filter retired code keeps
    // generating undeclared_coupling / tangled_file / layering findings keyed by
    // dead intents — green-gating noise. Every other detector below already
    // filters `status != "deprecated"`; the ownership maps must match.
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

    // The adjudication ledger: every note, loaded once. `last_decision` maps a
    // target id to its newest kind=decision note — the recorded "looked at it,
    // here's the call" that resolves a structural finding until the structure
    // changes again underneath it (recurrent_trouble set the pattern).
    let mut last_decision: HashMap<&str, &crate::types::Note> = HashMap::new();
    for n in inputs.notes {
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
            .filter(|n| rfc3339_after(n.created_at.as_str(), anchor))
            .copied()
    };

    let mut smells: Vec<Smell> = Vec::new();
    let mut adjudicated_out: Vec<AdjudicatedSmell> = Vec::new();

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

    // 1. Twin intents — split-brain in the semantic plane: two intents at the
    //    same abstraction level that read like the same responsibility, with
    //    no recorded relationship between them.
    for same_level in intents_by_level.values() {
        for i in 0..same_level.len() {
            for j in (i + 1)..same_level.len() {
                let (a, b) = (same_level[i], same_level[j]);
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
    for same_level in intents_by_level.values() {
        for i in 0..same_level.len() {
            for j in (i + 1)..same_level.len() {
                let (a, b) = (same_level[i], same_level[j]);
                if linked.contains(&(a.id.as_str(), b.id.as_str())) {
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
                    let shared_tokens: Vec<String> = signal_toks[a.id.as_str()]
                        .intersection(&signal_toks[b.id.as_str()])
                        .cloned()
                        .collect();
                    let sim = jaccard(&signal_toks[a.id.as_str()], &signal_toks[b.id.as_str()]);
                    if sim < DUP_UNTAGGED_SIMILARITY
                        || shared_tokens.len() < DUP_UNTAGGED_SHARED_TOKENS
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
                let registry = inputs.vocab_terms.len();
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

    // 4b. Large behavioral symbol — a pure physical snapshot signal for
    //     functions/methods/defs/impls whose span is large enough to deserve
    //     inspection. No complexity field needed: this deliberately coarse pass
    //     only says "read this large behavior before trusting the boundary".
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
            if let Some(note) = adjudicated(cf.id.as_str(), cf.last_modified.as_str()) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "large_behavioral_symbol".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "the file is modified after the ruling".into(),
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
                    "inspect {}:{}-{}; split distinct phases/modes into named helpers or smaller owned behavior, or record why the large symbol is deliberate: `loom note add --file {} --kind decision --text \"<why {} stays large>\"` resolves this finding (editing the file re-opens it)",
                    cf.path, f.line_start, f.line_end, cf.path, f.label
                ),
                teaching: teaching_for("large_behavioral_symbol"),
            });
        }
    }

    // 4c. Panic/unwrap/todo markers in implemented behavior — token-derived
    //     source facts populated during sync. These are not automatically bugs:
    //     they are places where sad-path behavior depends on an invariant that
    //     must be inspected, proven, or explicitly accepted.
    for cf in &snapshot.codefiles {
        for f in &cf.symbol_facts {
            if f.is_test || !behavioral_symbol_kind(f.kind.as_str()) || f.panic_marker_count == 0 {
                continue;
            }
            let summary = format!(
                "{} in {} has {} panic/unfinished marker(s)",
                f.label, cf.path, f.panic_marker_count
            );
            if let Some(note) = adjudicated(cf.id.as_str(), cf.last_modified.as_str()) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "panic_marker_risk".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "the file is modified after the ruling".into(),
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
                    "inspect {}:{}-{}; replace recoverable aborts with handled errors/proofs, move unfinished behavior to planned work, or accept the invariant: `loom note add --file {} --kind decision --text \"<why these markers are deliberate>\"` resolves this finding (editing the file re-opens it)",
                    cf.path, f.line_start, f.line_end, cf.path
                ),
                teaching: teaching_for("panic_marker_risk"),
            });
        }
    }

    // 4d. Repeated string contracts — long user-facing/help/error/example
    //     strings copied across symbols can drift silently. This is deliberately
    //     conservative and ignores short labels, path-like values, and tests.
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
                    file_id: cf.id.as_str(),
                    file_modified: cf.last_modified.as_str(),
                    label: f.label.as_str(),
                    line: literal.line,
                    value: literal.value.as_str(),
                });
            }
        }
    }
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
        if let Some(note) = adjudicated(anchor.file_id, newest) {
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
        smells.push(Smell {
            kind: "string_contract_duplicate".into(),
            score: locs.len() as f64 * (anchor.value.len() as f64 / 40.0).max(1.0),
            summary,
            evidence: format!(
                "normalized repeated text appears in {} symbol(s) across {} file(s): {}",
                distinct_symbols, distinct_files, evidence
            ),
            remedy: format!(
                "inspect the repeated text; extract one source of truth if the wording must change together, or record deliberate independence: `loom note add --file {} --kind decision --text \"<why this repeated string is intentional>\"` resolves this finding (editing any carrying file re-opens it)",
                anchor.path
            ),
            teaching: teaching_for("string_contract_duplicate"),
        });
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
                let example = format!("{} → {}", cf.path, target);
                let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
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
                        if seen_pairs.insert(key.clone()) {
                            pair_files.entry(key).or_default().push(example.clone());
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
        let layer_rank: HashMap<String, usize> = inputs
            .layer_order
            .iter()
            .cloned()
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

    // 6c. Transitive layering violation — an up-the-order dependency that is
    //     CLEAN at every single hop (6b never fires) but illegal across the
    //     whole path. Because a chain of declared, non-violating hops keeps the
    //     layer rank non-decreasing, the only way to reach a SHALLOWER layer
    //     through all-clean hops is to route through UNLAYERED intermediates —
    //     so this catches exactly what the direct check is blind to.
    {
        let layer_rank: HashMap<&str, usize> = inputs
            .layer_order
            .iter()
            .enumerate()
            .map(|(r, l)| (l.as_str(), r))
            .collect();
        if !layer_rank.is_empty() {
            let layer_of: HashMap<&str, &str> = intents
                .iter()
                .map(|i| (i.id.as_str(), i.layer.as_str()))
                .collect();
            let rank = |id: &str| layer_of.get(id).and_then(|l| layer_rank.get(*l)).copied();
            // Intent-level import graph. `adj` keeps only CLEAN hops (a hop a→b
            // is dropped when both are layered and rank(a) > rank(b) — those are
            // 6b's). `direct` records every direct import so we never re-report
            // a direct pair here.
            let mut adj: HashMap<&str, HashSet<&str>> = HashMap::new();
            let mut direct: HashSet<(&str, &str)> = HashSet::new();
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
                            if a == b {
                                continue;
                            }
                            direct.insert((*a, *b));
                            if let (Some(ra), Some(rb)) = (rank(a), rank(b)) {
                                if ra > rb {
                                    continue; // directly-violating hop — 6b owns it
                                }
                            }
                            adj.entry(*a).or_default().insert(*b);
                        }
                    }
                }
            }
            let mut layered: Vec<&str> = intents
                .iter()
                .filter(|i| i.status != "deprecated" && rank(i.id.as_str()).is_some())
                .map(|i| i.id.as_str())
                .collect();
            layered.sort();
            for &a in &layered {
                let Some(ra) = rank(a) else {
                    continue;
                };
                // BFS over clean hops from `a`, tracking a parent for one path.
                let mut parent: HashMap<&str, &str> = HashMap::new();
                let mut seen: HashSet<&str> = HashSet::new();
                let mut q: VecDeque<&str> = VecDeque::new();
                seen.insert(a);
                q.push_back(a);
                while let Some(v) = q.pop_front() {
                    if let Some(nbrs) = adj.get(v) {
                        let mut ns: Vec<&str> = nbrs.iter().copied().collect();
                        ns.sort(); // deterministic path reconstruction
                        for w in ns {
                            if seen.insert(w) {
                                parent.insert(w, v);
                                q.push_back(w);
                            }
                        }
                    }
                }
                let mut reached: Vec<&str> = seen.iter().copied().filter(|&c| c != a).collect();
                reached.sort();
                for c in reached {
                    let Some(rc) = rank(c) else {
                        continue;
                    };
                    if rc >= ra || direct.contains(&(a, c)) {
                        continue; // not a violating direction, or 6b's direct case
                    }
                    // Reconstruct the clean path a → … → c (≥1 unlayered hop).
                    let mut path = vec![c];
                    let mut cur = c;
                    while let Some(&p) = parent.get(cur) {
                        path.push(p);
                        if p == a {
                            break;
                        }
                        cur = p;
                    }
                    path.reverse();
                    if path.len() < 3 {
                        continue; // need at least one intermediate
                    }
                    let trail = path
                        .iter()
                        .map(|id| {
                            let n = name_of.get(*id).copied().unwrap_or(id);
                            let l = layer_of.get(*id).copied().unwrap_or("");
                            if l.is_empty() {
                                format!("'{n}' (unlayered)")
                            } else {
                                format!("'{n}' ({l})")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" → ");
                    let (na, nc) = (
                        name_of.get(a).copied().unwrap_or(a),
                        name_of.get(c).copied().unwrap_or(c),
                    );
                    let (la, lc) = (
                        layer_of.get(a).copied().unwrap_or(""),
                        layer_of.get(c).copied().unwrap_or(""),
                    );
                    let summary = format!(
                        "'{na}' ({la}) transitively depends on '{nc}' ({lc}) against the declared layer order — clean at every hop"
                    );
                    if let Some(note) =
                        adjudicated(a, newest_grounding.get(a).copied().unwrap_or(""))
                    {
                        adjudicated_out.push(AdjudicatedSmell {
                            kind: "transitive_layering_violation".into(),
                            summary,
                            ruling: note.text.clone(),
                            ruled_by: note.author.clone(),
                            ruled_at: note.created_at.clone(),
                            reopens_when: "a new grounding lands on the importing intent".into(),
                            teaching: teaching_for("transitive_layering_violation"),
                        });
                        continue;
                    }
                    smells.push(Smell {
                        kind: "transitive_layering_violation".into(),
                        score: 6.0 + (path.len() - 2) as f64,
                        summary,
                        evidence: format!(
                            "every hop is clean (6b sees nothing), but the chain routes a deeper layer up to a shallower one through unlayered intermediate(s): {trail}"
                        ),
                        remedy: format!(
                            "fix the END-TO-END direction: whatever '{la}' reaches up to use belongs at or below '{la}' (move it down / extract a lower shared module); OR give the unlayered intermediate(s) a `--layer` so the direct check governs each hop; OR if this up-dependency is DELIBERATE, record it: `loom note add --intent {a} --kind decision --text \"<why this layer may reach up>\"` (a new grounding re-opens it)"
                        ),
                        teaching: teaching_for("transitive_layering_violation"),
                    });
                }
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
        for n in inputs.notes {
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
        let proposed = inputs.proposed_hypotheses;
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
                let targeted: HashSet<&str> = inputs
                    .targets
                    .iter()
                    .map(|t| t.hypothesis_id.as_str())
                    .collect();
                let untargeted = proposed
                    .iter()
                    .filter(|h| !targeted.contains(h.id.as_str()))
                    .count();
                if let Some(oldest) = proposed.iter().min_by(|a, b| {
                    a.created_at
                        .cmp(&b.created_at)
                        .then_with(|| a.name.cmp(&b.name))
                }) {
                    let sample: Vec<&str> =
                        proposed.iter().take(5).map(|h| h.name.as_str()).collect();
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
            // A required-sibling child counts as SATISFIED only when realized,
            // grounded, and directly proven — the gating bar, applied to any
            // family's required aspects (sad/fallback, empty/error, …).
            if child.lifecycle == "implemented"
                && files_of.contains_key(child.id.as_str())
                && directly_proven_intents.contains(child.id.as_str())
            {
                satisfied_aspects
                    .entry(p.as_str())
                    .or_default()
                    .insert(child.aspect.as_str());
            }
            let e = newest_aspect_child.entry(p.as_str()).or_default();
            if rfc3339_after(child.created_at.as_str(), e) {
                *e = &child.created_at;
            }
        }
        for (parent_id, aspects) in &child_aspects {
            let satisfied = satisfied_aspects.get(parent_id);
            // One finding per TRIGGERED family (a parent could own both a
            // behavioral happy path and a UI populated state — each owes its own
            // siblings). `loading` is recognized but never required.
            for (trigger, required) in ASPECT_FAMILIES {
                if !aspects.contains(trigger) {
                    continue;
                }
                let missing: Vec<&str> = required
                    .iter()
                    .filter(|a| !satisfied.is_some_and(|s| s.contains(*a)))
                    .copied()
                    .collect();
                if missing.is_empty() {
                    continue;
                }
                // Adjudicated: a decision note on the parent newer than its
                // newest aspect-carrying child records why the missing path is
                // N/A. A new aspect-tagged child re-opens the question.
                let pname = name_of.get(parent_id).copied().unwrap_or(parent_id);
                let summary = format!(
                    "'{pname}' declares a '{trigger}' aspect but no realized+proven {} sibling",
                    missing.join("/")
                );
                if let Some(note) = adjudicated(
                    parent_id,
                    newest_aspect_child.get(parent_id).copied().unwrap_or(""),
                ) {
                    adjudicated_out.push(AdjudicatedSmell {
                        kind: "happy_path_only".into(),
                        summary,
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
                    summary,
                    evidence: format!(
                        "children carry aspects {{{}}}; realized+proven siblings {{{}}} — the '{trigger}' family's {} path(s) are not implemented, grounded, and directly proven",
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
                        },
                        missing.join("/")
                    ),
                    remedy: format!(
                        "realize and prove the missing path(s): loom intent add --aspect {first} --level feature … then loom edge hierarchy {parent_id} <child>, ground it with `loom edge implement`, and attach a passed validation; or record why it's N/A: loom note add --intent {parent_id} --kind decision --text \"<why no {m} path>\" (resolves this finding; a new aspect-tagged child re-opens it)",
                        first = missing[0],
                        m = missing.join("/")
                    ),
                    teaching: teaching_for("happy_path_only"),
                });
            }
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
        let terms = inputs.vocab_terms;
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
            inputs.notes,
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

    // 15. Reciprocal dependency — the one circular RELATES_TO pattern whose
    //     meaning survives loom's storage. RELATES_TO rows are stored directed
    //     (PRIMARY KEY(from_id,to_id)) but `loom edge explore` never
    //     canonicalizes endpoint order and the snapshot's `linked` set adds BOTH
    //     directions for every undirected analysis — so a long directed "cycle"
    //     a→b→c→a is just typing-order noise (an SCC over it is vacuous; on
    //     loom's own graph it surfaced as one 39-intent blob that gated green for
    //     nothing). The only honest signal is a RECIPROCAL pair: two intents
    //     where BOTH directed rows a→b AND b→a carry a real verdict. That is one
    //     undirected relationship stored twice — it double-counts in
    //     degree/betweenness (the snapshot adds both directions) and skews
    //     `loom next` ranking, and the two rows can carry silently-disagreeing
    //     verdicts. We require BOTH directions GROUNDED (status not uninspected,
    //     not independent) precisely to EXCLUDE the mechanically-created kind:
    //     `loom saga add` writes *uninspected* directed path edges, so a
    //     round-trip consumer journey legitimately produces an a↔b pair —
    //     gating on those would false-alarm on a healthy graph with wrong advice.
    //     (FUTURE: a real saga-flow cycle detector wants a stored saga-edge
    //     provenance marker; deferred — no such marker exists yet. RETIREMENT:
    //     if RELATES_TO storage is ever canonicalized at insert, reciprocal
    //     pairs become impossible and this detector should be retired, not left
    //     as a zero-yield gate.)
    {
        let active_ids: HashSet<&str> = intents
            .iter()
            .filter(|i| i.status != "deprecated")
            .map(|i| i.id.as_str())
            .collect();
        // Grounded directed rows only: a verdict recorded (NOT uninspected, NOT
        // independent) between two active intents. Indexed for O(1) reverse
        // lookup and to read each direction's status/last_inspected.
        let mut grounded: HashMap<(&str, &str), &crate::types::RelatesTo> = HashMap::new();
        for e in relates {
            if e.inspection_status == "uninspected"
                || e.inspection_status == "independent"
                || e.from_id == e.to_id
                || !active_ids.contains(e.from_id.as_str())
                || !active_ids.contains(e.to_id.as_str())
            {
                continue;
            }
            grounded.insert((e.from_id.as_str(), e.to_id.as_str()), e);
        }
        for (&(a, b), fwd) in &grounded {
            if a >= b {
                continue; // each unordered pair once; the a<b guard dedupes it
            }
            let Some(rev) = grounded.get(&(b, a)) else {
                continue; // only one direction grounded — not a reciprocal pair
            };
            let (na, nb) = (
                name_of.get(a).copied().unwrap_or(a),
                name_of.get(b).copied().unwrap_or(b),
            );
            // Both rows are grounded, so both carry a real last_inspected — no
            // empty-anchor phantom suppression. Re-opens when either is re-inspected.
            let pair_anchor = fwd.last_inspected.as_str().max(rev.last_inspected.as_str());
            let summary =
                format!("mutual RELATES_TO dependency: '{na}' ↔ '{nb}' (both directions grounded)");
            if let Some(note) = adjudicated(a, pair_anchor) {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "dependency_cycle".into(),
                    summary,
                    ruling: note.text.clone(),
                    ruled_by: note.author.clone(),
                    ruled_at: note.created_at.clone(),
                    reopens_when: "either direction's edge is re-inspected".into(),
                    teaching: teaching_for("dependency_cycle"),
                });
            } else {
                let deg =
                    *snapshot.degrees.get(a).unwrap_or(&0) + *snapshot.degrees.get(b).unwrap_or(&0);
                smells.push(Smell {
                    kind: "dependency_cycle".into(),
                    score: 6.0 + deg as f64,
                    summary,
                    evidence: format!(
                        "both directed rows are grounded — {na}→{nb} is {} and {nb}→{na} is {}. RELATES_TO is semantically undirected (the snapshot adds both directions for degree/centrality), so this is ONE relationship stored twice: it double-counts in degree/betweenness and skews `loom next` ranking, and the two verdicts can silently disagree.",
                        fwd.inspection_status, rev.inspection_status
                    ),
                    remedy: format!(
                        "`loom edge show rt:{a}:{b}` and `loom edge show rt:{b}:{a}`; decide which way the dependency really runs, then `loom edge explore <incidental-from> <incidental-to> independent` to retire the redundant direction (keep the better-grounded verdict). If '{na}' and '{nb}' are one responsibility, merge them. If the mutual relationship is DELIBERATE (true peers / a mutual contract), record it: `loom note add --intent {a} --kind decision --text \"<why both directions hold>\"` (re-inspecting either edge re-opens this)."
                    ),
                    teaching: teaching_for("dependency_cycle"),
                });
            }
        }
    }

    // 16. Intent island — a subgraph with no path to a system-level root. The
    //     UNDIRECTED connectivity over HIERARCHY + non-independent RELATES_TO
    //     partitions intents into components; a component holding no
    //     system-level intent cannot reach any product purpose. One finding per
    //     island. When the graph has NO system root at all the detector is
    //     unarmed (nothing to be an island relative to) and stays silent — the
    //     missing-system-root gap is the granularity contract's problem, not
    //     this detector's.
    {
        let active: Vec<&crate::types::Intent> = intents
            .iter()
            .filter(|i| i.status != "deprecated")
            .collect();
        let n = active.len();
        let has_system = active.iter().any(|i| i.abstraction_level == "system");
        if has_system && n > 0 {
            let idx: HashMap<&str, usize> = active
                .iter()
                .enumerate()
                .map(|(i, intent)| (intent.id.as_str(), i))
                .collect();
            let mut neighbors: Vec<HashSet<usize>> = vec![HashSet::new(); n];
            for (p, c) in hierarchy {
                if let (Some(&a), Some(&b)) = (idx.get(p.as_str()), idx.get(c.as_str())) {
                    if a != b {
                        neighbors[a].insert(b);
                        neighbors[b].insert(a);
                    }
                }
            }
            for e in relates {
                if e.inspection_status == "independent" {
                    continue;
                }
                if let (Some(&a), Some(&b)) =
                    (idx.get(e.from_id.as_str()), idx.get(e.to_id.as_str()))
                {
                    if a != b {
                        neighbors[a].insert(b);
                        neighbors[b].insert(a);
                    }
                }
            }
            let adjacency: Vec<Vec<usize>> = neighbors
                .into_iter()
                .map(|s| s.into_iter().collect())
                .collect();
            for comp in super::graph_algo::connected_components(n, &adjacency) {
                if comp
                    .iter()
                    .any(|&i| active[i].abstraction_level == "system")
                {
                    continue; // reaches a system root — not an island
                }
                let mut members: Vec<&crate::types::Intent> =
                    comp.iter().map(|&i| active[i]).collect();
                members.sort_by(|a, b| a.id.cmp(&b.id));
                let names: Vec<String> = members.iter().map(|i| format!("'{}'", i.name)).collect();
                let anchor = members[0]; // smallest id
                                         // A re-grounding of a member is the structural change that
                                         // re-opens a "deliberately separate" ruling.
                let island_anchor = members
                    .iter()
                    .filter_map(|i| newest_grounding.get(i.id.as_str()).copied())
                    .max()
                    .unwrap_or("");
                let summary = format!(
                    "{} intent(s) form an island with no path to a system-level root: {}",
                    members.len(),
                    names.join(", ")
                );
                if let Some(note) = adjudicated(anchor.id.as_str(), island_anchor) {
                    adjudicated_out.push(AdjudicatedSmell {
                        kind: "intent_island".into(),
                        summary,
                        ruling: note.text.clone(),
                        ruled_by: note.author.clone(),
                        ruled_at: note.created_at.clone(),
                        reopens_when: "a member is re-grounded".into(),
                        teaching: teaching_for("intent_island"),
                    });
                } else {
                    smells.push(Smell {
                        kind: "intent_island".into(),
                        score: 5.0 + members.len() as f64,
                        summary,
                        evidence: format!(
                            "no HIERARCHY or non-independent RELATES_TO path connects {} to any system-level intent: {}",
                            if members.len() == 1 { "this intent" } else { "these intents" },
                            names.join(", ")
                        ),
                        remedy: format!(
                            "attach the island: `loom edge hierarchy <parent> <child>` under its real parent, or `loom edge explore <a> <b>` to ground a relationship into the connected graph; if it is a genuinely separate top-level purpose, add a system intent for it; if the separation is DELIBERATE, record it: `loom note add --intent {} --kind decision --text \"<why this subgraph is intentionally disconnected>\"` (re-grounding a member re-opens this)",
                            anchor.id
                        ),
                        teaching: teaching_for("intent_island"),
                    });
                }
            }
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
    let declared_layers = inputs.layer_order.len();
    Ok(SmellReport {
        open: smells,
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
/// is ADVISORY and git-derived: it never gates green.
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

#[cfg(test)]
mod shotgun_surgery_tests {
    use super::*;
    use crate::types::{CodeFile, Implements, Intent, RelatesTo};

    fn intent(id: &str, name: &str) -> Intent {
        Intent {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            abstraction_level: "feature".into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn cf(path: &str) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: String::new(),
        }
    }

    fn imp(intent_id: &str, path: &str) -> Implements {
        Implements {
            id: format!("{intent_id}:{path}"),
            intent_id: intent_id.into(),
            codefile_id: path.into(),
            intent_name: intent_id.into(),
            codefile_path: path.into(),
            locator: String::new(),
            created_at: "t".into(),
            inspection_status: "passing".into(),
            last_inspected: "t".into(),
            inspected_by: String::new(),
            criterion: String::new(),
            evidence: String::new(),
            notes: String::new(),
            confidence: 1.0,
        }
    }

    fn rel(a: &str, b: &str) -> RelatesTo {
        RelatesTo {
            id: format!("{a}:{b}"),
            from_id: a.into(),
            from_name: a.into(),
            to_id: b.into(),
            to_name: b.into(),
            inspection_status: "passing".into(),
            last_inspected: "t".into(),
            inspected_by: String::new(),
            criterion: String::new(),
            evidence: String::new(),
            notes: String::new(),
            confidence: 1.0,
            priority_score: 0.0,
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: Default::default(),
        }
    }

    fn co_own_snapshot(second_status: &str) -> QuerySnapshot {
        let active = intent("act", "alpha config owner");
        let mut other = intent("dep", "delta config owner");
        other.status = second_status.into();
        QuerySnapshot::from_parts(
            vec![active, other],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![imp("act", "src/shared.rs"), imp("dep", "src/shared.rs")],
            vec![cf("src/shared.rs")],
            Some(Vec::new()),
        )
    }

    fn co_own_report(second_status: &str) -> SmellReport {
        compute_smells_from_parts(
            &co_own_snapshot(second_status),
            SmellInputs {
                notes: &[],
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }

    #[test]
    fn deprecated_intents_are_not_file_owners() {
        // Positive control: two ACTIVE intents co-owning one file overlap.
        let active = co_own_report("confirmed");
        let overlap_active = active
            .open
            .iter()
            .filter(|s| s.kind == "overlapping_ownership")
            .count();
        assert!(
            overlap_active >= 1,
            "two active co-owners should raise overlapping_ownership: {:?}",
            active.open
        );
        // `retire_intent` leaves the IMPLEMENTS edge behind; the deprecated
        // intent must no longer count as an owner, so the finding disappears.
        let deprecated = co_own_report("deprecated");
        let overlap_deprecated = deprecated
            .open
            .iter()
            .filter(|s| s.kind == "overlapping_ownership")
            .count();
        assert_eq!(
            overlap_deprecated, 0,
            "a deprecated co-owner must not own the file for smells: {:?}",
            deprecated.open
        );
    }

    fn snap(with_link: bool) -> QuerySnapshot {
        let mut relates = Vec::new();
        if with_link {
            relates.push(rel("hub", "p4"));
        }
        QuerySnapshot::from_parts(
            vec![
                intent("hub", "hub behavior"),
                intent("p1", "partner one"),
                intent("p2", "partner two"),
                intent("p3", "partner three"),
                intent("p4", "partner four"),
            ],
            Vec::new(),
            relates,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![
                imp("hub", "src/hub.rs"),
                imp("p1", "src/p1.rs"),
                imp("p2", "src/p2.rs"),
                imp("p3", "src/p3.rs"),
                imp("p4", "src/p4.rs"),
            ],
            vec![
                cf("src/hub.rs"),
                cf("src/p1.rs"),
                cf("src/p2.rs"),
                cf("src/p3.rs"),
                cf("src/p4.rs"),
            ],
            Some(Vec::new()),
        )
    }

    fn history() -> (HashMap<(String, String), usize>, HashMap<String, usize>) {
        let mut pairs = HashMap::new();
        let mut individual = HashMap::new();
        individual.insert("src/hub.rs".into(), 6);
        for p in ["src/p1.rs", "src/p2.rs", "src/p3.rs", "src/p4.rs"] {
            individual.insert(p.into(), 4);
            pairs.insert(("src/hub.rs".into(), p.into()), 3);
        }
        (pairs, individual)
    }

    #[test]
    fn hub_changing_with_many_unrelated_intents_flags() {
        let (pairs, individual) = history();
        let out = shotgun_surgery_suggestions(&snap(false), &pairs, &individual);
        assert!(
            out.iter()
                .any(|s| s.kind == "shotgun_surgery" && s.summary.contains("hub behavior")),
            "{out:?}"
        );
    }

    #[test]
    fn linked_partner_reduces_pressure_below_threshold() {
        let (pairs, individual) = history();
        assert!(
            shotgun_surgery_suggestions(&snap(true), &pairs, &individual).is_empty(),
            "one linked partner leaves only three unrelated partners"
        );
    }
}

// ---------------------------------------------------------------------------
// proof-locality advisory — the `proven` axis's quality check
// ---------------------------------------------------------------------------

/// `nonlocal_proof` advisories — the static counterpart to the 360° `proven`
/// axis. `proven` counts an implemented leaf with a VALIDATES edge to a PASSED
/// validation; it does NOT check the proof exercises the intent's grounded
/// code. A leaf grounded in file A but proven only by a `test` living in file B
/// reads green while A may have no direct test — partial-coverage overstatement.
///
/// This flags exactly that, STATICALLY (no instrumentation, no coverage run):
/// for each implemented leaf proven ONLY by `test`-type validations, it resolves
/// each test command's selectors to the files they live in (via the graph's
/// test-symbol facts) and flags the leaf when those resolve to a different
/// MODULE (directory) than its grounded code. Module-level — not file-level —
/// because Rust keeps a module's tests beside its code (`mod.rs` / a
/// `#[cfg(test)] mod tests`), so a same-directory test is legitimately local.
///
/// Two deliberate non-firings keep it false-positive-free (the trap the ④ spike
/// named): (1) any non-`test` proof — an `assertion`/`saga`/`manual_check` e2e
/// or subprocess check — exempts the leaf, because that proof legitimately
/// exercises code this static check can't see; (2) a command we can't resolve to
/// any file (an opaque script, a bare `cargo test`) is UNKNOWN, never non-local.
/// Computed OUTSIDE `compute_smells_from` — like `cochange_suggestions`, it is
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

    let mut out: Vec<Smell> = Vec::new();
    for ((hash_kind, _), members) in by_hash {
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
        let evidence = format!(
            "all share one {hash_label} ({} lines): {}",
            span,
            locs.iter()
                .map(|(p, f)| format!("{}:{}-{} '{}'", p, f.line_start, f.line_end, f.label))
                .collect::<Vec<_>>()
                .join(" · ")
        );
        out.push(Smell {
            kind: "code_clone".into(),
            score: span as f64 * count as f64,
            summary,
            evidence,
            remedy:
                "read each copy and decide which of three it is: (1) coincidental shape (e.g. dispatch shims that match by accident) — leave it; (2) one responsibility copied — dedupe now, or if both copies are owned `loom edge explore <a> <b>` to ground or refute the relationship (a structural clone is evidence for a `duplicated_responsibility` merge); (3) a real dup you are DEFERRING — file it as tracked work, not a dead note: `loom hypothesis add` with the clone as the claim and the shape group collapsing to one definition as the predicted outcome, so `loom hypothesis adopt --spawned` turns it into a planned refactor the build/validate machinery owns. If the copies must stay deliberately independent, record that ruling with `loom note add --file <participating-path> --kind decision --text \"<why these copies must stay independent>\"`; the advisory moves to adjudicated until a participating file changes".into(),
            teaching: teaching_for("code_clone"),
        });
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
    file_id: &'a str,
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
mod proof_locality_tests {
    use super::*;
    use crate::types::{CodeFile, Implements, Intent, SymbolFact, ValidatesEdge, Validation};

    fn leaf(id: &str, name: &str) -> Intent {
        Intent {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            abstraction_level: "feature".into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn cf(path: &str, test_syms: &[&str], code_syms: &[&str]) -> CodeFile {
        let mut facts = Vec::new();
        for s in test_syms {
            facts.push(SymbolFact {
                label: format!("fn {s}"),
                name: (*s).into(),
                kind: "fn".into(),
                visibility: "private".into(),
                line_start: 1,
                line_end: 2,
                is_test: true,
                string_literals: Vec::new(),
                panic_marker_count: 0,
                panic_markers: Vec::new(),
                body_hash: String::new(),
                shape_hash: String::new(),
            });
        }
        for s in code_syms {
            facts.push(SymbolFact {
                label: format!("pub fn {s}"),
                name: (*s).into(),
                kind: "fn".into(),
                visibility: "public".into(),
                line_start: 1,
                line_end: 2,
                is_test: false,
                string_literals: Vec::new(),
                panic_marker_count: 0,
                panic_markers: Vec::new(),
                body_hash: String::new(),
                shape_hash: String::new(),
            });
        }
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
        }
    }

    fn imp(intent: &str, path: &str) -> Implements {
        Implements {
            id: format!("imp:{intent}:{path}"),
            intent_id: intent.into(),
            codefile_id: path.into(),
            intent_name: String::new(),
            codefile_path: path.into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: String::new(),
            notes: String::new(),
            created_at: "t".into(),
        }
    }

    fn val(id: &str, vtype: &str, command: &str, result: &str) -> Validation {
        Validation {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            validation_type: vtype.into(),
            command: command.into(),
            last_run: String::new(),
            last_result: result.into(),
        }
    }

    fn vedge(vid: &str, iid: &str) -> ValidatesEdge {
        ValidatesEdge {
            id: format!("val:{vid}:{iid}"),
            validation_id: vid.into(),
            intent_id: iid.into(),
            validation_name: String::new(),
            intent_name: String::new(),
            created_at: "t".into(),
            inspection_status: "passing".into(),
            notes: String::new(),
        }
    }

    #[test]
    fn flags_a_leaf_proven_only_by_a_test_in_other_files() {
        let codefiles = vec![
            cf("src/commands/intent.rs", &[], &["delete_intent"]), // grounded, no tests
            cf("src/db/queries/intent.rs", &["retire_is_invisible"], &[]), // the test lives here
        ];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test retire_is_invisible",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert_eq!(out.len(), 1, "a non-local test proof should flag: {out:?}");
        assert_eq!(out[0].kind, "nonlocal_proof");
        assert!(
            out[0].summary.contains("intent retirement contract"),
            "{}",
            out[0].summary
        );
    }

    #[test]
    fn a_test_in_the_grounded_file_is_local_and_silent() {
        let codefiles = vec![cf(
            "src/db/queries/intent.rs",
            &["retire_is_invisible"],
            &["resolve_intent"],
        )];
        let intents = vec![leaf("i1", "intent resolution")];
        let implements = vec![imp("i1", "src/db/queries/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test retire_is_invisible",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a test in the grounded file is local: {out:?}"
        );
    }

    #[test]
    fn an_e2e_assertion_proof_exempts_the_leaf() {
        let codefiles = vec![
            cf("src/commands/intent.rs", &[], &["delete_intent"]),
            cf("src/db/queries/intent.rs", &["retire_is_invisible"], &[]),
        ];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        let validations = vec![
            val("v1", "test", "cargo test retire_is_invisible", "passed"),
            val("v2", "assertion", ".claude/skills/e2e_retire.sh", "passed"),
        ];
        let validates = vec![vedge("v1", "i1"), vedge("v2", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "an e2e/assertion proof exercises code the static check can't see — exempt: {out:?}"
        );
    }

    #[test]
    fn an_unresolvable_command_is_unknown_never_nonlocal() {
        let codefiles = vec![
            cf("src/commands/intent.rs", &[], &["delete_intent"]),
            cf("src/db/queries/intent.rs", &["retire_is_invisible"], &[]),
        ];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        // The selector matches no known test symbol → can't be located → silent.
        let validations = vec![val(
            "v1",
            "test",
            "cargo test no_such_test_name_xyz",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "an unresolvable selector is UNKNOWN, not non-local: {out:?}"
        );
    }

    #[test]
    fn a_sibling_test_in_the_same_module_is_local() {
        // loom keeps query-layer tests in queries/mod.rs, not beside each file;
        // module-level locality must treat that as covering the module (this would
        // wrongly flag under file-level locality).
        let codefiles = vec![
            cf("src/db/queries/intent.rs", &[], &["resolve_intent"]), // grounded code
            cf("src/db/queries/mod.rs", &["resolve_intent_roundtrips"], &[]), // tests live here
        ];
        let intents = vec![leaf("i1", "intent resolution")];
        let implements = vec![imp("i1", "src/db/queries/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test resolve_intent_roundtrips",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a test in a sibling file of the same module is local: {out:?}"
        );
    }

    #[test]
    fn a_module_path_selector_resolves_to_its_directory() {
        let codefiles = vec![cf(
            "src/db/queries/intent.rs",
            &["resolve_intent_roundtrips"],
            &["resolve_intent"],
        )];
        let intents = vec![leaf("i1", "intent resolution")];
        let implements = vec![imp("i1", "src/db/queries/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test db::queries 2>/dev/null",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a module-path selector resolves to its directory → local: {out:?}"
        );
    }

    #[test]
    fn a_trailing_module_path_selector_resolves_to_its_module() {
        let codefiles = vec![cf(
            "src/saga/spec.rs",
            &["good_spec_parses"],
            &["load_spec"],
        )];
        let intents = vec![leaf("i1", "saga spec")];
        let implements = vec![imp("i1", "src/saga/spec.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test saga::spec::tests::",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "a trailing module selector resolves to the grounded module: {out:?}"
        );
    }

    #[test]
    fn no_symbol_facts_means_the_instrument_is_unarmed() {
        // A graph with zero test-symbol facts must flag nothing, never everything.
        let codefiles = vec![cf("src/commands/intent.rs", &[], &["delete_intent"])];
        let intents = vec![leaf("i1", "intent retirement contract")];
        let implements = vec![imp("i1", "src/commands/intent.rs")];
        let validations = vec![val(
            "v1",
            "test",
            "cargo test retire_is_invisible",
            "passed",
        )];
        let validates = vec![vedge("v1", "i1")];
        let out = proof_locality_from_parts(
            &intents,
            &implements,
            &validates,
            &validations,
            &codefiles,
            &[],
        );
        assert!(
            out.is_empty(),
            "no symbol facts → unarmed → silent: {out:?}"
        );
    }
}

#[cfg(test)]
mod clone_tests {
    use super::*;
    use crate::types::{CodeFile, SymbolFact};

    fn sym(
        name: &str,
        body_hash: &str,
        shape_hash: &str,
        line_start: usize,
        line_end: usize,
        is_test: bool,
    ) -> SymbolFact {
        SymbolFact {
            label: format!("fn {name}"),
            name: name.into(),
            kind: "fn".into(),
            visibility: "private".into(),
            line_start,
            line_end,
            is_test,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: body_hash.into(),
            shape_hash: shape_hash.into(),
        }
    }

    fn cf(path: &str, facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
        }
    }

    fn snap(codefiles: Vec<CodeFile>) -> QuerySnapshot {
        QuerySnapshot::from_parts(
            Vec::new(), // intents
            Vec::new(), // hierarchy
            Vec::new(), // relates
            Vec::new(), // governs
            Vec::new(), // rules
            Vec::new(), // validates
            Vec::new(), // validations
            Vec::new(), // implements
            codefiles,
            Some(Vec::new()), // notes
        )
    }

    #[test]
    fn two_files_with_one_shared_body_hash_flag_once() {
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "HASH", "", 1, 10, false)]),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 29, false)]),
        ]);
        let out = clone_suggestions(&s, &[]);
        assert_eq!(
            out.len(),
            1,
            "an exact cross-file clone should flag once: {out:?}"
        );
        assert_eq!(out[0].kind, "code_clone");
        assert!(out[0].evidence.contains("body_hash"));
        assert!(out[0].evidence.contains("src/a.rs"));
        assert!(out[0].evidence.contains("src/b.rs"));
    }

    #[test]
    fn shared_shape_hash_flags_renamed_clone() {
        let s = snap(vec![
            cf(
                "src/a.rs",
                vec![sym("alpha", "BODY_A", "SHAPE", 1, 10, false)],
            ),
            cf(
                "src/b.rs",
                vec![sym("beta", "BODY_B", "SHAPE", 20, 29, false)],
            ),
        ]);
        let out = clone_suggestions(&s, &[]);
        assert_eq!(
            out.len(),
            1,
            "a normalized cross-file clone should flag once: {out:?}"
        );
        assert!(out[0].evidence.contains("shape_hash"));
    }

    #[test]
    fn data_declarations_do_not_group_by_shape_hash() {
        let mut alpha = sym("Alpha", "BODY_A", "SHAPE", 1, 10, false);
        alpha.kind = "struct".into();
        alpha.label = "struct Alpha".into();
        let mut beta = sym("Beta", "BODY_B", "SHAPE", 20, 29, false);
        beta.kind = "struct".into();
        beta.label = "struct Beta".into();
        let s = snap(vec![
            cf("src/a.rs", vec![alpha]),
            cf("src/b.rs", vec![beta]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "passive data declarations stay exact-text only"
        );
    }

    #[test]
    fn test_symbols_are_skipped() {
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "HASH", "", 1, 10, true)]),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 29, true)]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "test fixtures legitimately repeat"
        );
    }

    #[test]
    fn bodies_below_the_size_floor_are_skipped() {
        // span = line_end - line_start + 1 = 4 < MIN_CLONE_LINES (5).
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "HASH", "", 1, 4, false)]),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 23, false)]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "below the size floor is boilerplate"
        );
    }

    #[test]
    fn an_ignored_file_drops_the_pair_below_cross_file() {
        let pat = glob::Pattern::new("src/generated/**").unwrap();
        let s = snap(vec![
            cf(
                "src/generated/a.rs",
                vec![sym("alpha", "HASH", "", 1, 10, false)],
            ),
            cf("src/b.rs", vec![sym("beta", "HASH", "", 20, 29, false)]),
        ]);
        assert!(
            clone_suggestions(&s, &[pat]).is_empty(),
            "ignoring one copy leaves a single-file group → no cross-file clone"
        );
    }

    #[test]
    fn intra_file_repetition_is_not_a_cross_file_clone() {
        let s = snap(vec![cf(
            "src/a.rs",
            vec![
                sym("alpha", "HASH", "", 1, 10, false),
                sym("beta", "HASH", "", 20, 29, false),
            ],
        )]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "intra-file repetition is tangled_file's concern, not a cross-file clone"
        );
    }

    #[test]
    fn empty_body_hash_is_unarmed_and_silent() {
        let s = snap(vec![
            cf("src/a.rs", vec![sym("alpha", "", "", 1, 10, false)]),
            cf("src/b.rs", vec![sym("beta", "", "", 20, 29, false)]),
        ]);
        assert!(
            clone_suggestions(&s, &[]).is_empty(),
            "pre-v8 / feature-light facts carry no hash → instrument unarmed"
        );
    }
}

#[cfg(test)]
mod large_behavioral_symbol_tests {
    use super::*;
    use crate::types::{CodeFile, Note, SymbolFact};

    fn sym(
        kind: &str,
        label: &str,
        line_start: usize,
        line_end: usize,
        is_test: bool,
    ) -> SymbolFact {
        SymbolFact {
            label: label.into(),
            name: label.split_whitespace().last().unwrap_or(label).into(),
            kind: kind.into(),
            visibility: "private".into(),
            line_start,
            line_end,
            is_test,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: String::new(),
            shape_hash: String::new(),
        }
    }

    fn cf(path: &str, last_modified: &str, facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: last_modified.into(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
        }
    }

    fn report(codefiles: Vec<CodeFile>, notes: &[Note]) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            codefiles,
            None,
        );
        compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes,
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }

    fn of_kind<'a>(r: &'a SmellReport, kind: &str) -> Vec<&'a Smell> {
        r.open.iter().filter(|s| s.kind == kind).collect()
    }

    #[test]
    fn non_test_behavioral_symbol_at_threshold_is_open() {
        let r = report(
            vec![cf(
                "src/big.rs",
                "2026-06-15T00:00:00+00:00",
                vec![sym(
                    "fn",
                    "fn huge_behavior",
                    1,
                    LARGE_BEHAVIORAL_SYMBOL_LINES,
                    false,
                )],
            )],
            &[],
        );
        let found = of_kind(&r, "large_behavioral_symbol");
        assert_eq!(found.len(), 1, "large behavior should flag once: {found:?}");
        assert!(found[0].summary.contains("huge_behavior"));
        assert!(found[0].evidence.contains(&format!(
            "above the {}-line threshold",
            LARGE_BEHAVIORAL_SYMBOL_LINES
        )));
        assert_eq!(
            found[0].teaching.done_when,
            teaching_for("large_behavioral_symbol").done_when
        );
    }

    #[test]
    fn tests_data_declarations_and_small_functions_are_skipped() {
        let r = report(
            vec![cf(
                "src/mixed.rs",
                "2026-06-15T00:00:00+00:00",
                vec![
                    sym(
                        "fn",
                        "fn huge_test",
                        1,
                        LARGE_BEHAVIORAL_SYMBOL_LINES + 50,
                        true,
                    ),
                    sym(
                        "struct",
                        "struct HugeData",
                        1,
                        LARGE_BEHAVIORAL_SYMBOL_LINES + 50,
                        false,
                    ),
                    sym(
                        "fn",
                        "fn small_behavior",
                        1,
                        LARGE_BEHAVIORAL_SYMBOL_LINES - 1,
                        false,
                    ),
                ],
            )],
            &[],
        );
        assert!(
            of_kind(&r, "large_behavioral_symbol").is_empty(),
            "only non-test behavioral symbols over the threshold flag"
        );
    }

    #[test]
    fn current_file_decision_adjudicates_large_symbol() {
        let note = Note {
            id: "n1".into(),
            kind: "decision".into(),
            text: "large parser stays linear for now".into(),
            author: "llm".into(),
            target_kind: "file".into(),
            target_id: "src/big.rs".into(),
            audience: String::new(),
            created_at: "2026-06-16T00:00:00+00:00".into(),
        };
        let r = report(
            vec![cf(
                "src/big.rs",
                "2026-06-15T00:00:00+00:00",
                vec![sym(
                    "method",
                    "method parse_everything",
                    10,
                    LARGE_BEHAVIORAL_SYMBOL_LINES + 20,
                    false,
                )],
            )],
            &[note],
        );
        assert!(
            of_kind(&r, "large_behavioral_symbol").is_empty(),
            "a current file decision suppresses the open finding"
        );
        assert!(
            r.adjudicated
                .iter()
                .any(|s| s.kind == "large_behavioral_symbol"
                    && s.summary.contains("parse_everything")),
            "suppressed finding should be visible as adjudicated"
        );
    }
}

#[cfg(test)]
mod lexical_signal_tests {
    use super::*;
    use crate::types::Intent;

    fn intent(id: usize) -> Intent {
        Intent {
            id: format!("i{id}"),
            name: format!("feature {id} handler"),
            description:
                "feature processes requests and transforms data within the synthetic benchmark system"
                    .into(),
            abstraction_level: "feature".into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    #[test]
    fn ubiquitous_tokens_do_not_create_pairwise_twin_floods() {
        let snapshot = QuerySnapshot::from_parts(
            (0..40).map(intent).collect(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
        );
        let report = compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes: &[],
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap();
        assert!(
            report.open.iter().all(|s| s.kind != "twin_intents"),
            "{:?}",
            report
                .open
                .iter()
                .filter(|s| s.kind == "twin_intents")
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod source_fact_smell_tests {
    use super::*;
    use crate::types::{CodeFile, Note, StringLiteralFact, SymbolFact};

    fn sym(label: &str) -> SymbolFact {
        SymbolFact {
            label: label.into(),
            name: label.split_whitespace().last().unwrap_or(label).into(),
            kind: "fn".into(),
            visibility: "private".into(),
            line_start: 1,
            line_end: 20,
            is_test: false,
            string_literals: Vec::new(),
            panic_marker_count: 0,
            panic_markers: Vec::new(),
            body_hash: String::new(),
            shape_hash: String::new(),
        }
    }

    fn with_string(mut fact: SymbolFact, value: &str, line: usize) -> SymbolFact {
        fact.string_literals.push(StringLiteralFact {
            value: value.into(),
            line,
        });
        fact
    }

    fn with_panic(mut fact: SymbolFact, markers: &[&str]) -> SymbolFact {
        fact.panic_marker_count = markers.len();
        fact.panic_markers = markers.iter().map(|m| (*m).into()).collect();
        fact
    }

    fn cf(path: &str, facts: Vec<SymbolFact>) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: "2026-06-15T00:00:00+00:00".into(),
            imports: Vec::new(),
            symbols: facts.iter().map(|f| f.label.clone()).collect(),
            symbol_facts: facts,
            content_hash: String::new(),
        }
    }

    fn report(codefiles: Vec<CodeFile>, notes: &[Note]) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            codefiles,
            Some(Vec::new()),
        );
        compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes,
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }

    fn of_kind<'a>(r: &'a SmellReport, kind: &str) -> Vec<&'a Smell> {
        r.open.iter().filter(|s| s.kind == kind).collect()
    }

    #[test]
    fn repeated_long_string_contract_flags_once() {
        let text = "Run loom sync before reading the status output again";
        let r = report(
            vec![
                cf("src/a.rs", vec![with_string(sym("fn alpha"), text, 4)]),
                cf("src/b.rs", vec![with_string(sym("fn beta"), text, 9)]),
            ],
            &[],
        );
        let found = of_kind(&r, "string_contract_duplicate");
        assert_eq!(found.len(), 1, "repeated contract should flag: {found:?}");
        assert!(found[0].summary.contains("string contract repeated"));
        assert!(found[0].evidence.contains("src/a.rs:4"));
        assert!(found[0].evidence.contains("src/b.rs:9"));
    }

    #[test]
    fn short_or_path_like_strings_do_not_flag() {
        let r = report(
            vec![
                cf(
                    "src/a.rs",
                    vec![
                        with_string(sym("fn alpha"), "tiny label", 4),
                        with_string(sym("fn beta"), "src/db/queries/smells.rs", 5),
                    ],
                ),
                cf(
                    "src/b.rs",
                    vec![
                        with_string(sym("fn gamma"), "tiny label", 6),
                        with_string(sym("fn delta"), "src/db/queries/smells.rs", 7),
                    ],
                ),
            ],
            &[],
        );
        assert!(
            of_kind(&r, "string_contract_duplicate").is_empty(),
            "trivial strings should stay quiet"
        );
    }

    #[test]
    fn panic_marker_in_behavior_flags() {
        let r = report(
            vec![cf(
                "src/commands/run.rs",
                vec![with_panic(sym("fn run"), &["unwrap", "expect"])],
            )],
            &[],
        );
        let found = of_kind(&r, "panic_marker_risk");
        assert_eq!(found.len(), 1, "panic marker should flag: {found:?}");
        assert!(found[0].evidence.contains("markers=[unwrap, expect]"));
        assert!(found[0].evidence.contains("command/public surface"));
    }

    #[test]
    fn panic_marker_file_decision_adjudicates() {
        let note = Note {
            id: "n1".into(),
            kind: "decision".into(),
            text: "unwraps are construction-time invariants here".into(),
            author: "llm".into(),
            target_kind: "file".into(),
            target_id: "src/a.rs".into(),
            audience: String::new(),
            created_at: "2026-06-16T00:00:00+00:00".into(),
        };
        let r = report(
            vec![cf(
                "src/a.rs",
                vec![with_panic(sym("fn alpha"), &["unwrap"])],
            )],
            &[note],
        );
        assert!(of_kind(&r, "panic_marker_risk").is_empty());
        assert!(r.adjudicated.iter().any(|s| s.kind == "panic_marker_risk"));
    }
}

#[cfg(test)]
mod cycle_island_tests {
    use super::*;
    use crate::types::Intent;

    fn intent(id: &str, level: &str) -> Intent {
        Intent {
            id: id.into(),
            name: format!("intent {id}"),
            description: format!("does {id} things"),
            abstraction_level: level.into(),
            domain: String::new(),
            layer: String::new(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    fn rel(from: &str, to: &str) -> crate::types::RelatesTo {
        // Default to a GROUNDED (passing) verdict with a real last_inspected, so
        // a reciprocal pair built from two `rel(...)`s is a deliberate
        // double-grounding (the case the detector fires on).
        rel_st(from, to, "passing")
    }

    fn rel_st(from: &str, to: &str, status: &str) -> crate::types::RelatesTo {
        // Grounded statuses carry a timestamp; uninspected/independent do not —
        // mirrors how the store records verdicts vs mechanically-created edges.
        let last_inspected = match status {
            "passing" | "failing" | "needs_reverification" => "2026-06-15T00:00:00+00:00",
            _ => "",
        };
        crate::types::RelatesTo {
            id: format!("rt:{from}:{to}"),
            from_id: from.into(),
            to_id: to.into(),
            from_name: from.into(),
            to_name: to.into(),
            inspection_status: status.into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: last_inspected.into(),
            inspected_by: String::new(),
            priority_score: 0.0,
            notes: String::new(),
            discovery_class: String::new(),
            discovery_signals: Vec::new(),
            discovery_centrality: Default::default(),
        }
    }

    fn report(
        intents: Vec<Intent>,
        hierarchy: Vec<(String, String)>,
        relates: Vec<crate::types::RelatesTo>,
    ) -> SmellReport {
        report_with_notes(intents, hierarchy, relates, &[])
    }

    fn report_with_notes(
        intents: Vec<Intent>,
        hierarchy: Vec<(String, String)>,
        relates: Vec<crate::types::RelatesTo>,
        notes: &[Note],
    ) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            intents,
            hierarchy,
            relates,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            None,
        );
        compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes,
                vocab_terms: &[],
                layer_order: &[],
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }

    fn of_kind<'a>(r: &'a SmellReport, kind: &str) -> Vec<&'a Smell> {
        r.open.iter().filter(|s| s.kind == kind).collect()
    }

    // A system root parents a..c so they are never islands; the cycle, if any,
    // is purely in the RELATES_TO direction.
    fn rooted(children: &[&str]) -> (Vec<Intent>, Vec<(String, String)>) {
        let mut intents = vec![intent("sys", "system")];
        let mut hierarchy = Vec::new();
        for c in children {
            intents.push(intent(c, "feature"));
            hierarchy.push(("sys".to_string(), c.to_string()));
        }
        (intents, hierarchy)
    }

    #[test]
    fn a_long_directed_cycle_is_not_a_dependency_cycle() {
        // a→b→c→a: a directed loop of single-direction grounded edges. Stored
        // direction is a typing-order artifact, so this is NOT flagged — the
        // behavior change from the SCC era, and the central de-noising win.
        let (intents, hierarchy) = rooted(&["a", "b", "c"]);
        let relates = vec![rel("a", "b"), rel("b", "c"), rel("c", "a")];
        let r = report(intents, hierarchy, relates);
        assert!(
            of_kind(&r, "dependency_cycle").is_empty(),
            "a single-direction long cycle is noise, not a finding"
        );
    }

    #[test]
    fn a_reciprocal_grounded_pair_is_a_dependency_cycle() {
        let (intents, hierarchy) = rooted(&["a", "b"]);
        // BOTH directions grounded with a verdict → a deliberate double-assert.
        let relates = vec![rel("a", "b"), rel("b", "a")];
        let r = report(intents, hierarchy, relates);
        let found = of_kind(&r, "dependency_cycle");
        assert_eq!(
            found.len(),
            1,
            "one finding for the reciprocal pair: {found:?}"
        );
        assert!(found[0].summary.contains("intent a") && found[0].summary.contains("intent b"));
    }

    #[test]
    fn an_acyclic_chain_reports_no_cycle() {
        let (intents, hierarchy) = rooted(&["a", "b", "c"]);
        let relates = vec![rel("a", "b"), rel("b", "c")]; // single-direction DAG
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "dependency_cycle").is_empty());
    }

    #[test]
    fn an_uninspected_reciprocal_pair_does_not_fire() {
        // The saga fingerprint: `loom saga add` writes BOTH directions as
        // mechanically-created uninspected path edges (a round-trip journey).
        // Requiring a verdict on both directions excludes this — no false gate.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![
            rel_st("a", "b", "uninspected"),
            rel_st("b", "a", "uninspected"),
        ];
        let r = report(intents, hierarchy, relates);
        assert!(
            of_kind(&r, "dependency_cycle").is_empty(),
            "mechanically-created uninspected round-trip is not graph-hygiene debt"
        );
    }

    #[test]
    fn a_half_grounded_reciprocal_pair_does_not_fire() {
        // One direction grounded by an agent, the reverse left uninspected
        // (e.g. a saga path edge) — not a deliberate double-grounding.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel_st("a", "b", "passing"), rel_st("b", "a", "uninspected")];
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "dependency_cycle").is_empty());
    }

    #[test]
    fn an_independent_reverse_does_not_make_a_reciprocal_pair() {
        // a→b grounded, b→a explicitly independent (no relationship that way) —
        // only ONE real relationship, so not a redundant double-grounding.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel_st("a", "b", "passing"), rel_st("b", "a", "independent")];
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "dependency_cycle").is_empty());
    }

    #[test]
    fn reciprocal_pair_evidence_surfaces_disagreeing_verdicts() {
        // The two stored rows can silently disagree — the evidence must show it.
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel_st("a", "b", "passing"), rel_st("b", "a", "failing")];
        let r = report(intents, hierarchy, relates);
        let found = of_kind(&r, "dependency_cycle");
        assert_eq!(found.len(), 1);
        assert!(
            found[0].evidence.contains("passing") && found[0].evidence.contains("failing"),
            "evidence surfaces the disagreement: {}",
            found[0].evidence
        );
    }

    #[test]
    fn an_adjudicated_reciprocal_pair_is_suppressed() {
        // A decision note on the smaller-id anchor, newer than both edges'
        // last_inspected, suppresses the OPEN finding (shows as adjudicated).
        let (intents, hierarchy) = rooted(&["a", "b"]);
        let relates = vec![rel("a", "b"), rel("b", "a")];
        let note = Note {
            id: "n1".into(),
            kind: "decision".into(),
            text: "a and b are deliberate mutual peers".into(),
            author: "llm".into(),
            target_kind: "intent".into(),
            target_id: "a".into(), // the smaller-id anchor
            audience: String::new(),
            created_at: "2026-06-16T00:00:00+00:00".into(), // newer than the edges
        };
        let r = report_with_notes(intents, hierarchy, relates, &[note]);
        assert!(
            of_kind(&r, "dependency_cycle").is_empty(),
            "a current decision on the anchor suppresses the open finding"
        );
        assert!(
            r.adjudicated.iter().any(|a| a.kind == "dependency_cycle"),
            "it surfaces as adjudicated, not silently gone"
        );
    }

    #[test]
    fn a_disconnected_subgraph_is_an_island() {
        // sys—a is connected; x—y float free with no path to a system root.
        let mut intents = vec![intent("sys", "system"), intent("a", "feature")];
        intents.push(intent("x", "feature"));
        intents.push(intent("y", "feature"));
        let hierarchy = vec![("sys".to_string(), "a".to_string())];
        let relates = vec![rel("x", "y")];
        let r = report(intents, hierarchy, relates);
        let islands = of_kind(&r, "intent_island");
        assert_eq!(
            islands.len(),
            1,
            "the x—y component is one island: {islands:?}"
        );
        assert!(islands[0].summary.contains("intent x"));
        assert!(islands[0].summary.contains("intent y"));
    }

    #[test]
    fn a_fully_connected_graph_has_no_islands() {
        let (mut intents, mut hierarchy) = rooted(&["a", "b"]);
        intents.push(intent("c", "feature"));
        // c reaches the system root via a RELATES_TO edge, not hierarchy.
        hierarchy.retain(|_| true);
        let relates = vec![rel("a", "c")];
        let r = report(intents, hierarchy, relates);
        assert!(of_kind(&r, "intent_island").is_empty());
    }

    #[test]
    fn island_detector_is_unarmed_without_a_system_root() {
        // No system intent at all → nothing to be an island relative to.
        let intents = vec![intent("a", "feature"), intent("b", "feature")];
        let relates = vec![rel("a", "b")];
        let r = report(intents, vec![], relates);
        assert!(
            of_kind(&r, "intent_island").is_empty(),
            "silent without a system root (missing-root is a different problem)"
        );
    }

    fn with_aspect(id: &str, aspect: &str) -> Intent {
        let mut i = intent(id, "feature");
        aspect.clone_into(&mut i.aspect);
        i
    }

    #[test]
    fn ui_state_populated_without_empty_or_error_is_happy_path_only() {
        // A screen with a populated state but no empty/error sibling — the new
        // UI-state aspect family, sharing the happy_path_only detector.
        let intents = vec![
            intent("sys", "system"),
            intent("screen", "component"),
            with_aspect("populated_state", "populated"),
        ];
        let hierarchy = vec![
            ("sys".to_string(), "screen".to_string()),
            ("screen".to_string(), "populated_state".to_string()),
        ];
        let r = report(intents, hierarchy, vec![]);
        let hp = of_kind(&r, "happy_path_only");
        assert_eq!(
            hp.len(),
            1,
            "populated with no empty/error sibling fires: {hp:?}"
        );
        assert!(
            hp[0].summary.contains("populated") && hp[0].summary.contains("empty/error"),
            "names the UI-state family: {}",
            hp[0].summary
        );
    }

    #[test]
    fn behavioral_happy_without_sad_or_fallback_still_fires() {
        // Regression: the original behavioral family still works through the
        // generalized families table.
        let intents = vec![
            intent("sys", "system"),
            intent("feat", "component"),
            with_aspect("sunny", "happy"),
        ];
        let hierarchy = vec![
            ("sys".to_string(), "feat".to_string()),
            ("feat".to_string(), "sunny".to_string()),
        ];
        let r = report(intents, hierarchy, vec![]);
        let hp = of_kind(&r, "happy_path_only");
        assert_eq!(hp.len(), 1);
        assert!(hp[0].summary.contains("happy") && hp[0].summary.contains("sad/fallback"));
    }

    #[test]
    fn a_loading_only_state_does_not_trigger() {
        // `loading` is recognized but never a trigger and never required.
        let intents = vec![
            intent("sys", "system"),
            intent("screen", "component"),
            with_aspect("spinner", "loading"),
        ];
        let hierarchy = vec![
            ("sys".to_string(), "screen".to_string()),
            ("screen".to_string(), "spinner".to_string()),
        ];
        let r = report(intents, hierarchy, vec![]);
        assert!(
            of_kind(&r, "happy_path_only").is_empty(),
            "loading alone is not a triggering aspect"
        );
    }
}

#[cfg(test)]
mod transitive_layering_tests {
    use super::*;
    use crate::types::{CodeFile, Implements, Intent};

    fn intent(id: &str, layer: &str) -> Intent {
        Intent {
            id: id.into(),
            name: format!("intent {id}"),
            description: String::new(),
            abstraction_level: "component".into(),
            domain: String::new(),
            layer: layer.into(),
            source_refs: Vec::new(),
            status: "confirmed".into(),
            aspect: String::new(),
            tags: Vec::new(),
            visibility: "internal".into(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }
    fn cf(path: &str, imports: &[&str]) -> CodeFile {
        CodeFile {
            id: path.into(),
            path: path.into(),
            language: "rust".into(),
            last_modified: String::new(),
            imports: imports.iter().map(|s| s.to_string()).collect(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash: String::new(),
        }
    }
    fn imp(intent: &str, path: &str) -> Implements {
        Implements {
            id: format!("imp:{intent}:{path}"),
            intent_id: intent.into(),
            codefile_id: path.into(),
            intent_name: intent.into(),
            codefile_path: path.into(),
            inspection_status: "passing".into(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: String::new(),
            notes: String::new(),
            created_at: "t".into(),
        }
    }
    fn report(
        intents: Vec<Intent>,
        codefiles: Vec<CodeFile>,
        implements: Vec<Implements>,
        order: &[&str],
    ) -> SmellReport {
        let snapshot = QuerySnapshot::from_parts(
            intents,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            implements,
            codefiles,
            None,
        );
        let order: Vec<String> = order.iter().map(|s| s.to_string()).collect();
        compute_smells_from_parts(
            &snapshot,
            SmellInputs {
                notes: &[],
                vocab_terms: &[],
                layer_order: &order,
                proposed_hypotheses: &[],
                targets: &[],
            },
        )
        .unwrap()
    }
    fn of_kind<'a>(r: &'a SmellReport, kind: &str) -> Vec<&'a Smell> {
        r.open.iter().filter(|s| s.kind == kind).collect()
    }

    // storage → (unlayered) → presentation: each hop is exempt, but storage
    // depending on presentation across the chain violates the order.
    #[test]
    fn a_clean_chain_through_an_unlayered_intermediate_is_flagged() {
        let intents = vec![
            intent("a", "storage"),
            intent("m", ""), // unlayered intermediate hides the violation
            intent("c", "presentation"),
        ];
        let codefiles = vec![
            cf("a.rs", &["m.rs"]),
            cf("m.rs", &["c.rs"]),
            cf("c.rs", &[]),
        ];
        let implements = vec![imp("a", "a.rs"), imp("m", "m.rs"), imp("c", "c.rs")];
        let r = report(
            intents,
            codefiles,
            implements,
            &["presentation", "application", "storage"],
        );
        let found = of_kind(&r, "transitive_layering_violation");
        assert_eq!(
            found.len(),
            1,
            "the masked up-dependency must surface: {found:?}"
        );
        assert!(found[0].summary.contains("intent a") && found[0].summary.contains("intent c"));
        assert!(found[0].evidence.contains("unlayered"));
        // It is NOT a direct layering_violation (no single hop violates).
        assert!(of_kind(&r, "layering_violation").is_empty());
    }

    // All hops point DOWN the order (presentation→application→storage): clean
    // end-to-end, nothing to flag.
    #[test]
    fn a_downward_chain_is_clean() {
        let intents = vec![
            intent("a", "presentation"),
            intent("m", ""),
            intent("c", "storage"),
        ];
        let codefiles = vec![
            cf("a.rs", &["m.rs"]),
            cf("m.rs", &["c.rs"]),
            cf("c.rs", &[]),
        ];
        let implements = vec![imp("a", "a.rs"), imp("m", "m.rs"), imp("c", "c.rs")];
        let r = report(
            intents,
            codefiles,
            implements,
            &["presentation", "application", "storage"],
        );
        assert!(of_kind(&r, "transitive_layering_violation").is_empty());
    }

    // A DIRECT up-dependency is the direct check's job (6b), not the transitive
    // one — the transitive detector must not double-report it.
    #[test]
    fn a_direct_violation_is_not_double_reported_as_transitive() {
        let intents = vec![intent("a", "storage"), intent("c", "presentation")];
        let codefiles = vec![cf("a.rs", &["c.rs"]), cf("c.rs", &[])];
        let implements = vec![imp("a", "a.rs"), imp("c", "c.rs")];
        let r = report(
            intents,
            codefiles,
            implements,
            &["presentation", "application", "storage"],
        );
        assert_eq!(
            of_kind(&r, "layering_violation").len(),
            1,
            "direct check fires"
        );
        assert!(
            of_kind(&r, "transitive_layering_violation").is_empty(),
            "transitive check defers direct pairs to 6b"
        );
    }
}
