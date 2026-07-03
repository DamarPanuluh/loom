//! Journey coverage commands.

use super::{is_journey_validation, open, pulse};
use crate::cli::JourneyCoverageCmd;
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use serde_json::{json, Value};

// ---- coverage --------------------------------------------------------------

pub(super) fn coverage(
    graph: Option<&std::path::Path>,
    cmd: JourneyCoverageCmd,
    json: bool,
) -> Result<()> {
    match cmd {
        JourneyCoverageCmd::Add {
            name,
            flow,
            intent,
            description,
            runner_ref,
            test_ref,
            contract_artifact,
        } => coverage_add(
            graph,
            CoverageAddArgs {
                name: &name,
                flow: &flow,
                intent_key: &intent,
                description: &description,
                runner_ref: runner_ref.as_deref(),
                test_ref: test_ref.as_deref(),
                contract_artifact: contract_artifact.as_deref(),
            },
            json,
        ),
        JourneyCoverageCmd::Update {
            key,
            runner_ref,
            test_ref,
            contract_artifact,
            reason,
        } => coverage_update(
            graph,
            &key,
            runner_ref.as_deref(),
            test_ref.as_deref(),
            contract_artifact.as_deref(),
            &reason,
            json,
        ),
        JourneyCoverageCmd::Remove { key } => coverage_remove(graph, &key, json),
        JourneyCoverageCmd::List { limit } => coverage_list(graph, limit, json),
        JourneyCoverageCmd::Discover { spawn_missing } => {
            coverage_discover(graph, spawn_missing, json)
        }
        JourneyCoverageCmd::Drift => coverage_drift(graph, json),
    }
}

struct CoverageAddArgs<'a> {
    name: &'a str,
    flow: &'a str,
    intent_key: &'a str,
    description: &'a str,
    runner_ref: Option<&'a str>,
    test_ref: Option<&'a str>,
    contract_artifact: Option<&'a str>,
}

