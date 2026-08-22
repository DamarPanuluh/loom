use super::super::{q, EvidenceClause, PromptContract};
use super::{FINDING_ADD_ACTION, NON_BLOCKING_SMELL_RULE};
use crate::model::Node;
use crate::store::Store;
use crate::Result;

pub(in super::super) fn builder_contract(intent: &Node) -> PromptContract {
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
            format!("when research is exhausted and a current external prerequisite still forbids writing code: loom intent update {name} --lifecycle blocked --reason '<the concrete external prerequisite>'"),
        ],
        forbidden_actions: vec![
            "loom rule verdict passing (quality lane)".into(),
            "loom validation verdict passed (validator lane)".into(),
            "marking the intent implemented without realizing code and a locator".into(),
            "retiring the intent because implementation is waiting on an external prerequisite — retire means the behavior is no longer wanted; use --lifecycle blocked".into(),
            NON_BLOCKING_SMELL_RULE.into(),
        ],
        evidence_clauses: vec![
            EvidenceClause::CitesSpans { n: 1 },
            EvidenceClause::VerificationAtLeast {
                level: "cited".into(),
            },
        ],
        required_evidence: "Loom context checked, relevant code inspected, then either code written and locator confirmed, or a blocked lifecycle recorded with a concrete external prerequisite — never silence and never invented wantedness".into(),
        evidence_template: None,
        examples: None,
        pre_screen: None,
        pre_screened_hits: Vec::new(),
        write_back: format!(
            "loom edge implement {name} <codefile> --locator <symbol>; loom intent update {name} --lifecycle implemented --reason '<what was built>'   (or, when a current external prerequisite forbids code: loom intent update {name} --lifecycle blocked --reason '<the prerequisite>')"
        ),
        stop_condition: "after grounding + sync, or after recording --lifecycle blocked, return to loom status".into(),
        human_gate: None,
    }
}

/// A registration pointing at a file that no longer exists on disk. There is
/// nothing to read — the honest moves are unregistering, or registering the
/// successor file and re-grounding the affected intents there.
pub(in super::super) fn missing_codefile_contract(codefile: &Node) -> PromptContract {
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

pub(in super::super) fn coverage_contract(
    store: &Store,
    codefile: &Node,
) -> Result<PromptContract> {
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
