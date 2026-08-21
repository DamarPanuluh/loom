//! Role contracts — the PromptContract text for every work-item lane.
//!
//! Plane: judgment-plane routing (text assembly only). Each contract states
//! the role, mindset, allowed and forbidden actions, required evidence, and
//! the exact prefilled write-back command for one lane — the write-back must
//! target the store's gated paths, so a contract can never instruct a way
//! around INV-4/5/6. No store writes happen here.

use super::queues::prescreen_for;
use super::{q, EvidenceClause, HumanGate, HumanGateOption, PromptContract};
use crate::model::{Edge, EdgeKind, Node};
use crate::store::Store;
use crate::Result;

const FINDING_ADD_ACTION: &str = "loom finding add '<claim>' --source code_audit --kind code_audit --file <registered-codefile> --evidence '<file:line — observed fact>' --impact '<why it matters>' --confidence <0.0-1.0>";
const NON_BLOCKING_SMELL_RULE: &str = "silently skipping a material non-blocking smell; either capture it as a finding, reject it with evidence in triage, or leave it unmentioned because it is below capture threshold";

/// The elaboration contract: grow the surroundings the human forgot, decide
/// nothing that belongs to the human.
pub(super) fn elaborator_contract(
    intent: &Node,
    card: &crate::completeness::Scorecard,
) -> PromptContract {
    let name = q(&intent.name);
    PromptContract {
        role: "builder".into(),
        mindset: "The user may have only a partial idea and may not know Loom can help \
                  complete it. FIRST tell them, in plain language, that you can round out \
                  the idea: you will fill technical or repository-derivable gaps and ask \
                  only for product choices that require their judgment. Briefly reflect \
                  the core idea and material gaps; do not expose graph vocabulary unless \
                  asked. Then address each OPEN axis: create the missing artifact, waive \
                  it for a real reason, or, for a product decision, record and directly \
                  ask ONE plain-language question. Never answer for the user or treat \
                  silence as consent. Proposed scenarios are planned intents; the human-only \
                  ratify queue presents them for product authority before they count as wanted. \
                  When evidence shows an important unnamed behavior, offer Keep / Decline / Revise \
                  in product language and wait; mint or ratify only after the human answers. \
                  Silence is not wantedness."
            .into(),
        why_now: format!(
            "{} of {} completeness axes are open around this user-visible idea",
            card.open,
            card.axes.len()
        ),
        allowed_actions: vec![
            "engage the user first: explain that their idea does not need to be a complete specification; summarize what is already clear and say you can fill technical/inferable gaps while asking one understandable product question at a time".into(),
            format!(
                "scenarios: loom intent add --name '<what goes wrong / degraded path / boundary case>' --description '<falsifiable criterion>' --aspect <sad|fallback|edge_case> --visibility user_visible; then loom edge relate scenario-of '<that scenario>' {name}"
            ),
            format!(
                "prerequisites: loom edge relate requires {name} '<intent that must exist first>'"
            ),
            "boundary: work the quality queue; it proposes the applicable rule × code-bearing intent pair and requires an evidence-backed verdict".into(),
            "proof/journey: author or refine the Journey root, then let the derive, surface, and validate queues compile and run its proof profile".into(),
            format!(
                "product decision: loom question add \"<one crisp product question>\" --intent {name}; ask that question directly in plain language, offer a recommended default with consequences when useful, WAIT for the reply, then loom question answer <question> --answer '<the user’s answer>'"
            ),
            format!(
                "unnamed wantedness: if evidence shows an important behavior the human has not named (a sad path, a missing gate, or a rule the code already enforces that nobody authored), loom question add \"<one Keep / Decline / Revise question>\" --intent {name}; ask it in product language; WAIT; mint or ratify only after their answer"
            ),
            format!("waive: loom intent waive {name} <axis> --reason '<why it deliberately does not apply>'"),
            format!("only when a current external fact blocks elaboration: loom task add '<bounded question>' --kind research --why-external '<why current external knowledge is needed>' --preferred-source '<authoritative source guidance>' --target {name}"),
        ],
        forbidden_actions: vec![
            "deciding a product question yourself, continuing past it, or treating silence as consent — record it, ask the user, and wait".into(),
            "asking the user to choose implementation details the repository or engineering judgment can determine safely".into(),
            "assuming the user knows Loom commands, scorecards, axes, or graph terminology; translate the gap into ordinary product language".into(),
            "asking multiple product questions in one turn".into(),
            "proposing scenarios that restate the happy path".into(),
            "minting an unratified intent as a way to offer unnamed wantedness — ask Keep / Decline / Revise first, then mint only after the human answers".into(),
            "treating a finding, smell, or brainstormed feature as wantedness".into(),
            "waiving an axis just to close it (a waiver needs a real reason)".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "every open axis closed by an artifact, a waiver, or a question — never by silence".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "autonomous gap: one command per open axis (see the scorecard); product decision or unnamed wantedness: \
             loom question add \"<one crisp Keep / Decline / Revise question>\" --intent {name}, ask the user, then \
             loom question answer <question> --answer '<their reply>'; mint or ratify only after that answer; finally loom status"
        ),
        stop_condition: "if a product decision or unnamed wantedness offer is needed, record it, ask ONE Keep / Decline / Revise question, and wait for the user; no answer means no mint; otherwise, after addressing every open axis, return to loom status".into(),
        human_gate: None,
    }
}

/// Derive technical requirements from an authored Journey. The packet may
/// propose the mapping, but the acceptance boundary remains human-mediated:
/// one manifest, bound to the Journey's current semantic hash, is the unit the
/// human authorizes.
pub(super) fn derive_contract(
    journey: &Node,
    readiness: &crate::completeness::JourneyReadiness,
) -> PromptContract {
    let id = q(&journey.id);
    let gaps = if readiness.derive_gaps.is_empty() {
        "unrooted non-exempt intents".to_string()
    } else {
        readiness.derive_gaps.join("; ")
    };
    PromptContract {
        role: "builder".into(),
        mindset: "Treat the authored Journey as the root. Read its ordered semantic steps and map each one to the smallest falsifiable technical intents required to realize it. Reuse an existing intent when its criterion genuinely matches; otherwise propose a planned intent. The manifest is a proposal until the human authorizes that exact journey/hash mapping. Ask one plain-language product question when meaning is missing; never invent wantedness or edit code in this lane.".into(),
        why_now: format!("journey '{}' has derivation gaps: {gaps}", journey.name),
        allowed_actions: vec![
            format!("loom journey derive {id}"),
            "inspect the authored steps, existing current/stale derivations, and unrooted non-exempt intents in the packet".into(),
            "write a strict loom.journey-derivation/v1 manifest bound to the packet's journey_id and journey_hash: proposal_id, proposal_rationale, explicit create|reuse intent operations, criterion/rationale, relationship entries, and unresolved_question".into(),
            "reuse a matching Intent with operation=reuse and intent_id rather than duplicating it; use operation=create only for a new falsifiable criterion, and include every covered step id explicitly".into(),
            "reconcile declared requires|hierarchy relationships against the current graph; each relationship has id, kind, from, to, and rationale".into(),
            "when a true product choice is missing: loom question add '<one crisp question>' --journey <journey>; ask the human ONE plain-language question and wait".into(),
            format!("loom journey derive-accept {id} --manifest <file> --human-decision '<exact human answer>'"),
        ],
        forbidden_actions: vec![
            "editing production code — Build follows accepted derivation".into(),
            "mapping by name similarity without reading the step criterion".into(),
            "submitting duplicate relationship declarations, a requires/hierarchy cycle, or an unresolved_question to derive-accept".into(),
            "supplying --human-decision before the human answers or treating silence as approval".into(),
            "using a stale manifest whose journey_hash differs from the authored Journey".into(),
            "exempting an Intent without the dedicated canonical journey_exemption human-decision record".into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::Produces {
                what: "a complete hash-bound derivation manifest".into(),
            },
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "one conversationally reviewed hash-table batch containing proposal_id, journey_hash, manifest hash, create/reuse rows, step ids, criteria, rationales, and relationships; the exact human answer authorizing that table; every mapped Intent states a falsifiable technical criterion and stable Journey step ids".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom journey derive-accept {id} --manifest <file> --human-decision '<exact human answer>'"
        ),
        stop_condition: "wait for the human before acceptance; an accepted manifest creates an adopted Proposal and reconciled current Derives/relationships. An identical replay is idempotent; return to loom status".into(),
        human_gate: Some(super::derivation_human_gate(journey)),
    }
}

