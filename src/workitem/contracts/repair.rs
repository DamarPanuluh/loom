use super::super::{q, EvidenceClause, PromptContract};
use super::{FINDING_ADD_ACTION, NON_BLOCKING_SMELL_RULE};
use crate::model::{Edge, EdgeKind, Node};

pub(in super::super) fn fixer_contract(
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
pub(in super::super) fn needed_finding_fix_contract(
    id: &str,
    file: Option<&str>,
) -> PromptContract {
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
pub(in super::super) fn needed_finding_validate_contract(id: &str) -> PromptContract {
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
pub(in super::super) fn needed_finding_analyze_contract(id: &str) -> PromptContract {
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
