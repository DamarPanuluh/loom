//! `loom journey` command family — journey coverage and invariant points.
//!
//! A `journey_coverage` node marks a flow (entry → mutation → projection) that
//! needs a journey proof. It is linked via a `Covers` edge to the Intent whose
//! behavior the flow exercises. Coverage STATUS IS DERIVED, never asserted: a
//! coverage node reads "effectively covered" iff its covered intent currently
//! has a passing L5/L6 journey validation (proof_kind=journey). This avoids a
//! second stale truth source — when sync stales the proof, coverage reads
//! uncovered automatically (see the artifact-drift gate in `sync`).
//!
//! A `journey_invariant_point` node marks where an internal domain assertion
//! should go — a check the journey must verify that may not be visible via HTTP
//! alone. It is linked via an `Asserts` edge to the Intent it concerns. The
//! invariant's `assertion` is a design claim about the flow, not a truth claim
//! about proof; whether it is verified is derived from validations, not stored.

use super::open;
use crate::cli::{JourneyCmd, JourneyCoverageCmd, JourneyInvariantCmd};
use crate::model::{EdgeKind, InspectionStatus, NodeType};
use crate::store::Store;
use crate::Result;
use serde_json::{json, Value};

/// Dispatch entry point for the `loom journey` family.
pub fn dispatch(graph: Option<&std::path::Path>, cmd: JourneyCmd, json: bool) -> Result<()> {
    match cmd {
        JourneyCmd::Coverage { cmd } => coverage(graph, cmd, json),
        JourneyCmd::Prompt { intent } => prompt(graph, &intent, json),
        JourneyCmd::Invariant { cmd } => invariant(graph, cmd, json),
    }
}

// ---- coverage --------------------------------------------------------------

fn coverage(graph: Option<&std::path::Path>, cmd: JourneyCoverageCmd, json: bool) -> Result<()> {
    match cmd {
        JourneyCoverageCmd::Add {
            name,
            flow,
            intent,
            description,
        } => coverage_add(graph, &name, &flow, &intent, &description, json),
        JourneyCoverageCmd::List { limit } => coverage_list(graph, limit, json),
        JourneyCoverageCmd::Discover { spawn_missing } => {
            coverage_discover(graph, spawn_missing, json)
        }
    }
}