/// Compile one fully derived and grounded Journey into a real target-repo CLI
/// surface. Loom supplies the structured contract; the builder writes code in
/// the repository's own language and idiom.
pub(super) fn surface_contract(
    journey: &Node,
    readiness: &crate::completeness::JourneyReadiness,
) -> PromptContract {
    let id = q(&journey.id);
    let missing = readiness.surface_gaps.join("; ");
    PromptContract {
        role: "builder".into(),
        mindset: "Build a stable, production-owned black-box CLI surface in the target repository from Loom's structured surface contract. Prefer one unified consumer/administrative CLI that invokes the same application, API, or service boundary as the public interface. It may be operator-only, but it must not be a feature-gated proof binary, test fixture, mock-only path, or privileged shortcut around production behavior. Loom does not template-generate source. Preserve the authored command/argument/JSON-output contract, use the repository's established language and patterns, and expose a deeper debug/inspect mode without cluttering the ordinary command. Bind the accepted manifest to the Journey's current semantic hash.".into(),
        why_now: format!(
            "journey '{}' has current ratified derivations with realizing groundings but is not surfaced into live target-repository code: {}",
            journey.name,
            if missing.is_empty() { "surface missing" } else { missing.as_str() }
        ),
        allowed_actions: vec![
            format!("loom journey surface {id}"),
            "read the packet's operation bindings, typed arguments, real endpoints, expected JSON output, and derived Intent groundings".into(),
            "edit target-repository source in its existing CLI/application structure".into(),
            "loom codefile add <surface-source>".into(),
            "ground each accepted derived Intent to the code that realizes it with loom edge implement <intent> <codefile> --role realizes --locator <symbol>".into(),
            "write a loom.journey.surface/v1 manifest whose structured operations bind every authored step and whose InterfaceSurface exposes the real CodeFile".into(),
            format!("loom journey surface-accept {id} --manifest <file>"),
            "loom sync".into(),
        ],
        forbidden_actions: vec![
            "asking Loom to template-generate the target repository's source code".into(),
            "creating a test-only or feature-gated proof binary instead of a stable production-owned black-box CLI".into(),
            "bypassing the application/API/service boundary that the public behavior actually uses".into(),
            "accepting shell-string operations instead of structured argv/typed arguments and JSON outputs".into(),
            "surfacing before every current derivation is ratified, implemented, and realizing-grounded".into(),
            "recording a passing proof from the builder role — Validate runs the compiled Journey".into(),
            "using a stale manifest whose journey_hash differs from the authored Journey".into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesAnchor,
            EvidenceClause::Produces {
                what: "a hash-bound InterfaceSurface with complete operation bindings and an exposed CodeFile".into(),
            },
        ],
        required_evidence: "the real target-repository source location, complete step-to-operation bindings, exposed CodeFile, and a surface manifest bound to the current Journey semantic hash".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom journey surface-accept {id} --manifest <file>; loom sync"),
        stop_condition: "after surface acceptance, exposed live code, and sync make the Journey surfaced, return to loom status; compile/proof then belong to Validate".into(),
        human_gate: None,
    }
}

// ---- role contracts (see docs/llm-driver.md) -------------------------------

/// The exact re-record command that closes THIS edge's verdict, prefilled with
/// real endpoint names. Relates claims use the ergonomic name-resolving
/// `edge explore`; every other relationship kind is re-recorded by edge id —
/// `edge explore` would silently target a different (relates) edge.
fn verdict_write_back(edge: &Edge, from: &str, to: &str) -> String {
    match edge.kind {
        EdgeKind::Governs => format!(
            "loom rule verdict {} {} <passing|failing|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            q(from),
            q(to)
        ),
        EdgeKind::Validates => format!(
            "loom validation verdict {} <passed|failed|blocked> --evidence '…'   (blocked: add --reason '…')",
            q(from)
        ),
        EdgeKind::Relates => format!(
            "loom edge explore {} {} <ground|issue|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            q(from),
            q(to)
        ),
        _ => format!(
            "loom edge verdict {} <ground|issue|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
            edge.id
        ),
    }
}

pub(super) fn builder_contract(intent: &Node) -> PromptContract {
    let name = q(&intent.name);
    PromptContract {
        role: "builder".into(),
        mindset: "Use Loom first to understand why this work exists, which entities/files are \
                  likely relevant, and what prior evidence says; then inspect the relevant code \
                  before editing. Functions/symbols are locators, not intents. Do not self-certify \
                  quality or proofs."
            .into(),
        why_now: format!("intent '{}' is {} and not yet realized", intent.name, intent.status),
        allowed_actions: vec![
            "loom status".into(),
            "loom next --all".into(),
            format!("loom intent show {name}"),
            "loom codefile list".into(),
            "loom codefile show <file>".into(),
            "edit code".into(),
            format!("loom edge implement {name} <codefile> --locator <symbol>"),
            format!("loom intent update {name} --lifecycle implemented --reason '<what was built>'"),
            "loom sync".into(),
            FINDING_ADD_ACTION.into(),
            format!("only when a current external fact blocks implementation: loom task add '<bounded question>' --kind research --why-external '<why current external knowledge is needed>' --preferred-source '<authoritative source guidance>' --target {name}"),
        ],
        forbidden_actions: vec![
            "loom rule verdict passing (quality lane)".into(),
            "loom validation verdict passed (validator lane)".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesSpans { n: 1 },
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "Loom context checked, relevant code inspected, code written, locator confirmed, sync clean".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom edge implement {name} <codefile> --locator <symbol>; loom intent update {name} --lifecycle implemented --reason '<what was built>'"
        ),
        stop_condition: "after grounding + sync, return to loom status".into(),
        human_gate: None,
    }
}

