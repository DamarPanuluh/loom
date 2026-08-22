use super::super::{q, EvidenceClause, PromptContract};
use crate::model::Node;

/// The elaboration contract: grow the surroundings the human forgot, decide
/// nothing that belongs to the human.
pub(in super::super) fn elaborator_contract(
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
pub(in super::super) fn derive_contract(
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
        human_gate: Some(super::super::derivation_human_gate(journey)),
    }
}

/// Compile one fully derived and grounded Journey into a real target-repo CLI
/// surface. Loom supplies the structured contract; the builder writes code in
/// the repository's own language and idiom.
pub(in super::super) fn surface_contract(
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