fn coverage_add(
    graph: Option<&std::path::Path>,
    name: &str,
    flow: &str,
    intent_key: &str,
    description: &str,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;
    let body = json!({ "flow": flow });
    // `status` is the asserted planning state: a coverage node starts uncovered.
    // Effective coverage is derived at read time (coverage_list), never stored.
    let node = store.add_node(
        NodeType::JourneyCoverage,
        name,
        description,
        "uncovered",
        body,
    )?;
    store.add_edge(
        EdgeKind::Covers,
        &node.id,
        &intent.id,
        crate::model::TruthClass::Asserted,
    )?;
    if json {
        let out = json!({
            "id": node.id,
            "name": node.name,
            "status": node.status,
            "flow": flow,
            "covers": intent.name,
            "covers_id": intent.id,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "added journey coverage '{}' → covers '{}' [{}]",
            node.name,
            intent.name,
            &node.id[..8]
        );
    }
    Ok(())
}

fn coverage_list(graph: Option<&std::path::Path>, limit: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let nodes = store.list_nodes(Some(NodeType::JourneyCoverage), limit)?;
    let mut rows: Vec<Value> = Vec::new();
    for n in &nodes {
        let covers = store
            .edges_with(Some(EdgeKind::Covers), Some(&n.id), None)?
            .into_iter()
            .next();
        let (covers_name, effective) = match &covers {
            None => (None, "uncovered".to_string()),
            Some(e) => {
                let intent = store.get_node(&e.to_id)?;
                let name = intent.as_ref().map(|i| i.name.clone());
                let eff = intent
                    .as_ref()
                    .map(|i| effective_coverage(&store, &i.id))
                    .unwrap_or_else(|| "uncovered".to_string());
                (name, eff)
            }
        };
        let flow = n.body.get("flow").and_then(|v| v.as_str()).unwrap_or("");
        if json {
            rows.push(json!({
                "id": n.id,
                "name": n.name,
                "flow": flow,
                "covers": covers_name,
                "status": n.status,
                "effective_status": effective,
            }));
        } else {
            println!(
                "{}  {}  flow={}  covers={}  effective={}",
                &n.id[..8],
                n.name,
                flow,
                covers_name.as_deref().unwrap_or("—"),
                effective
            );
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    }
    Ok(())
}

/// Discover coverage gaps: user-visible implemented intents with no passing L5
/// journey proof and no existing journey_coverage node. This is graph-derived
/// discovery (from visibility + lifecycle + validations), not static call-graph
/// flow analysis — loom's extraction has symbols/imports but no inter-symbol
/// call edges, so an honest entry→mutation→projection matcher is not feasible
/// here. The gap set is exactly what the journey_proof smell flags, minus
/// intents already explicitly marked with a coverage node.
fn coverage_discover(
    graph: Option<&std::path::Path>,
    spawn_missing: bool,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let snap = store.snapshot()?;
    // visibility facet per node
    let mut visibility: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for f in &snap.facets {
        if f.key == "visibility" && f.target_kind == crate::model::TargetKind::Node {
            visibility.insert(f.target_id.as_str(), f.value.as_str());
        }
    }
    // intent ids already covered by a Covers edge
    let mut already_covered: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for e in &snap.edges {
        if e.kind == EdgeKind::Covers {
            already_covered.insert(e.to_id.as_str());
        }
    }
    // validation bodies by intent (to_id of Validates)
    use std::collections::BTreeMap;
    let nodes_by_id: BTreeMap<&str, &crate::model::Node> =
        snap.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut has_l5_journey: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for e in &snap.edges {
        if e.kind != EdgeKind::Validates {
            continue;
        }
        let Some(v) = nodes_by_id.get(e.from_id.as_str()) else {
            continue;
        };
        if e.status != InspectionStatus::Passing || v.status != "passed" {
            continue;
        }
        let is_journey = v.body.get("proof_kind").and_then(|x| x.as_str()) == Some("journey");
        let is_l5 = matches!(
            v.body.get("proof_level").and_then(|x| x.as_str()),
            Some("L5") | Some("L6")
        );
        if is_journey && is_l5 {
            has_l5_journey.insert(e.to_id.as_str());
        }
    }

    let mut gaps: Vec<&crate::model::Node> = Vec::new();
    for n in &snap.nodes {
        if n.node_type != NodeType::Intent || n.status != "implemented" {
            continue;
        }
        if visibility.get(n.id.as_str()).copied() != Some("user_visible") {
            continue;
        }
        if has_l5_journey.contains(n.id.as_str()) {
            continue;
        }
        if already_covered.contains(n.id.as_str()) {
            continue;
        }
        gaps.push(n);
    }

    let mut spawned: Vec<Value> = Vec::new();
    if spawn_missing {
        for n in &gaps {
            let flow = format!("(discovered) {}", n.name);
            let body = json!({ "flow": flow });
            let node = store.add_node(
                NodeType::JourneyCoverage,
                &format!("{} flow", n.name),
                "auto-discovered coverage gap",
                "uncovered",
                body,
            )?;
            store.add_edge(
                EdgeKind::Covers,
                &node.id,
                &n.id,
                crate::model::TruthClass::Asserted,
            )?;
            spawned.push(json!({ "id": node.id, "covers": n.name }));
        }
    }

    if json {
        let out = json!({
            "gaps": gaps.iter().map(|n| n.name.clone()).collect::<Vec<_>>(),
            "gap_count": gaps.len(),
            "spawned": spawned,
            "spawned_count": spawned.len(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("{} coverage gap(s):", gaps.len());
        for n in &gaps {
            println!("  — {} [{}]", n.name, &n.id[..8]);
        }
        if spawn_missing {
            println!("spawned {} journey_coverage node(s)", spawned.len());
        }
    }
    Ok(())
}

/// Derived coverage status: "covered" iff the intent currently has a passing
/// L5/L6 journey validation (proof_kind=journey), else "uncovered". This is the
/// single truth source — it reads the same validations the journey_proof smell
/// reads, so a staled proof flips coverage to uncovered with no separate write.
fn effective_coverage(store: &Store, intent_id: &str) -> String {
    let Ok(validations) = store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id)) else {
        return "uncovered".into();
    };
    for e in &validations {
        if e.status != InspectionStatus::Passing {
            continue;
        }
        let Ok(Some(v)) = store.get_node(&e.from_id) else {
            continue;
        };
        if v.status != "passed" {
            continue;
        }
        let is_journey = v.body.get("proof_kind").and_then(|x| x.as_str()) == Some("journey");
        let is_l5_plus = matches!(
            v.body.get("proof_level").and_then(|x| x.as_str()),
            Some("L5") | Some("L6")
        );
        if is_journey && is_l5_plus {
            return "covered".into();
        }
    }
    "uncovered".into()
}

// ---- typed runner prompt context ------------------------------------------

fn prompt(graph: Option<&std::path::Path>, intent_key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;

    let implements = store.edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)?;
    let mut modules: Vec<Value> = Vec::new();
    for e in implements {
        let Some(cf) = store.get_node(&e.to_id)? else {
            continue;
        };
        let locator = store
            .get_facet(&e.id, crate::model::TargetKind::Edge, "locator")?
            .unwrap_or_default();
        modules.push(json!({
            "path": cf.name,
            "locator": locator,
            "evidence": e.evidence,
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
        "rules": [
            "Use the repo's actual domain types — no generic JSON.",
            "Call the same methods the production handlers call.",
            "Assert internal domain state, not just HTTP status codes.",
            "If a step mutates state, prove the mutation in the next step.",
            "Return a JSON success body with ok=true on success.",
            "Return a descriptive error string on failure."
        ],
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&context)?);
    } else {
        println!("{}", render_prompt(&context));
    }
    Ok(())
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

// ---- invariant points ------------------------------------------------------

fn invariant(graph: Option<&std::path::Path>, cmd: JourneyInvariantCmd, json: bool) -> Result<()> {
    match cmd {
        JourneyInvariantCmd::Add {
            name,
            intent,
            field,
            assertion,
            reason,
        } => invariant_add(graph, &name, &intent, &field, &assertion, &reason, json),
        JourneyInvariantCmd::List { limit } => invariant_list(graph, limit, json),
    }
}

fn invariant_add(
    graph: Option<&std::path::Path>,
    name: &str,
    intent_key: &str,
    field: &str,
    assertion: &str,
    reason: &str,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;
    let body = json!({
        "field": field,
        "assertion": assertion,
        "reason": reason,
    });
    let node = store.add_node(
        NodeType::JourneyInvariantPoint,
        name,
        "",
        "unverified",
        body,
    )?;
    store.add_edge(
        EdgeKind::Asserts,
        &node.id,
        &intent.id,
        crate::model::TruthClass::Asserted,
    )?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "id": node.id,
                "name": node.name,
                "field": field,
                "assertion": assertion,
                "asserts": intent.name,
                "asserts_id": intent.id,
            }))?
        );
    } else {
        println!(
            "added journey invariant '{}' → asserts '{}' [{}]",
            node.name,
            intent.name,
            &node.id[..8]
        );
    }
    Ok(())
}

fn invariant_list(graph: Option<&std::path::Path>, limit: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let nodes = store.list_nodes(Some(NodeType::JourneyInvariantPoint), limit)?;
    let mut rows: Vec<Value> = Vec::new();
    for n in &nodes {
        let asserts = store
            .edges_with(Some(EdgeKind::Asserts), Some(&n.id), None)?
            .into_iter()
            .next();
        let asserts_name = asserts
            .as_ref()
            .and_then(|e| store.get_node(&e.to_id).ok().flatten())
            .map(|i| i.name);
        if json {
            rows.push(json!({
                "id": n.id,
                "name": n.name,
                "field": n.body.get("field"),
                "assertion": n.body.get("assertion"),
                "asserts": asserts_name,
            }));
        } else {
            println!(
                "{}  {}  field={}  asserts={}",
                &n.id[..8],
                n.name,
                n.body.get("field").and_then(|v| v.as_str()).unwrap_or(""),
                asserts_name.as_deref().unwrap_or("—"),
            );
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    }
    Ok(())
}