/// A registration pointing at a file that no longer exists on disk. There is
/// nothing to read — the honest moves are unregistering, or registering the
/// successor file and re-grounding the affected intents there.
pub(super) fn missing_codefile_contract(codefile: &Node) -> PromptContract {
    let file = q(&codefile.name);
    PromptContract {
        role: "builder".into(),
        mindset: "This registered file is GONE from disk (deleted or renamed). Do not try to \
                  read it. If it was renamed/split, register the successor file(s) and ground \
                  the affected intents there; then unregister this ghost. If the behavior it \
                  carried is genuinely gone, unregister it and let sync settle the residue."
            .into(),
        why_now: format!("codefile '{}' is registered but missing from disk", codefile.name),
        allowed_actions: vec![
            format!("loom codefile show {file} (see which intents grounded here)"),
            "loom codefile add <successor-path> (when the file was renamed/split)".into(),
            format!("loom edge implement <intent> <successor> --locator <symbol> (re-ground before removing)"),
            format!("loom codefile remove {file}"),
            "loom sync".into(),
        ],
        forbidden_actions: vec![
            "grounding an intent to the missing file".into(),
            "inventing an intent to keep a dead registration alive".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "the successor registration + re-grounding, or the removal of the dead registration".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom codefile remove {file}  (after re-grounding any intents it carried)"
        ),
        stop_condition: "after unregistering (and any re-grounding) + sync, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn coverage_contract(store: &Store, codefile: &Node) -> Result<PromptContract> {
    let file = q(&codefile.name);
    let ignore_precedents = crate::coverage::ignore_rules(store)?;
    let unowned: std::collections::HashSet<String> =
        crate::coverage::unowned_names(store)?.into_iter().collect();
    let parent = std::path::Path::new(&codefile.name).parent();
    let mut neighboring_files = Vec::new();
    for neighbor in store.codefiles()? {
        if neighbor.id == codefile.id || std::path::Path::new(&neighbor.name).parent() != parent {
            continue;
        }
        let excluded = !crate::coverage::matching_ignore_rules(store, &neighbor.name)?.is_empty();
        let disposition = if excluded {
            "excluded"
        } else if crate::coverage::codefile_observed(&neighbor) {
            "observed"
        } else if unowned.contains(&neighbor.name) {
            "unowned"
        } else {
            "owned"
        };
        neighboring_files.push(serde_json::json!({
            "path": neighbor.name,
            "disposition": disposition,
        }));
    }
    neighboring_files.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    neighboring_files.truncate(20);

    Ok(PromptContract {
        role: "builder".into(),
        mindset: "Coverage triages a registered file; it does not authorize inventing graph truth. \
                  A tracked implementation file needs a realizing owner. One intent may realize in \
                  many files (sibling slices). BEFORE grounding, read the file, the ignore list, \
                  neighbors, and existing intents. Decide in this order: (1) sibling slice — an \
                  existing intent's criterion already LIVES here; add --role realizes with a locator \
                  for that slice, even if another file already realizes it; (2) distinct behavior — \
                  what lives here is a different observable criterion; record discovered_behavior \
                  and STOP (do not mint in coverage); (3) established exclusion when the file is \
                  outside the tracked surface; (4) unregister a mistaken registration. consumes \
                  records a call/host seam and NEVER owns the file or closes coverage. Never mark a \
                  mere caller as realizes, and never stretch an engine intent to cover a criterion \
                  it does not name."
            .into(),
        why_now: format!("codefile '{}' is registered but unowned", codefile.name),
        allowed_actions: vec![
            format!("loom codefile show {file}"),
            "loom intent list".into(),
            "loom ignore list".into(),
            "loom codefile list".into(),
            "read the file to see whether an existing criterion LIVES here (sibling slice), a distinct criterion lives here, or this is only a call/host seam".into(),
            format!("loom edge implement <intent> {file} --role realizes --locator <symbol>"),
            format!("loom edge implement <consumed-intent> {file} --role consumes --locator <seam>"),
            format!("loom codefile remove {file} (if it should not be tracked)"),
            "loom ignore add '<glob>' --reason '<existing category verbatim>' (only when established precedent applies)".into(),
            format!("loom finding add '<distinct behavior absent from graph>' --source coverage --kind discovered_behavior --evidence '<file:line>' --impact '<why it matters>' --file {file} (then stop for triage)"),
            "loom sync".into(),
        ],
        forbidden_actions: vec![
            "grounding a mere caller as --role realizes just to satisfy coverage".into(),
            "creating a new intent in the coverage lane".into(),
            "writing a new free-text ignore category when an existing category applies".into(),
            "loom rule verdict passing (quality lane)".into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "file read; ignore list and neighboring dispositions reviewed; then a sibling-slice realizes on an existing intent whose criterion lives here, an evidence-backed discovered_behavior finding for a distinct absent criterion, an established exclusion, or a reason to unregister".into(),
        evidence_template: None,
        examples: Some(serde_json::json!({
            "decision_order": [
                "sibling_slice",
                "record_discovery_and_stop",
                "follow_exclusion_precedent",
                "unregister"
            ],
            "grounding_pattern": {
                "sibling_slice": "One behavior may live in many files. If this file implements a slice of an existing intent's criterion, add realizes here with a locator. Do not mint a second intent for the same behavior.",
                "distinct_behavior": "If the observable criterion that lives here is not named by any intent, record discovered_behavior and stop. Mint the new intent outside coverage, then realize it here. The file may also consume the engine it calls.",
                "consumes": "Records a call or host seam. It does not own the file and does not close coverage."
            },
            "existing_ignore_precedents": ignore_precedents,
            "neighboring_file_dispositions": neighboring_files,
        })),
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom edge implement <existing-intent> {file} --role realizes --locator <symbol>   (or)   loom ignore add '<established-glob>' --reason '<existing category verbatim>'   (or)   loom codefile remove {file}   (or)   loom finding add '<distinct behavior absent from graph>' --source coverage --kind discovered_behavior --evidence '<file:line>' --impact '<why it matters>' --file {file}"
        ),
        stop_condition: "after a sibling-slice realizes, applying an established exclusion, or unregistering + sync, return to loom status; after recording distinct absent behavior, stop for triage without creating an intent".into(),
        human_gate: None,
    })
}

/// Analyze / re-verify contract for an asserted edge.
///
/// `owner_role` is the registry lane that may record the verdict (implements →
/// builder, relates → analyzer, …). It must appear as `prompt_contract.role`
/// so a driver following the contract selects an identity that can write —
/// hardcoding `"analyzer"` here is what made analyze packets advertise
/// `owner_role: builder` beside `prompt_contract.role: analyzer`. The
/// analyzer *mindset* (inspect, do not fix) stays regardless of which lane
/// owns the write.
pub(super) fn analyzer_contract(
    edge: &Edge,
    owner_role: &str,
    from_name: &str,
    to_name: &str,
) -> PromptContract {
    let write_back = verdict_write_back(edge, from_name, to_name);
    PromptContract {
        role: owner_role.into(),
        mindset: "Read both sides. Form a hypothesis before inspecting code. Record exactly what \
                  the code shows. Do not fix code; do not preserve the old verdict by assumption."
            .into(),
        why_now: format!("{} edge is {}", edge.kind, edge.status),
        allowed_actions: vec![
            "read codefiles, notes, prior evidence".into(),
            write_back.clone(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "edit code".into(),
            "record a verdict from name similarity or assumption".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "file/line locators, validation output, or runtime evidence".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn research_contract(task: &Node) -> PromptContract {
    let id = &task.id;
    PromptContract {
        role: "analyzer".into(),
        mindset: "Use the host's web search/browser to discover sources, then read actual pages. Prefer primary authoritative sources, record conflicts and the dates that make claims current, and preserve provenance. Search snippets are discovery only, never evidence. The outcome is advisory context: never convert it into a human preference, Fact verification, professional authority, certification, or code edits. It is valid to conclude expert review required, conflicting, or inconclusive.".into(),
        why_now: "a current external fact is explicitly missing and blocks informed work".into(),
        allowed_actions: vec![
            "use the host web search/browser; read actual pages, not snippets".into(),
            format!("loom task source-add {id} --url '<actual-http(s)-page>' --title '<page title>' --publisher '<publisher>' --source-kind <official_docs|standard|regulation|maintainer|primary|secondary> --quote '<substantive exact quote>' [--published-at <RFC3339>] [--fresh-until <RFC3339>]"),
            format!("loom task close {id} --result '<advisory result, conflicts/current dates, or expert-review-required/inconclusive conclusion>'"),
        ],
        forbidden_actions: vec!["use a search snippet as evidence or record a search-results URL".into(), "edit code".into(), "convert web claims into Fact evidence, verification, human preference, or professional certification".into()],
        evidence_clauses: Vec::new(),
        required_evidence: "at least one substantive quote from an actual page with complete URL/title/publisher/kind and Loom-stamped retrieval provenance; note conflicts and relevant dates".into(),
        evidence_template: None, examples: None, pre_screen: None, pre_screened_hits: Vec::new(),
        write_back: format!("loom task source-add {id} <all required source fields>  (repeat per actual page); loom task close {id} --result '<advisory synthesis>'"),
        stop_condition: "after recording sources and closing with an advisory result, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn exemplar_contract(edge: &Edge, pattern: &str, file: &str) -> PromptContract {
    let write_back = format!(
        "loom pattern exemplar verdict {} <ground|issue|independent> --criterion '…' --evidence '{file}:<line>' --confidence <0.0-1.0>",
        edge.id
    );
    PromptContract {
        role: "analyzer".into(),
        mindset: format!("Inspect the uniquely located symbol in {file} against Pattern '{pattern}'. Decide whether the code actually exemplifies the authored rationale and use boundaries; do not infer compliance from names."),
        why_now: format!("Exemplar edge is {} and cannot route guidance until reviewed", edge.status),
        allowed_actions: vec!["read the Pattern body and the located symbol".into(), write_back.clone(), FINDING_ADD_ACTION.into()],
        forbidden_actions: vec!["edit code".into(), "use generic relationship reasoning".into(), "record ground without inspecting the located source".into()],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "a file/line citation inside the uniquely located exemplar symbol".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording exactly one Exemplar verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn fixer_contract(
    edge: &Edge,
    from_name: &str,
    to_name: &str,
    compiler_owned: Option<(Node, String)>,
) -> PromptContract {
    // Which endpoint is the INTENT depends on the edge kind, and getting it
    // wrong produces a command that cannot resolve. `implements` and `relates`
    // run intent→target, so the intent is `from`; `governs` runs rule→intent
    // and `validates` runs validation→intent, so it is `to`.
    let intent_name = match edge.kind {
        EdgeKind::Governs | EdgeKind::Validates => to_name,
        _ => from_name,
    };
    // Compiler-owned Journey proof topology does not re-measure through sync:
    // `journey compile/run` is the only writer that can take those edges off
    // failing. Naming sync alone would send the fixer to a door that cannot
    // close their own packet.
    let rerun = compiler_owned.map(|(journey, profile)| {
        format!(
            "loom journey compile {} --profile {}; loom journey run {} --profile {}",
            q(&journey.id),
            q(&profile),
            q(&journey.id),
            q(&profile)
        )
    });
    PromptContract {
        role: "fixer".into(),
        mindset: "Use Loom first to understand the stale/failing criterion, linked entities, \
                  likely files, and prior evidence; then inspect the relevant code before editing. \
                  Repair the actual broken behavior at its root cause, not the symptom. Code moving \
                  is not behavior changing. If the product changed, route through intent update, not \
                  a silent code change. After the fix, sync re-opens this claim and routes \
                  re-measurement to its owning lane — do not record the verdict from the fixer hat."
            .into(),
        why_now: format!("{} edge is failing", edge.kind),
        allowed_actions: vec![
            "loom status".into(),
            "loom next --all".into(),
            format!("loom edge show {}", edge.id),
            format!("loom intent show {}", q(intent_name)),
            "loom codefile show <file>".into(),
            "edit code".into(),
            "loom sync".into(),
            "loom edge implement / loom edge retarget (re-ground if the fix moved code)".into(),
            // The case with no source to repair: the behavior was removed on
            // purpose and the graph still claims it. Without this the packet
            // says "fix the root cause" at a worker who has correctly
            // established there is nothing to fix, and offers them no move.
            format!(
                "loom intent retire {} --reason '<why this behavior no longer exists>'  \
                 (ONLY if the behavior was deliberately removed — not because the proof is hard)",
                q(intent_name)
            ),
            FINDING_ADD_ACTION.into(),
        ]
        .into_iter()
        .chain(rerun.clone())
        .collect(),
        forbidden_actions: vec![
            "recording the passing verdict yourself (the owning lane re-measures after sync)".into(),
            "suppress the symptom without a root-cause fix".into(),
            "retiring an intent because its proof is inconvenient — retire means the behavior \
             is gone, not that proving it is hard"
                .into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "Loom context checked, relevant code inspected, code change, sync clean, the failing criterion now addressed at its cause".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: match &rerun {
            Some(rerun) => format!(
                "fix the source at root cause, then loom sync; this claim is compiler-owned Journey \
                 proof topology, so sync alone cannot re-measure it — re-run the profile: {rerun}. \
                 If the behavior was deliberately removed, retire the intent instead: the failure is \
                 then the graph being out of date, and that IS the root cause"
            ),
            None => "fix the source at root cause, then loom sync — sync re-opens this claim as \
                     needs_reverification and its owning lane re-measures it. If the behavior was \
                     deliberately removed, retire the intent instead: the failure is then the \
                     graph being out of date, and that IS the root cause"
                .into(),
        },
        stop_condition: "after the fix + sync, return to loom status".into(),
        human_gate: None,
    }
}

/// The fix contract for a finding adjudicated `needed`. The packet grants no
/// adjudication authority: the repair reopens the finding through the
/// adjudication stamp (any edit to the cited file stales an open verdict),
/// and triage re-serves it for the analyzer's `resolved`.
pub(super) fn needed_finding_fix_contract(id: &str, file: Option<&str>) -> PromptContract {
    let file_action = match file {
        Some(file) => format!("loom codefile show {}", q(file)),
        None => "loom codefile show <cited file>".into(),
    };
    PromptContract {
        role: "fixer".into(),
        mindset: "A triager already judged this finding `needed`: the reason says what to do, \
                  the evidence says where. Read the cited code fresh, repair the root cause the \
                  evidence names — not the symptom, not the finding text. After the fix, sync: \
                  the edit stales the open adjudication, the finding re-enters triage, and the \
                  analyzer records `resolved` from the observed repair."
            .into(),
        why_now: "an adjudicated-needed finding is routed repair work; a needed verdict nobody serves is a decision that silently expires".into(),
        allowed_actions: vec![
            format!("loom finding list --state needed"),
            file_action,
            "read the cited code and the finding's evidence".into(),
            "edit code".into(),
            "loom sync".into(),
            "loom edge implement / loom edge retarget (re-ground if the fix moved code)".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            format!(
                "loom finding verdict {id} resolved — the analyzer records resolution after \
                 observing the repair, never the fixer"
            ),
            "suppress the symptom without a root-cause fix".into(),
            "rewording the finding instead of repairing the code".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "the finding's cited spans read, the repair made at the named cause, sync clean afterwards".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: "fix the cited code at root cause, then loom sync — the edit stales this finding's open adjudication and triage re-serves it for the resolved verdict".into(),
        stop_condition: "after the repair + sync, return to loom status".into(),
        human_gate: None,
    }
}

/// A `needed` finding whose named repair writes validator-owned facts
/// (`journey compile/run`, `validation add/run`). Served on the validate lane
/// so the packet's owner can actually perform the write.
pub(super) fn needed_finding_validate_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "validator".into(),
        mindset: "A triager already judged this finding `needed`: the named repair is a proof \
                  run, not a code edit. Compile and run the current Journey proof profile (or \
                  register and run a validation if no Journey exists). Do not edit code to make \
                  the proof pass. After a passing S3-or-stronger run, return to status — the \
                  detector drops the finding when the proof holds."
            .into(),
        why_now: "an adjudicated-needed proof-depth finding is validate work; serving it to fixer \
                  names a write the lane gate refuses"
            .into(),
        allowed_actions: vec![
            format!("loom finding list --state needed"),
            "loom journey compile <journey> --profile proof".into(),
            "loom journey run <journey> --profile proof".into(),
            "loom validation add --name '<what it proves>' --type test --command '<cmd>' --intent <intent> then loom validation run <name>".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "editing source code to make the proof pass".into(),
            format!(
                "loom finding verdict {id} resolved — the detector drops this finding when the proof holds; do not self-adjudicate"
            ),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::ProofStrengthAtLeast { grade: "S3".into() },
        ],
        required_evidence: "the proof run Loom performed, including exit status/output".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: "loom journey compile <journey> --profile proof; loom journey run <journey> --profile proof".into(),
        stop_condition: "stop when the named intent has a passing S3-or-stronger journey proof, or when Loom records the honest failure; then return to loom status".into(),
        human_gate: None,
    }
}

/// A `needed` finding whose named repair writes analyzer-owned facts
/// (`relates`). Served on the analyze lane so the packet's owner can record
/// the missing relationship.
pub(super) fn needed_finding_analyze_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "A triager already judged this finding `needed`: the named repair is recording \
                  the missing relationship, not editing production code. Read the owning intents, \
                  then record a `relates` edge (or explore an existing pair). After the write, \
                  sync so the detector can drop the coupling smell."
            .into(),
        why_now: "an adjudicated-needed undeclared-coupling finding is analyze work; serving it \
                  to fixer names a write the lane gate refuses"
            .into(),
        allowed_actions: vec![
            format!("loom finding list --state needed"),
            "loom edge relate relates <intent-a> <intent-b>".into(),
            "loom edge explore <intent-a> <intent-b> ground --criterion '…' --evidence 'file:line — …' --confidence <0.0-1.0>".into(),
            "loom sync".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "edit code".into(),
            format!(
                "loom finding verdict {id} resolved — sync drops the smell once the relationship exists; do not self-adjudicate"
            ),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "the owning intents read, the relates edge recorded (or an honest independent verdict on explore)".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: "loom edge relate relates <intent-a> <intent-b>; loom sync".into(),
        stop_condition: "after recording the relationship and syncing, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn quality_contract(
    store: &Store,
    edge: &Edge,
    rule_name: &str,
    intent_name: &str,
) -> Result<PromptContract> {
    // The rule (edge.from) carries the inspection protocol — embed it so verdicts
    // are consistent across sessions (see docs/llm-driver.md quality contract).
    let rule = store.get_node(&edge.from_id)?;
    let hits = prescreen_for(store, rule.as_ref(), &edge.to_id)?;
    Ok(quality_contract_body(
        rule.as_ref(),
        &format!("governs edge is {}", edge.status),
        rule_name,
        intent_name,
        hits,
    ))
}

/// The quality prompt contract, shared by the edge path (re-measure) and the
/// unmeasured-pair path (first measurement, where no edge exists yet).
pub(super) fn quality_contract_body(
    rule: Option<&Node>,
    why_now: &str,
    rule_name: &str,
    intent_name: &str,
    screen: super::queues::PreScreen,
) -> PromptContract {
    let body = rule.map(|n| n.body.clone()).unwrap_or_default();
    let guide = body
        .get("inspection_guide")
        .and_then(|v| v.as_str())
        .unwrap_or("inspect the code against this rule")
        .to_string();
    let hints: Vec<String> = body
        .get("detection_hints")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let evidence_template = body
        .get("evidence_template")
        .cloned()
        .filter(|v| !v.is_null());
    // Only the sides that actually exist. A rule carrying the keys with null
    // values used to produce `{"passing":null,"failing":null}`, which renders
    // as a section header with nothing under it — noise in a packet whose whole
    // value is being readable.
    let examples = {
        let mut pairs = serde_json::Map::new();
        for (key, value) in [
            ("passing", body.get("passing_example")),
            ("failing", body.get("failing_example")),
        ] {
            if let Some(v) = value.filter(|v| !v.is_null()) {
                pairs.insert(key.to_string(), v.clone());
            }
        }
        (!pairs.is_empty()).then_some(serde_json::Value::Object(pairs))
    };
    let write_back = format!(
        "loom rule verdict {} {} <passing|failing|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
        q(rule_name),
        q(intent_name)
    );
    let mut allowed = vec![
        "loom codefile show <file>".into(),
        "read the grounded code".into(),
        write_back.clone(),
        FINDING_ADD_ACTION.into(),
    ];
    allowed.extend(hints.into_iter().map(|h| format!("hint: {h}")));
    let template_note = if evidence_template.is_some() {
        " Phrase evidence with the rule's evidence_template so verdicts are comparable across sessions."
    } else {
        ""
    };
    let hits_note = if screen.hits.is_empty() {
        ""
    } else {
        " Machine pre-screened hits are attached: confirm or refute EVERY hit before your verdict — they are candidates, not conclusions."
    };
    // A scan that ran and found nothing is the evidence an ABSENCE rule needs.
    // Reporting only hits made "loom looked and found none" indistinguishable
    // from "loom never looked", so the worker re-grepped what loom had already
    // grepped and could not cite the scan even when it was the whole answer.
    let pre_screen = screen.ran.then(|| {
        if screen.hits.is_empty() {
            format!(
                "loom scanned {} pattern(s) over {} grounded file(s) and found NOTHING. \
                 That absence IS the evidence for this rule — cite it rather than re-grepping.",
                screen.patterns, screen.files
            )
        } else {
            format!(
                "loom scanned {} pattern(s) over {} grounded file(s) and found {} candidate(s), \
                 listed below. Confirm or refute each.",
                screen.patterns,
                screen.files,
                screen.hits.len()
            )
        }
    });
    PromptContract {
        role: "quality".into(),
        mindset: format!(
            "Measure this rule at the highest honest altitude. Follow the rule's inspection guide; \
             do not invent your own protocol. independent requires evidence of non-applicability.\
             {template_note}{hits_note} Guide: {guide}"
        ),
        why_now: why_now.into(),
        allowed_actions: allowed,
        forbidden_actions: vec![
            "edit code".into(),
            "mark passing without inspecting".into(),
            "mark independent without evidence the rule does not apply".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "file/line locators showing compliance, violation, or non-applicability"
            .into(),
        evidence_template,
        examples,
        pre_screen,
        pre_screened_hits: screen.hits,
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn validator_contract(
    store: &Store,
    edge: &Edge,
    val_name: &str,
    intent_name: &str,
) -> Result<PromptContract> {
    let val = store.get_node(&edge.from_id)?;
    if let Some(validation) = &val {
        if let Some((journey, profile)) =
            crate::completeness::compiler_owned_journey_validation(store, validation)?
        {
            let mut contract = journey_proof_contract_for_profile(&journey, &profile);
            contract.why_now = format!(
                "compiler-owned {} edge is {}; settle it with `loom journey run {} --profile {}`",
                edge.kind, edge.status, journey.id, profile
            );
            return Ok(contract);
        }
    }
    let command = val
        .as_ref()
        .and_then(|n| {
            n.body
                .get("command")
                .and_then(|c| c.as_str())
                .map(String::from)
        })
        .unwrap_or_default();
    // A runnable proof cannot be verdicted by hand — the floor demands a Run,
    // and offering `validation verdict` here sends the worker at a wall the
    // write boundary will refuse. Offer it ONLY for a manual check, which is
    // the one shape that has no command for loom to execute.
    let validation_type = val
        .as_ref()
        .and_then(|n| n.body.get("type").and_then(|t| t.as_str()))
        .unwrap_or("test");
    let manual_check = validation_type == "manual_check";
    let runnable = !command.trim().is_empty() && !manual_check;
    let unconfigured = command.trim().is_empty() && !manual_check;
    let write_back = if runnable {
        format!(
            "loom observe --for {} -- {}   (or)   loom validation run {}",
            q(intent_name),
            if command.is_empty() {
                "<cmd>"
            } else {
                &command
            },
            q(intent_name)
        )
    } else if unconfigured {
        format!(
            "loom validation update {} --command '<runnable-command>'; loom validation run {}",
            q(val_name),
            q(val_name)
        )
    } else {
        verdict_write_back(edge, val_name, intent_name)
    };
    Ok(PromptContract {
        role: "validator".into(),
        mindset:
            "Run it; do not guess. Record exactly what the command produced. Do not edit code \
                  to make a proof pass. A blocked proof is honest — record it with a reason."
                .into(),
        why_now: format!("validates edge is {}", edge.status),
        allowed_actions: {
            let mut actions = Vec::new();
            if runnable {
                // A stored command containing shell operators cannot be pasted
                // after `--`: the calling shell splits it, so the wrapper sees
                // only the first clause, mints a proof for THAT, and leaves
                // this edge unrun. The queue then serves the same item forever
                // while every run appears to succeed.
                //
                // `loom validation run` executes the stored command exactly, so
                // it is the correct offer for those — and the wrapper leads for
                // the simple case, where pasting is safe and the worker gets to
                // keep a command they were going to type anyway.
                let shell_shaped = command.contains("&&")
                    || command.contains("||")
                    || command.contains(';')
                    || command.contains('|')
                    || command.contains('>');
                if shell_shaped {
                    actions.push(format!("loom validation run {}", q(intent_name)));
                    actions.push(format!(
                        "loom observe --for {} -- sh -c {}",
                        q(intent_name),
                        q(&command)
                    ));
                } else {
                    actions.push(format!(
                        "loom observe --for {} -- {}",
                        q(intent_name),
                        command
                    ));
                    actions.push(format!("loom validation run {}", q(intent_name)));
                }
            } else if manual_check {
                actions.push("<no command — this is a manual check>".into());
                actions.push(verdict_write_back(edge, val_name, intent_name));
            } else {
                actions.push(format!("loom validation show {}", q(val_name)));
                actions.push(format!(
                    "loom validation update {} --command '<runnable-command>'",
                    q(val_name)
                ));
                actions.push(format!("loom validation run {}", q(val_name)));
            }
            actions.push(FINDING_ADD_ACTION.into());
            actions
        },
        forbidden_actions: vec![
            "edit code to make the proof pass".into(),
            "mark passed without observed proof".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::VerificationAtLeast {
                level: "verified".into(),
            },
        ],
        required_evidence: if runnable || unconfigured {
            "a run loom performed — its exit code and output. A reported outcome is refused".into()
        } else {
            "what you observed in the manual check, citing file:line or a journal entry".into()
        },
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the result, return to loom status".into(),
        human_gate: None,
    })
}

/// Contract for an implemented intent whose proof floor is still open. Distinct
/// from `validator_contract`, which re-runs an EXISTING pending proof: here the
/// proof must be added or strengthened. Journey-root S3 is routed separately
/// through [`journey_proof_contract`]; an Intent proof closes only the S2
/// behavioral floor and never invents a legacy executable Journey spec.
pub(super) fn unproven_contract(
    intent: &Node,
    proof: crate::proofstrength::ProofAssessment,
) -> PromptContract {
    let name = q(&intent.name);
    let weak_passing = proof.any_passing && !proof.meaningful_passing;
    let best = proof
        .best_passing_strength
        .unwrap_or(crate::proofstrength::Strength::S0)
        .as_str();
    PromptContract {
        role: "validator".into(),
        mindset: if weak_passing {
            format!(
                "This proof ran and passed, but its {best} grade proves only liveness. Strengthen \
                 the proof so it would FAIL if this behavior broke: add an output/content assertion, \
                 update or replace the registered command/spec, and rerun it."
            )
        } else {
            "An implemented claim with no passing proof is a claim, not truth. Write a proof \
             that would FAIL if this behavior broke — a check that only asserts the process \
             exited 0 proves liveness, not behavior."
                .into()
        },
        why_now: if weak_passing {
            format!("implemented, proof ran and passed at {best}, but meaningful proof requires S2")
        } else if proof.any_registered {
            "implemented, proof registered, none passing".into()
        } else {
            "implemented with no registered proof at all".into()
        },
        allowed_actions: {
            let mut actions = vec![
                format!("loom intent show {}", q(&intent.id)),
                "read the grounded files listed in this packet's read set".into(),
            ];
            if weak_passing {
                actions.extend([
                    "strengthen or replace the proof with an output/content assertion (for example stdout_contains/body/exists, or a runner command that reports passing assertions)".into(),
                    "loom validation update <validation> --command '<command whose output/content assertion establishes the behavior>'".into(),
                    format!("loom validation run {name}"),
                ]);
            } else {
                actions.extend([
                    format!(
                        "loom validation add --name '<what it proves>' --type test --command '<cmd>' --intent {name}"
                    ),
                    format!("loom validation run {name}"),
                ]);
            }
            actions.push(FINDING_ADD_ACTION.into());
            actions
        },
        forbidden_actions: vec![
            "recording a passing result without running the command".into(),
            "asserting only an exit code when the behavior has observable output".into(),
            "editing code to make a proof pass".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::ProofStrengthAtLeast { grade: "S2".into() },
        ],
        required_evidence:
            "the command loom ran, its exit status, and the assertion that would have caught a \
             regression"
                .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: if weak_passing {
            format!(
                "loom validation update <validation> --command '<stronger command with an output/content assertion>'  then  loom validation run {name}  (or add a new S2+ proof)"
            )
        } else {
            format!(
                "loom validation add --name '<what it proves>' --type test --command '<cmd>' --intent {name}  then  loom validation run {name}"
            )
        },
        stop_condition: "stop when this intent has a passing meaningful proof at S2 or stronger; Journey-root S3 is a separate compiled Journey packet".into(),
        human_gate: None,
    }
}

/// A compiled Journey whose consumer-plane proof has not yet earned S3.
/// Compilation creates the validation-specific Proves/Validates/Calls/
/// Exercises closure; running records only what Loom actually observes.
pub(super) fn journey_proof_contract(journey: &Node) -> PromptContract {
    journey_proof_contract_for_profile(journey, "proof")
}

pub(super) fn journey_proof_contract_for_profile(journey: &Node, profile: &str) -> PromptContract {
    let id = q(&journey.id);
    let profile = q(profile);
    PromptContract {
        role: "validator".into(),
        mindset: "Compile the current authored Journey into its proof profile, then run that exact profile. Do not hand-author a sibling Validation or attach an intent-wide witness: Journey compile owns the validation-specific Proves/Validates/Calls/Exercises closure. Run it; do not guess, and do not edit code to make the proof pass.".into(),
        why_now: format!(
            "compiled Journey '{}' has no current passing S3 proof through its surfaced CLI",
            journey.name
        ),
        allowed_actions: vec![
            format!("loom journey compile {id} --profile {profile}"),
            format!("loom journey run {id} --profile {profile}"),
            "inspect the compiled validation's call evidence and exact failure output".into(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "loom validation add --journey (there is no alternate proof door)".into(),
            "editing source code to make the proof pass".into(),
            "recording a passing verdict without a Loom-observed run".into(),
            "relying on an intent-wide verifies grounding instead of the compiled validation-specific witness".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::ProofStrengthAtLeast { grade: "S3".into() },
        ],
        required_evidence: "the proof-profile run Loom performed, including exit status/output and the validation-specific call witness through the surfaced CLI".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom journey compile {id} --profile {profile}; loom journey run {id} --profile {profile}"
        ),
        stop_condition: "stop when the current Journey has a passing S3 proof through its surfaced CLI, or when Loom records the honest failure/blocker; then return to loom status".into(),
        human_gate: None,
    }
}

/// Independent re-inspection of a verdict recorded below the confidence floor.
/// The reviewer forms their own hypothesis BEFORE reading the recorded
/// evidence, then confirms or overturns with honest confidence.
pub(super) fn reviewer_contract(
    edge: &Edge,
    owner_role: &str,
    from_name: &str,
    to_name: &str,
    review_floor: f64,
) -> PromptContract {
    let write_back = verdict_write_back(edge, from_name, to_name);
    PromptContract {
        role: owner_role.into(),
        mindset: "This verdict was recorded honestly but with low confidence — it is not settled \
                  truth. Re-inspect INDEPENDENTLY: form your own hypothesis from the code before \
                  reading the recorded criterion/evidence, then compare. Confirm or overturn; \
                  either way, record your own evidence and your own honest confidence."
            .into(),
        why_now: format!(
            "{} verdict stands at confidence {:.2}, below the {} review floor",
            edge.kind, edge.confidence, review_floor
        ),
        allowed_actions: vec![
            "read both endpoints and the grounded code FIRST".into(),
            format!("loom edge show {} (recorded criterion/evidence — read AFTER forming your own view)", edge.id),
            write_back.clone(),
            FINDING_ADD_ACTION.into(),
        ],
        forbidden_actions: vec![
            "edit code".into(),
            "rubber-stamping the prior verdict without independent inspection".into(),
            "inheriting the prior confidence instead of stating your own".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "fresh file/line or runtime evidence; state explicitly whether the prior verdict was confirmed or overturned".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back,
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

/// Adversarial review challenges one exact settled verdict revision without
/// granting the reviewer authority to rewrite it. A credible counterexample
/// becomes a Finding; Triage remains the adjudication seam.
pub(super) fn adversarial_reviewer_contract(
    edge: &Edge,
    from_name: &str,
    to_name: &str,
    prior_profile: Option<&str>,
) -> PromptContract {
    let avoid = prior_profile
        .map(|profile| format!(" Prefer an executor profile other than '{profile}'."))
        .unwrap_or_else(|| {
            " The prior executor profile is unavailable; declare your own profile so independence is auditable.".into()
        });
    PromptContract {
        role: "analyzer".into(),
        mindset: format!(
            "Act as a refutation-biased reviewer. Form a concrete falsification hypothesis from the endpoints and code BEFORE reading the prior verdict evidence; then attack boundary cases, negative paths, and hidden assumptions. A failed attack records survived, not proof of perfection. A credible break records counterexample and lets Triage decide.{}",
            avoid
        ),
        why_now: format!(
            "the current {} claim '{} —{}→ {}' is in the bounded high-risk frontier and has no adversarial attempt against this verdict revision",
            edge.kind, from_name, edge.kind, to_name
        ),
        allowed_actions: vec![
            "read both endpoints and grounded code before reading the prior verdict evidence".into(),
            format!("loom edge show {} (read only after writing down your own falsification hypothesis)", edge.id),
            "run focused read-only checks or tests that exercise the hypothesis".into(),
            format!("loom challenge record {} <survived|counterexample|inconclusive> --hypothesis '<what would falsify this claim>' --evidence '<what you tried and observed, including file:line or journal:id>' [--impact '<consequence>'] --confidence <0.0-1.0>", edge.id),
        ],
        forbidden_actions: vec![
            "edit source code while reviewing".into(),
            "replace the challenged edge verdict directly".into(),
            "read the prior criterion/evidence before forming an independent hypothesis".into(),
            "treat an inconclusive attempt as either confirmation or refutation".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        required_evidence: "a substantive falsification hypothesis, a substantive account of the attempt, and at least one live file:line or journal:id citation; counterexample also requires impact".into(),
        evidence_clauses: vec![
            EvidenceClause::CitesSpans { n: 1 },
            EvidenceClause::VerificationAtLeast { level: "cited".into() },
            EvidenceClause::Prose,
        ],
        evidence_template: None,
        examples: None,
        pre_screened_hits: Vec::new(),
        pre_screen: None,
        write_back: format!(
            "loom challenge record {} <survived|counterexample|inconclusive> --hypothesis '<falsifiable attack>' --evidence '<attempt + file:line or journal:id>' [--impact '<required for counterexample>'] --confidence <0.0-1.0>",
            edge.id
        ),
        stop_condition: "stop when one current Challenge fact exists for this exact Verdict revision; a counterexample must also have created its untriaged Finding atomically, then return to loom status".into(),
        human_gate: None,
    }
}

pub(super) fn prove_contract(hyp: &Node) -> PromptContract {
    let name = q(&hyp.name);
    PromptContract {
        role: "analyzer".into(),
        mindset:
            "An idea is not work until its claim survives contact with the code. Form your own \
                  reading first, then prove or refute the claim. Unproven ideas die honestly."
                .into(),
        why_now: format!("hypothesis '{}' is unproven", hyp.name),
        allowed_actions: vec![
            "read the targeted code".into(),
            format!("loom hypothesis prove {name} supported|refuted --evidence '…'"),
            format!(
                "if SUPPORTED: loom hypothesis adopt {name} — promotes the proven idea to a planned build intent (nothing else re-queues it)"
            ),
            "only when a current external fact blocks the verdict: loom task add '<bounded question>' --kind research --why-external '<why current external knowledge is needed>' --preferred-source '<authoritative source guidance>'".into(),
        ],
        forbidden_actions: vec![
            "adopt the hypothesis before proving it".into(),
            "edit code".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "code evidence that the claim holds or fails".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom hypothesis prove {name} <supported|refuted> --evidence '…'"),
        stop_condition: "a SUPPORTED verdict is not work until adopted (loom hypothesis adopt) — adopt it to spawn build work; a REFUTED verdict stands as an honest record. Then return to loom status.".into(),
        human_gate: None,
    }
}

pub(super) fn triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Look and decide; do not fix here. Every material finding must become needed, justified, rejected, deferred, blocked, duplicate, or resolved with a concrete reason. Use resolved only after observing the repair."
            .into(),
        why_now: "an evidence-backed finding is unjudged (or its prior judgment went stale when the file changed)".into(),
        allowed_actions: vec![
            format!("loom finding verdict {id} needed --reason <what to do>"),
            format!("loom finding verdict {id} justified --reason <why it is acceptable> --evidence <file:line in the flagged code, or journal:ref>"),
            format!("loom finding verdict {id} rejected --reason <why it is false or below threshold> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} deferred --reason <why not scheduled now> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} blocked --reason <what it waits on>"),
            format!("loom finding verdict {id} duplicate --reason <duplicate finding id or target> --evidence <the duplicate's id, or a file:line>"),
            format!("loom finding verdict {id} resolved --reason <observed repair and proof> --evidence <file:line of the fix, or journal:ref>"),
        ],
        forbidden_actions: vec![
            "edit code here (mark it needed, then fix in build/fix)".into(),
            "justified without a concrete reason".into(),
        ],
        evidence_clauses: vec![EvidenceClause::CitesSpans { n: 1 }],
        required_evidence: "a concrete reason; and for a settling verdict (justified/rejected/deferred/duplicate/resolved) a cited --evidence (file:line in the flagged file, or a journal:ref) — the reason says WHAT you decided, the evidence says what you decided it FROM"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom finding verdict {id} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…' --evidence '<file:line for a settling verdict; omit for needed/blocked>'"),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

/// Structural size/complexity findings need cohesion judgment — not a mechanical
/// "length is intentional" closeout. Owner-count is a hint, not the verdict.
pub(super) fn structural_finding_triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Judge cohesion, not line count. Read the flagged file's top-level modules/handlers. One concern → justified; a catch-all bag of unrelated commands/surfaces → needed (split). Do not fix here."
            .into(),
        why_now: "a structural detector flagged size or complexity; calibrate already set the gate — this packet is about whether the file is one concern".into(),
        allowed_actions: vec![
            format!("loom finding verdict {id} needed --reason <split plan: which concerns to separate>"),
            format!("loom finding verdict {id} justified --reason <the single cohesive concern> --evidence <file:line showing that concern>"),
            format!("loom finding verdict {id} rejected --reason <why the metric is a false positive> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} deferred --reason <why not scheduled now> --evidence <file:line, or journal:ref>"),
            format!("loom finding verdict {id} blocked --reason <what it waits on>"),
            format!("loom finding verdict {id} duplicate --reason <duplicate finding id or target> --evidence <the duplicate's id, or a file:line>"),
            format!("loom finding verdict {id} resolved --reason <observed repair and proof> --evidence <file:line of the fix, or journal:ref>"),
        ],
        forbidden_actions: vec![
            "edit code here (mark it needed, then fix in build/fix)".into(),
            "justified because 'length is intentional' or 'cohesive surface' without naming one concern".into(),
            "justified from owner-count alone without reading the file structure".into(),
            "batch-reaffirm / mechanical closeout of this packet".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "name the concern(s) you saw: one → justified with that name; several unrelated → needed with a split plan; false gate → rejected. A settling verdict (justified/rejected/deferred/duplicate/resolved) must also cite --evidence (file:line in the flagged file, or a journal:ref)"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom finding verdict {id} <needed|justified|rejected|deferred|blocked|duplicate|resolved> --reason '…' --evidence '<file:line for a settling verdict; omit for needed/blocked>'"),
        stop_condition: "after recording the verdict, return to loom status".into(),
        human_gate: None,
    }
}

/// Ratification: the decision is human-only; presentation and recording may be
/// mediated by an LLM. The host-facing gate is structured so the LLM can offer
/// useful choices and a recommendation, wait, then perform the typed write.
pub(super) fn ratify_contract(intent: &Node) -> PromptContract {
    let id = crate::model::short(&intent.id);
    PromptContract {
        role: "human".into(),
        mindset: "Product authority. The LLM summarizes the evidence, recommends one option with reasons, asks the human, waits, then records the human's answer. It may execute the write; it may not choose the answer."
            .into(),
        why_now: "the intent's wantedness is unestablished: minted without ratification, or redefined after it".into(),
        allowed_actions: vec![
            format!("loom intent show {id}"),
            "ask the human with the three options in human_gate; mark the evidence-backed recommendation and explain its tradeoff".into(),
            format!("loom intent ratify {id} --evidence <why this is wanted> --human-decision <exact human answer>"),
            format!("loom intent reject {id} --reason <why it is not wanted> --human-decision <exact human answer>"),
            format!("loom intent update {id} --description <corrected criterion> --reason <…>  (then re-ratify)"),
        ],
        forbidden_actions: vec![
            "supplying --human-decision before the human answers, paraphrasing silence as approval, or choosing on the human's behalf".into(),
            "using the direct ratification path from an llm:* lane (INV-8 rejects it)".into(),
            "treating silence or plausibility as ratification".into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::Prose,
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "the human's reason this behavior is wanted: an utterance, a source doc, a decision"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("after the answer: loom intent ratify {id} --evidence '…' --human-decision '<exact answer>'  (or the selected reject/update command)"),
        stop_condition: "wait for the human; after recording their selected option, return to loom status. If they defer or do not answer, write nothing".into(),
        human_gate: Some(HumanGate {
            question: format!("Should '{}' remain a wanted behavior?", intent.name),
            options: vec![
                HumanGateOption {
                    id: "ratify".into(),
                    label: "Keep behavior".into(),
                    description: "Ratify the current criterion as wanted and continue proving or maintaining it.".into(),
                    write_back: Some(format!("loom intent ratify {id} --evidence '<why wanted>' --human-decision '<exact human answer>'")),
                },
                HumanGateOption {
                    id: "reject".into(),
                    label: "Remove behavior".into(),
                    description: "Reject it as unwanted; Loom will track any live implementation as removal work.".into(),
                    write_back: Some(format!("loom intent reject {id} --reason '<why unwanted>' --human-decision '<exact human answer>'")),
                },
                HumanGateOption {
                    id: "revise".into(),
                    label: "Revise criterion".into(),
                    description: "Correct what the behavior should mean before deciding whether to keep it.".into(),
                    write_back: Some(format!("loom intent update {id} --description '<corrected criterion>' --reason '<human decision>'")),
                },
            ],
            recommendation: "The presenting LLM must recommend one option from the packet's current implementation, proof, usage, and divergence evidence; state uncertainty and never treat the recommendation as the decision.".into(),
            after_answer: "Wait for the human's selection. Record Keep/Remove with their exact answer in --human-decision; for Revise, obtain the corrected criterion before writing. No answer means no write.".into(),
        }),
    }
}

/// Prep lane: shrink the human ratify queue. Never invents wantedness.
pub(super) fn rectify_contract(intent: &Node, kind: &str) -> PromptContract {
    let id = crate::model::short(&intent.id);
    let name = q(&intent.name);
    let mut allowed_actions = vec![
        format!("loom intent show {id}"),
        format!("loom intent update {id} --visibility internal --reason '<why this is not user-facing product surface>'"),
        format!("loom intent update {id} --rectify escalated --reason '<why a human must decide wantedness>'"),
        format!("loom edge relate scenario-of {name} <parent-intent>"),
        format!("loom edge relate relates {name} <sibling-intent>"),
        format!("loom edge explore {name} <peer> ground --criterion '…' --evidence 'file:line — …' --confidence <0.0-1.0>"),
        format!("loom intent retire {id} --reason '<duplicate of better-named intent>' --replaced-by <keeper>"),
        format!("loom intent update {id} --description '<sharper falsifiable criterion>' --reword --reason '<same meaning, clearer words>'"),
    ];
    if kind == "duplicate_intent" {
        allowed_actions.push(format!(
            "loom intent update {id} --rectify clear --reason '<description discriminator proving these intents are distinct>'"
        ));
    }
    PromptContract {
        role: "rectify".into(),
        mindset: "Clear NEEDLESS ratify friction. Structural fixes only — false duplicates, \
                  mis-marked visibility, missing scenario_of/relates. If the behavior is a \
                  real user-visible product call that an LLM cannot honestly decide, escalate \
                  it to the human ratify lane. Never invent a yes or no on wantedness."
            .into(),
        why_now: format!(
            "blocking divergence '{kind}' looks like prep work, not a product decision yet"
        ),
        allowed_actions,
        forbidden_actions: vec![
            "loom intent ratify (INV-8 — wantedness is human-only)".into(),
            "loom intent reject (that is a product decision; escalate instead)".into(),
            "supplying --human-decision or treating obviousness as ratification".into(),
            "editing production code to silence a divergence".into(),
            "loom edge implement (rectify does not ground new behavior)".into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::Prose,
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "file:line (or graph structure) showing why the friction was false, \
                            or a concrete reason the human must decide"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: if kind == "duplicate_intent" {
            format!(
                "record the pair-specific discriminator with `loom intent update {id} \
                 --rectify clear --reason '…'`, or apply a structural fix (scenario-of / retire / reword)"
            )
        } else {
            format!(
                "structural fix (visibility / scenario-of / retire / reword) OR \
                 loom intent update {id} --rectify escalated --reason '…'"
            )
        },
        stop_condition: "after the write, return to loom status. If escalated, the item \
                         leaves rectify and appears on loom next --mode ratify"
            .into(),
        human_gate: None,
    }
}

pub(super) fn inbox_triage_contract(id: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Normalize raw human/external input. Route it to typed graph work or reject it with a reason; do not use inbox for code-audit findings or product questions.".into(),
        why_now: "a raw inbox item is still new".into(),
        allowed_actions: vec![
            format!("loom inbox show {id}"),
            "choose one supported landing: existing_journey | new_journey | existing_intent | hypothesis | spike | external_research; run its creation/lookup command and retain the returned stable node id".into(),
            "routing commands: loom journey add / loom intent add / loom hypothesis add / loom task add --kind spike|research".into(),
            format!("loom inbox mark {id} routed --reason '<supported-destination-kind>:<returned-stable-node-id>'"),
            format!("loom inbox mark {id} rejected --reason <why it is not actionable>"),
        ],
        forbidden_actions: vec![
            "leave the item new after using it".into(),
            "drop context without recording the disposition".into(),
        ],
        evidence_clauses: Vec::new(),
        required_evidence: "the durable destination or concrete rejection reason".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!("loom inbox mark {id} routed --reason '<existing_journey|new_journey|existing_intent|hypothesis|spike|external_research>:<returned-stable-node-id>' (or use rejected|duplicate|deferred with a concrete prose reason)"),
        stop_condition: "after disposition, return to loom status".into(),
        human_gate: None,
    }
}

/// The deepen contract. Unlike every other lane, this one does not close a gap
/// — it raises a floor that is already met, so the stop condition is a single
/// move rather than an empty queue.
/// The deepen contract.
///
/// Whether the move can raise a grade decides what this contract may demand.
/// `FreezeBaseline` is the move at S3, and S3 is the highest grade
/// `proofstrength` assigns (see the comment at src/proofstrength.rs:1653), so
/// asking for "a grade higher than the current one" as *acceptance criteria*
/// asks for something no correct move can produce. `required_evidence` and
/// `evidence_clauses` are the contract's acceptance criteria, not prose — a
/// worker satisfying them literally cannot, and anything consuming the clauses
/// programmatically inherits the impossible demand.
pub(super) fn deepen_contract(
    id: &str,
    name: &str,
    next_move: crate::risk::Move,
) -> PromptContract {
    let name = q(name);
    let raises_grade = next_move != crate::risk::Move::FreezeBaseline;
    let next_move = next_move.as_str();
    PromptContract {
        role: "validator".into(),
        mindset: "This behavior is already green. You are not fixing it — you are making \
                  the graph harder to be wrong about, starting with the thing that would \
                  hurt most if it were."
            .into(),
        why_now: "every floor is met, so the question is no longer 'what is missing' but \
                  'what is weakest'"
            .into(),
        allowed_actions: vec![
            format!("loom intent show {id}"),
            format!("loom impact {id}"),
            "loom journey add <spec>".into(),
            "loom journey freeze <journey>".into(),
            "loom validation run <name>".into(),
        ],
        forbidden_actions: vec![
            "weakening an existing assertion to make a proof pass".into(),
            "recording a stronger grade — strength is derived from the proof's shape, \
             never asserted"
                .into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesRun,
            EvidenceClause::Produces {
                what: if raises_grade {
                    "a proof grade higher than the current one".into()
                } else {
                    "a frozen baseline that replays, so a change in the shape of the \
                     output is noticed"
                        .into()
                },
            },
        ],
        required_evidence: if raises_grade {
            "a proof loom ran, whose new grade is higher than the old one".into()
        } else {
            "a proof loom ran, and a baseline frozen from it — the grade will not move, \
             because S3 is the highest grade currently assigned"
                .to_string()
        },
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "the move for this behavior is: {next_move} — close it with one proof move: \
             loom validation add --intent {name} --name '<what it proves>' --type test \
             --command '<cmd>' then loom validation run <name>; or loom journey add <spec> \
             for the behavior; or loom journey freeze <journey> to pin the baseline. \
             {closing}",
            closing = if raises_grade {
                "loom sync then re-grades the proof and re-ranks this queue"
            } else {
                "loom sync then re-reads the proof, but the grade stays at S3 and this \
                 item ranks first again — that is the ceiling, not your move failing"
            }
        ),
        stop_condition: if raises_grade {
            "stop after ONE move — this queue re-ranks after every change, and the \
             next-most-important thing is probably no longer this one"
                .into()
        } else {
            "stop after ONE move — and expect this same item back: the baseline move \
             cannot change the ranking, because the grade it would raise is already at \
             its ceiling"
                .to_string()
        },
        human_gate: None,
    }
}

/// The audit contract: loom found something in its own record that does not
/// look earned.
pub(super) fn audit_contract(remedy: &str) -> PromptContract {
    PromptContract {
        role: "analyzer".into(),
        mindset: "Something in this graph's record does not look like it was earned. \
                  Establish what actually happened before changing anything — a record \
                  that was wrong once can be wrong again in the fix."
            .into(),
        why_now: "a graph whose claim is falsifiability has to be able to fail its own check"
            .into(),
        allowed_actions: vec![
            "loom audit --json".into(),
            "read the append-only record at .loom/journal/events.jsonl".into(),
            "loom intent show <id>".into(),
        ],
        forbidden_actions: vec![
            "re-asserting the flagged claim to clear the finding".into(),
            "deleting the record instead of correcting it".into(),
        ],
        evidence_clauses: vec![EvidenceClause::Produces {
            what: "either the missing anchor, or a withdrawal of the claim".into(),
        }],
        required_evidence: "either the anchor that was missing, or a withdrawal of the claim"
            .into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        // Uniform adjudicability: every audit packet's write_back names a
        // runnable closeout. Remedies that already name one keep it; prose
        // remedies get the universal state-closure: fix, then re-read the
        // record — the finding must be absent.
        write_back: if remedy.contains("loom ") {
            format!("{remedy}; close: loom audit --json — the finding must be absent")
        } else {
            format!(
                "{remedy}; close: fix per the remedy, then loom audit --json — the finding must be absent"
            )
        },
        stop_condition: "stop when the claim is either anchored or withdrawn — never when \
                         it merely stops being reported"
            .into(),
        human_gate: None,
    }
}
