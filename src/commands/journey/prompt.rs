//! Typed runner prompt context command.
//!
//! Plane: CLI surface, read-only — emits extracted structural facts for the
//! LLM to classify; loom asserts no architecture and writes nothing here.

use super::{coverage::effective_coverage, open};
use crate::model::{EdgeKind, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use serde_json::{json, Value};

// ---- typed runner prompt context ------------------------------------------

pub(super) fn prompt(graph: Option<&std::path::Path>, intent_key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;

    let implements = store.realizing_groundings(&intent.id)?;
    let mut modules: Vec<Value> = Vec::new();
    // Raw classification signals — imports + language across the intent's
    // grounded files. These are FACTS loom already extracts, never a verdict:
    // the LLM classifies the repo from them, loom does not assert an architecture.
    let mut all_imports: Vec<String> = Vec::new();
    let mut languages: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in implements {
        let Some(cf) = store.get_node(&e.to_id)? else {
            continue;
        };
        let locator = store
            .get_facet(&e.id, crate::model::TargetKind::Edge, "locator")?
            .unwrap_or_default();
        if let Some(lang) = store.get_facet(&cf.id, TargetKind::Node, "language")? {
            languages.insert(lang);
        }
        if let Some(raw) = store.get_facet(&cf.id, TargetKind::Node, "imports")? {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
                all_imports.extend(list);
            }
        }
        modules.push(json!({
            "path": cf.name,
            "locator": locator,
            "evidence": store.verdict_prose(&e.id)?,
            "status": e.status.as_str(),
        }));
    }

    let coverages = store.edges_with(Some(EdgeKind::Covers), None, Some(&intent.id))?;
    let mut flows: Vec<Value> = Vec::new();
    for e in coverages {
        let Some(cov) = store.get_node(&e.from_id)? else {
            continue;
        };
        flows.push(json!({
            "name": cov.name,
            "flow": cov.body.get("flow").and_then(|v| v.as_str()).unwrap_or(""),
            "effective_status": effective_coverage(&store, &intent.id),
        }));
    }

    let invariants = store.edges_with(Some(EdgeKind::Asserts), None, Some(&intent.id))?;
    let mut invariant_points: Vec<Value> = Vec::new();
    for e in invariants {
        let Some(inv) = store.get_node(&e.from_id)? else {
            continue;
        };
        invariant_points.push(json!({
            "name": inv.name,
            "field": inv.body.get("field").and_then(|v| v.as_str()).unwrap_or(""),
            "assertion": inv.body.get("assertion").and_then(|v| v.as_str()).unwrap_or(""),
            "reason": inv.body.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
        }));
    }

    let signals =
        classification_signals(&store, &intent.id, &all_imports, &languages, modules.len())?;
    let rules = prompt_rules(&signals);
    let context = json!({
        "intent": {
            "id": intent.id,
            "name": intent.name,
            "description": intent.description,
            "lifecycle": intent.status,
        },
        "flows": flows,
        "modules": modules,
        "invariant_points": invariant_points,
        "signals": signals,
        "rules": rules,
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&context)?);
    } else {
        println!("{}", render_prompt(&context));
    }
    Ok(())
}

/// Infra import fragments → the capability they usually imply. Matched as
/// case-insensitive substrings against the intent's grounded files' imports.
/// These are HINTS, not a verdict: a match means "this import usually needs a
/// running X", which the LLM weighs — loom never asserts the architecture.
const INFRA_HINTS: &[(&str, &str)] = &[
    ("sqlx", "database"),
    ("diesel", "database"),
    ("sea_orm", "database"),
    ("tokio_postgres", "database"),
    ("postgres", "database"),
    ("mysql", "database"),
    ("rusqlite", "database"),
    ("mongodb", "database"),
    ("redis", "cache_or_store"),
    ("reqwest", "outbound_http"),
    ("hyper", "outbound_http"),
    ("tonic", "grpc_service_call"),
    ("kafka", "message_queue"),
    ("rdkafka", "message_queue"),
    ("lapin", "message_queue"),
    ("aws_sdk", "cloud_sdk"),
];

