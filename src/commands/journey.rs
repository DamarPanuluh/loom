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
