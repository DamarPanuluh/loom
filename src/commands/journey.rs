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
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use serde_json::{json, Value};

/// Dispatch entry point for the `loom journey` family.
pub fn dispatch(graph: Option<&std::path::Path>, cmd: JourneyCmd, json: bool) -> Result<()> {
    match cmd {
        JourneyCmd::Coverage { cmd } => coverage(graph, cmd, json),
        JourneyCmd::Prompt { intent } => prompt(graph, &intent, json),
        JourneyCmd::Invariant { cmd } => invariant(graph, cmd, json),
        JourneyCmd::Run { spec, base_url } => journey_run(&spec, base_url.as_deref(), json),
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
    if json {
        let out = json!({
            "id": node.id,
            "name": node.name,
            "status": node.status,
            "flow": args.flow,
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
                "remedy": "add/update the validation with --artifact pointing at the contract/saga/runner artifact",
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

fn current_l5_journey_validations(store: &Store, intent_id: &str) -> Result<Vec<Node>> {
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
        let is_journey = v.body.get("proof_kind").and_then(|x| x.as_str()) == Some("journey");
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
/// imports were seen so the LLM can decide in-process-runner vs. saga/contract.
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
/// hints dominate, steer toward a saga/contract proof instead of asserting a
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
    // contradict the saga steer below.
    let in_process = grounded && !has_service_call;
    if in_process {
        rules.push("Use the repo's actual domain types — no generic JSON.".into());
        rules.push("Call the same methods the production handlers call.".into());
        rules.push("Assert internal domain state, not just HTTP status codes.".into());
    } else if !grounded {
        rules.push(
            "This intent has no in-process code grounding — prefer a consumer-facing HTTP/saga proof over an in-process typed runner.".into(),
        );
    }
    rules.push("If a step mutates state, prove the mutation in the next step.".into());
    if has_infra {
        rules.push(
            "This flow's code imports infrastructure (see signals.infra_hints); if the runner would need a live dependency it cannot stand up, generate a saga/contract proof and flag the typed runner as \"needs infrastructure\".".into(),
        );
    }
    if has_service_call {
        rules.push(
            "This flow crosses a service boundary (outbound HTTP/gRPC/queue import) — prove it with a cross-service saga spec, not an in-process runner.".into(),
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

// ---- journey run (graph-free HTTP contract executor) -------------------------

/// Execute a contract spec (JSON or YAML) directly. No graph registration,
/// no intent resolution — consumer-facing proof that sends requests, checks
/// status + fields, threads captures, and reports green/red.
fn journey_run(spec: &std::path::Path, base_url: Option<&str>, json: bool) -> Result<()> {
    let mut parsed = crate::saga::parse(spec)?;
    if let Some(b) = base_url {
        parsed.base = b.to_string();
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let mut vars: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut outcomes: Vec<Value> = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;

    for step in &parsed.steps {
        let base = interpolate_vars(&parsed.base, &vars);
        if base.is_empty() || base.contains("{{") {
            bail!(
                "journey '{}' has no usable base URL (spec base='{}' resolved to '{base}'). \
                 Pass --base-url, set BASE_URL in the environment, or add a \"base\" field to the spec.",
                parsed.saga,
                parsed.base
            );
        }
        let url = interpolate_vars(&format!("{base}{}", step.request.url), &vars);
        let method = step.request.method.to_uppercase();
        let mut req = client.request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET),
            &url,
        );
        for (name, value) in &step.request.headers {
            req = req.header(name, interpolate_vars(value, &vars));
        }
        if !step.request.query.is_empty() {
            let query: Vec<(String, String)> = step
                .request
                .query
                .iter()
                .map(|(k, v)| (k.clone(), interpolate_vars(&value_to_string(v), &vars)))
                .collect();
            req = req.query(&query);
        }
        if let Some(body) = &step.request.json {
            let interpolated = interpolate_json_vars(body, &vars);
            req = req.json(&interpolated);
        }
        let (step_passed, detail) = match req.send() {
            Ok(resp) => {
                let status_code = resp.status().as_u16();
                let expected = step.expect.status.unwrap_or(200);
                if status_code != expected {
                    (
                        false,
                        format!("expected status {expected}, got {status_code}"),
                    )
                } else {
                    let body: serde_json::Value = resp.json().unwrap_or(Value::Null);
                    let mut ok = true;
                    let mut err_detail = String::new();
                    for path in &step.expect.exists {
                        if jsonpath_val(&body, path).is_none() {
                            ok = false;
                            err_detail = format!("missing field {path}");
                            break;
                        }
                    }
                    if ok {
                        for (path, want) in &step.expect.body {
                            let want_resolved = interpolate_json_vars(want, &vars);
                            let got = jsonpath_val(&body, path);
                            if got.as_ref() != Some(&want_resolved) {
                                ok = false;
                                err_detail = format!(
                                    "body {path}: expected {want_resolved}, got {}",
                                    got.map(|v| v.to_string()).unwrap_or_else(|| "null".into())
                                );
                                break;
                            }
                        }
                    }
                    if ok {
                        // Thread captures
                        for (var, path) in &step.capture {
                            if let Some(v) = jsonpath_val(&body, path) {
                                let s = match v {
                                    Value::String(s) => s,
                                    other => other.to_string(),
                                };
                                vars.insert(var.clone(), s);
                            }
                        }
                        let success_detail =
                            if step.expect.exists.is_empty() && step.expect.body.is_empty() {
                                format!("status {status_code} ok")
                            } else {
                                let mut checked: Vec<&str> =
                                    step.expect.body.keys().map(String::as_str).collect();
                                checked.extend(step.expect.exists.iter().map(String::as_str));
                                format!("status {status_code} ok, verified: {}", checked.join(", "))
                            };
                        (true, success_detail)
                    } else {
                        (false, err_detail)
                    }
                }
            }
            Err(e) => (false, format!("request failed: {e}")),
        };
        if step_passed {
            passed += 1;
        } else {
            failed += 1;
        }
        outcomes.push(json!({
            "step": step.name,
            "passed": step_passed,
            "detail": detail,
        }));
        if !step_passed {
            break; // stop at boundary
        }
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "journey": parsed.saga,
                "passed": passed,
                "failed": failed,
                "total": passed + failed,
                "outcomes": outcomes,
            }))?
        );
    } else {
        for o in &outcomes {
            println!(
                "{} {} — {}",
                if o["passed"].as_bool().unwrap_or(false) {
                    "PASS"
                } else {
                    "FAIL"
                },
                o["step"].as_str().unwrap_or(""),
                o["detail"].as_str().unwrap_or("")
            );
        }
        println!(
            "journey '{}': {}/{} step(s) passed",
            parsed.saga,
            passed,
            passed + failed
        );
    }

    if failed > 0 {
        bail!(
            "journey '{}' failed ({} step(s) failed)",
            parsed.saga,
            failed
        )
    } else {
        Ok(())
    }
}

/// Interpolate `{{ var }}` and `{{ env.NAME }}` in a string (graph-free version).
fn interpolate_vars(s: &str, vars: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let key = after[..end].trim();
            if let Some(stripped) = key.strip_prefix("env.") {
                out.push_str(&std::env::var(stripped).unwrap_or_default());
            } else if let Some(val) = vars.get(key) {
                out.push_str(val);
            }
            rest = &after[end + 2..];
        } else {
            out.push_str("{{");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn interpolate_json_vars(v: &Value, vars: &std::collections::BTreeMap<String, String>) -> Value {
    match v {
        Value::String(s) => Value::String(interpolate_vars(s, vars)),
        Value::Array(a) => Value::Array(a.iter().map(|x| interpolate_json_vars(x, vars)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), interpolate_json_vars(x, vars)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn jsonpath_val(v: &Value, path: &str) -> Option<Value> {
    let p = path
        .strip_prefix("$.")
        .or_else(|| path.strip_prefix('$'))
        .unwrap_or(path);
    let mut cur = v.clone();
    for seg in p.split('.') {
        cur = cur.get(seg)?.clone();
    }
    Some(cur)
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