/// Assemble raw, evidence-tagged classification signals for the LLM. Every
/// field is a fact loom already has (imports, language, declared layer), never
/// an asserted architecture label. `infra_hints` reports which infra-touching
/// imports were seen so the LLM can decide in-process-runner vs. journey/contract.
fn classification_signals(
    store: &Store,
    intent_id: &str,
    imports: &[String],
    languages: &std::collections::BTreeSet<String>,
    module_count: usize,
) -> Result<Value> {
    let mut infra: std::collections::BTreeMap<&str, Vec<String>> =
        std::collections::BTreeMap::new();
    for imp in imports {
        let lower = imp.to_ascii_lowercase();
        for (fragment, capability) in INFRA_HINTS {
            if lower.contains(fragment) {
                infra.entry(*capability).or_default().push(imp.clone());
            }
        }
    }
    let infra_hints: Vec<Value> = infra
        .into_iter()
        .map(|(capability, matched)| json!({ "capability": capability, "imports": matched }))
        .collect();

    // Declared architecture layer for this intent (only if an author set it).
    let layer = store.get_facet(intent_id, TargetKind::Node, "layer")?;

    Ok(json!({
        "note": "Raw signals loom extracted — NOT an architecture verdict. Classify the repo yourself and pick the runner shape; loom cannot see call graphs or type dependencies.",
        "languages": languages.iter().cloned().collect::<Vec<_>>(),
        "declared_layer": layer,
        "grounded": module_count > 0,
        "infra_hints": infra_hints,
    }))
}

/// Choose the runner rules based on the signals. The in-process-runner rules
/// only apply when the intent is actually grounded in domain code; when infra
/// hints dominate, steer toward a journey/contract proof instead of asserting a
/// typed runner that may need infrastructure the runner can't stand up.
fn prompt_rules(signals: &Value) -> Vec<String> {
    let grounded = signals["grounded"].as_bool().unwrap_or(false);
    let has_infra = signals["infra_hints"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    let has_service_call = signals["infra_hints"]
        .as_array()
        .map(|a| {
            a.iter().any(|h| {
                matches!(
                    h["capability"].as_str(),
                    Some("grpc_service_call") | Some("outbound_http") | Some("message_queue")
                )
            })
        })
        .unwrap_or(false);

    let mut rules = Vec::new();
    // In-process typed-runner rules apply only when the flow is grounded in
    // domain code AND does not cross a service boundary. A cross-service flow
    // can't be proven by calling in-process methods, so those rules would
    // contradict the journey steer below.
    let in_process = grounded && !has_service_call;
    if in_process {
        rules.push("Use the repo's actual domain types — no generic JSON.".into());
        rules.push("Call the same methods the production handlers call.".into());
        rules.push("Assert internal domain state, not just HTTP status codes.".into());
    } else if !grounded {
        rules.push(
            "This intent has no in-process code grounding — prefer a consumer-facing HTTP/journey proof over an in-process typed runner.".into(),
        );
    }
    rules.push("If a step mutates state, prove the mutation in the next step.".into());
    if has_infra {
        rules.push(
            "This flow's code imports infrastructure (see signals.infra_hints); if the runner would need a live dependency it cannot stand up, generate a journey/contract proof and flag the typed runner as \"needs infrastructure\".".into(),
        );
    }
    if has_service_call {
        rules.push(
            "This flow crosses a service boundary (outbound HTTP/gRPC/queue import) — prove it with a cross-service journey spec, not an in-process runner.".into(),
        );
    }
    rules.push("Return a JSON success body with ok=true on success.".into());
    rules.push("Return a descriptive error string on failure.".into());
    rules.push(
        "Include a drift test that verifies the consumer artifact is in sync with the implementation.".into(),
    );
    rules
}

fn render_prompt(context: &Value) -> String {
    let intent = &context["intent"];
    let mut out = String::new();
    out.push_str("You are generating a typed journey runner for this repo.\n\n");
    out.push_str(&format!(
        "Flow to cover: {}\n",
        intent["name"].as_str().unwrap_or("")
    ));
    if let Some(desc) = intent["description"].as_str() {
        if !desc.is_empty() {
            out.push_str(&format!("Intent description: {desc}\n"));
        }
    }
    out.push_str("\nModules involved:\n");
    for m in context["modules"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "- {} {}\n",
            m["path"].as_str().unwrap_or(""),
            m["locator"].as_str().unwrap_or("")
        ));
    }
    out.push_str("\nDiscovered flows / coverage markers:\n");
    for f in context["flows"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "- {} ({})\n",
            f["flow"].as_str().unwrap_or(""),
            f["effective_status"].as_str().unwrap_or("uncovered")
        ));
    }
    out.push_str("\nInvariant points:\n");
    for inv in context["invariant_points"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "- {}: {} ({})\n",
            inv["field"].as_str().unwrap_or(""),
            inv["assertion"].as_str().unwrap_or(""),
            inv["reason"].as_str().unwrap_or("")
        ));
    }
    out.push_str("\nRules:\n");
    for rule in context["rules"].as_array().into_iter().flatten() {
        out.push_str(&format!("- {}\n", rule.as_str().unwrap_or("")));
    }
    out.push_str(
        "\nOutput: a single file in the repo's primary language implementing the runner.\n",
    );
    out
}
