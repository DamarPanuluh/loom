use super::super::queues::prescreen_for;
use super::super::{q, PromptContract};
use super::{FINDING_ADD_ACTION, NON_BLOCKING_SMELL_RULE};
use crate::model::{Edge, Node};
use crate::store::Store;
use crate::Result;

pub(in super::super) fn quality_contract(
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
pub(in super::super) fn quality_contract_body(
    rule: Option<&Node>,
    why_now: &str,
    rule_name: &str,
    intent_name: &str,
    screen: super::super::queues::PreScreen,
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