fn coverage_add(
    graph: Option<&std::path::Path>,
    args: CoverageAddArgs<'_>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let intent = store.resolve_node(args.intent_key, Some(NodeType::Intent))?;
    let mut body = json!({ "flow": args.flow });
    if let Some(v) = args.runner_ref {
        body["runner_ref"] = json!(v);
    }
    if let Some(v) = args.test_ref {
        body["test_ref"] = json!(v);
    }
    if let Some(v) = args.contract_artifact {
        body["contract_artifact"] = json!(v);
    }
    // `status` is the asserted planning state: a coverage node starts uncovered.
    // Effective coverage is derived at read time (coverage_list), never stored.
    let node = store.add_node(
        NodeType::JourneyCoverage,
        args.name,
        args.description,
        "uncovered",
        body,
    )?;
    store.add_edge(
        EdgeKind::Covers,
        &node.id,
        &intent.id,
        crate::model::TruthClass::Asserted,
    )?;
    let payload = json!({
        "id": node.id,
        "name": node.name,
        "status": node.status,
        "flow": args.flow,
        "covers": intent.name,
        "covers_id": intent.id,
    });
    let line = format!(
        "added journey coverage '{}' → covers '{}' [{}]",
        node.name,
        intent.name,
        &node.id[..8]
    );
    pulse::emit_line(
        &store,
        json,
        payload,
        "run `loom journey prompt <intent>` when you are ready to author the proof",
        line,
    )
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
fn coverage_update(
    graph: Option<&std::path::Path>,
    key: &str,
    runner_ref: Option<&str>,
    test_ref: Option<&str>,
    contract_artifact: Option<&str>,
    reason: &str,
    json: bool,
) -> Result<()> {
    if reason.trim().is_empty() {
        bail!("journey coverage update needs substantive --reason");
    }
    if runner_ref.is_none() && test_ref.is_none() && contract_artifact.is_none() {
        bail!("nothing to update — pass --runner-ref, --test-ref, and/or --contract-artifact");
    }
    let store = open(graph)?;
    let node = store.resolve_node(key, Some(NodeType::JourneyCoverage))?;
    let mut body = node.body.clone();
    if let Some(v) = runner_ref {
        body["runner_ref"] = json!(v);
    }
    if let Some(v) = test_ref {
        body["test_ref"] = json!(v);
    }
    if let Some(v) = contract_artifact {
        body["contract_artifact"] = json!(v);
    }
    store.set_node_body(&node.id, &body)?;
    store.add_note(
        &node.id,
        "decision",
        &format!("updated journey coverage declaration: {reason}"),
    )?;
    pulse::emit_line(
        &store,
        json,
        json!({
            "coverage": {
                "id": node.id,
                "name": node.name,
                "status": node.status,
                "body": body,
            },
            "reason": reason,
        }),
        "loom journey coverage list",
        format!("updated journey coverage '{}'", node.name),
    )
}

fn coverage_remove(graph: Option<&std::path::Path>, key: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let node = store.resolve_node(key, Some(NodeType::JourneyCoverage))?;
    store.delete_node(&node.id)?;
    pulse::emit_line(
        &store,
        json,
        json!({
            "removed": true,
            "coverage": {
                "id": node.id,
                "name": node.name,
                "status": node.status,
                "body": node.body,
            },
        }),
        "loom journey coverage list",
        format!("removed journey coverage '{}'", node.name),
    )
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
        let is_journey = is_journey_validation(v);
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

    let payload = json!({
        "gaps": gaps.iter().map(|n| n.name.clone()).collect::<Vec<_>>(),
        "gap_count": gaps.len(),
        "spawned": spawned,
        "spawned_count": spawned.len(),
    });
    if spawn_missing {
        let line = format!(
            "spawned {} journey_coverage node(s)",
            payload["spawned_count"]
        );
        pulse::emit_line(
            &store,
            json,
            payload,
            "run `loom journey coverage list` to review spawned coverage nodes",
            line,
        )
    } else {
        if json {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            println!("{} coverage gap(s):", gaps.len());
            for n in &gaps {
                println!("  — {} [{}]", n.name, &n.id[..8]);
            }
        }
        Ok(())
    }
}

fn coverage_drift(graph: Option<&std::path::Path>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let coverages = store.list_nodes(Some(NodeType::JourneyCoverage), usize::MAX)?;
    let mut findings: Vec<Value> = Vec::new();
    for cov in &coverages {
        let Some(intent) = coverage_intent(&store, &cov.id)? else {
            findings.push(json!({
                "kind": "journey_coverage_unlinked",
                "coverage": cov.name,
                "message": "journey_coverage node has no Covers edge to an intent",
                "remedy": "link the coverage node to the intent it covers",
            }));
            continue;
        };
        for (field, kind) in [
            ("runner_ref", "journey_runner_ref_missing"),
            ("test_ref", "journey_test_ref_missing"),
        ] {
            let Some(reference) = cov.body.get(field).and_then(|v| v.as_str()) else {
                continue;
            };
            if !repo_ref_exists(store.root(), reference) {
                findings.push(json!({
                    "kind": kind,
                    "coverage": cov.name,
                    "intent": intent.name,
                    "reference": reference,
                    "message": format!("configured {field} does not resolve on disk"),
                    "remedy": "update the coverage node reference or restore the runner/test code",
                }));
            }
        }
        let proofs = current_l5_journey_validations(&store, &intent.id)?;
        if proofs.is_empty() {
            // Not drift: uncovered coverage is a gap, reported by discover/smells.
            continue;
        }

        let coverage_artifact = cov.body.get("contract_artifact").and_then(|v| v.as_str());
        let proof = match coverage_artifact {
            Some(expected) => proofs
                .iter()
                .find(|p| p.body.get("artifact").and_then(|v| v.as_str()) == Some(expected))
                .unwrap_or(&proofs[0]),
            None => &proofs[0],
        };
        let proof_artifact = proof.body.get("artifact").and_then(|v| v.as_str());
        let expected_artifact = coverage_artifact.or(proof_artifact);
        match expected_artifact {
            Some(path) => {
                if coverage_artifact.is_some() && proof_artifact != coverage_artifact {
                    let actual_artifacts: Vec<&str> = proofs
                        .iter()
                        .filter_map(|p| p.body.get("artifact").and_then(|v| v.as_str()))
                        .collect();
                    findings.push(json!({
                        "kind": "journey_contract_artifact_mismatch",
                        "coverage": cov.name,
                        "intent": intent.name,
                        "expected_artifact": coverage_artifact,
                        "actual_artifacts": actual_artifacts,
                        "message": "coverage contract_artifact does not match any current passing L5 journey proof artifact",
                        "remedy": "update the coverage node, validation artifact, or rerun the correct journey proof",
                    }));
                }
                if !store.root().join(path).exists() {
                    findings.push(json!({
                        "kind": "journey_contract_artifact_missing",
                        "coverage": cov.name,
                        "intent": intent.name,
                        "proof": proof.name,
                        "artifact": path,
                        "message": "current passing journey proof artifact is missing on disk",
                        "remedy": "restore the artifact or rerun sync so the proof stales and coverage becomes uncovered",
                    }));
                }
            }
            None => findings.push(json!({
                "kind": "journey_proof_missing_artifact",
                "coverage": cov.name,
                "intent": intent.name,
                "proof": proof.name,
                "message": "current passing L5 journey proof has no body.artifact, so contract drift cannot be tracked",
                "remedy": "add/update the validation with --artifact pointing at the contract/journey/runner artifact",
            })),
        }
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else if findings.is_empty() {
        println!("journey coverage drift: clean");
    } else {
        for f in &findings {
            println!(
                "{}: {}",
                f["kind"].as_str().unwrap_or("journey_drift"),
                f["message"].as_str().unwrap_or("")
            );
        }
    }
    if findings.is_empty() {
        Ok(())
    } else {
        bail!("journey coverage drift found {} issue(s)", findings.len())
    }
}

fn coverage_intent(store: &Store, coverage_id: &str) -> Result<Option<Node>> {
    let edge = store
        .edges_with(Some(EdgeKind::Covers), Some(coverage_id), None)?
        .into_iter()
        .next();
    match edge {
        Some(e) => store.get_node(&e.to_id),
        None => Ok(None),
    }
}

pub(super) fn current_l5_journey_validations(store: &Store, intent_id: &str) -> Result<Vec<Node>> {
    let mut out = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id))? {
        if e.status != InspectionStatus::Passing {
            continue;
        }
        let Some(v) = store.get_node(&e.from_id)? else {
            continue;
        };
        if v.status != "passed" {
            continue;
        }
        let is_journey = is_journey_validation(&v);
        let is_l5_plus = matches!(
            v.body.get("proof_level").and_then(|x| x.as_str()),
            Some("L5") | Some("L6")
        );
        if is_journey && is_l5_plus {
            out.push(v);
        }
    }
    Ok(out)
}

fn repo_ref_exists(root: &std::path::Path, reference: &str) -> bool {
    if let Some((path, symbol)) = reference.split_once("::") {
        let p = root.join(path);
        return std::fs::read_to_string(p)
            .map(|content| content.contains(symbol))
            .unwrap_or(false);
    }
    let p = root.join(reference);
    if p.exists() {
        return true;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if matches!(name.to_str(), Some(".git" | ".loom" | "target")) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if std::fs::read_to_string(&path)
                .map(|content| content.contains(reference))
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn effective_coverage(store: &Store, intent_id: &str) -> String {
    match current_l5_journey_validations(store, intent_id) {
        Ok(proofs) if !proofs.is_empty() => "covered".into(),
        _ => "uncovered".into(),
    }
}

pub(super) fn coverage_context(store: &Store, intent_id: &str) -> Result<Value> {
    let effective = effective_coverage(store, intent_id);
    let mut nodes: Vec<(String, Value)> = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Covers), None, Some(intent_id))? {
        let Some(cov) = store.get_node(&e.from_id)? else {
            continue;
        };
        if cov.node_type != NodeType::JourneyCoverage {
            continue;
        }
        let sort_key = cov.name.clone();
        nodes.push((
            sort_key,
            json!({
                "id": cov.id,
                "name": cov.name,
                "flow": cov.body.get("flow").and_then(|v| v.as_str()).unwrap_or(""),
                "status": cov.status,
                "effective_status": effective.clone(),
            }),
        ));
    }
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    let status = if effective == "covered" {
        "covered"
    } else if nodes.is_empty() {
        "none"
    } else {
        "planned_unproven"
    };
    Ok(json!({
        "status": status,
        "nodes": nodes.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
    }))
}
