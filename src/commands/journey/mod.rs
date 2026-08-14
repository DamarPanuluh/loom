//! Semantic Journey command handlers.
//!
//! `add` registers only the authored root artifact. `derive` and `surface`
//! emit read-only JSON packets. Their corresponding `*-accept` commands apply
//! strict, hash-bound manifests atomically.

use super::{open, open_read, pulse};
use crate::cli::JourneyCmd;
use crate::model::{EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub fn dispatch(graph: Option<&Path>, cmd: JourneyCmd, json: bool) -> Result<()> {
    match cmd {
        JourneyCmd::Lint { journey } => journey_lint(graph, journey.as_deref(), json),
        JourneyCmd::Add { spec } => journey_add(graph, spec, json),
        JourneyCmd::Show { journey } => journey_show(graph, &journey, json),
        JourneyCmd::Remove { journey } => journey_remove(graph, &journey, json),
        JourneyCmd::List { limit, offset } => journey_list(graph, limit, offset, json),
        JourneyCmd::Map => journey_map(graph, json),
        JourneyCmd::Derive {
            journey,
            candidate_json,
        } => journey_derive(graph, &journey, candidate_json.as_deref(), json),
        JourneyCmd::DeriveAccept {
            journey,
            manifest,
            human_decision,
        } => journey_derive_accept(graph, &journey, &manifest, human_decision, json),
        JourneyCmd::Surface { journey } => journey_surface(graph, &journey, json),
        JourneyCmd::SurfaceAccept { journey, manifest } => {
            journey_surface_accept(graph, &journey, &manifest, json)
        }
        JourneyCmd::Compile { journey, profile } => {
            journey_compile(graph, &journey, &profile, json)
        }
        JourneyCmd::Run { journey, profile } => journey_run(graph, &journey, &profile, json),
        JourneyCmd::Resume {
            token,
            choice,
            human_decision,
            free_form,
        } => journey_resume(
            graph,
            &token,
            &choice,
            &human_decision,
            free_form.as_deref(),
            json,
        ),
        JourneyCmd::Diagnose {
            journey,
            profile,
            input,
        } => journey_diagnose(graph, &journey, &profile, &input, json),
        JourneyCmd::RehearseCold { journey } => journey_rehearse_cold(graph, &journey, json),
        JourneyCmd::Freeze { journey, profile } => journey_freeze(graph, &journey, &profile, json),
        JourneyCmd::Drift { journey } => journey_drift(graph, journey.as_deref(), json),
    }
}

fn journey_rehearse_cold(graph: Option<&Path>, journey_key: &str, json_output: bool) -> Result<()> {
    if !json_output {
        bail!("`loom journey rehearse-cold` requires --json");
    }
    let store = open_read(graph)?;
    let (journey, _, _) = load_registered_journey(&store, journey_key)?;
    let report = crate::release::rehearse_cold_journey(store.root(), &journey.name)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn journey_lint(graph: Option<&Path>, journey_key: Option<&str>, json_output: bool) -> Result<()> {
    let store = open_read(graph)?;
    let journeys = if let Some(key) = journey_key {
        vec![resolve_journey(&store, key)?]
    } else {
        let mut nodes = store.list_nodes(Some(NodeType::Journey), usize::MAX)?;
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        nodes
    };
    let mut findings = Vec::new();
    let mut scanned = 0;
    for journey in journeys {
        let (_, spec, hash) = load_registered_journey(&store, &journey.id)?;
        let artifact = journey
            .body
            .get("artifact")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Journey '{}' has no artifact", journey.name))?;
        let surface = Path::new(artifact)
            .parent()
            .unwrap_or(Path::new(""))
            .join("surfaces")
            .join(format!("{}.surface.json", journey.name));
        let absolute = store.root().join(&surface);
        if !absolute.is_file() {
            bail!(
                "Journey '{}' has no surface manifest at '{}'",
                journey.name,
                surface.display()
            );
        }
        let manifest = crate::journey::SurfaceManifest::parse_json(&absolute)?;
        manifest.validate_for(&spec, &hash)?;
        manifest.validate_setup_for_store(&store)?;
        let report = manifest.lint(&store, &spec, &surface.to_string_lossy())?;
        scanned += report.scanned;
        findings.extend(report.findings);
    }
    let report = crate::journey::JourneyLintReport::new(scanned, findings);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for finding in &report.findings {
            let location = finding
                .operation
                .as_deref()
                .map(|op| format!(" operation={op}"))
                .unwrap_or_default();
            println!(
                "{:?} {} {}{}: {}",
                finding.severity, finding.rule, finding.journey_id, location, finding.message
            );
        }
        println!(
            "{}: scanned={}, blocking={}, advisory={}",
            report.status, report.scanned, report.blocking, report.advisory
        );
    }
    if report.blocking > 0 {
        bail!("Journey lint found {} blocking finding(s)", report.blocking);
    }
    Ok(())
}

fn journey_resume(
    graph: Option<&Path>,
    token: &str,
    choice: &str,
    human_decision: &str,
    free_form: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let pending = crate::journey_runtime::pending_continuation(token)?;
    let store = open(graph)?;
    if store.root() != pending.live_root {
        bail!(
            "Journey gate resume token belongs to a different graph root ('{}')",
            pending.live_root.display()
        );
    }
    let executor = store.execution_identity().actor();
    match crate::journey::resume_and_settle_compiled_validation(
        &store,
        token,
        crate::journey_gate::ResumeAnswer {
            choice_id: choice.to_string(),
            human_decision: human_decision.to_string(),
            free_form: free_form.map(str::to_string),
        },
        &executor,
    )? {
        crate::journey::InteractiveJourneyRun::Completed(report) => {
            emit_report(&report, json_output)
        }
        crate::journey::InteractiveJourneyRun::Pending(pending) => emit_runtime_value(
            serde_json::to_value(&pending)?,
            json_output,
            &format!(
                "Journey '{}:{}' is still waiting for a human decision",
                pending.binding.journey_id, pending.binding.profile
            ),
        ),
    }
}

pub(crate) fn journey_add(graph: Option<&Path>, spec: PathBuf, json: bool) -> Result<()> {
    let store = open(graph)?;
    // Confinement is the read boundary: reject an out-of-root path before
    // opening or parsing it, even when its contents are malformed.
    let artifact = confined_artifact(&store, &spec)?;
    let parsed = crate::journey::parse(&spec)?;
    let semantic_hash = parsed.semantic_hash()?;
    let body = journey_body(&parsed, &artifact, &semantic_hash);
    let existing = journey_nodes(&store, &parsed.id)?;
    if existing.len() > 1 {
        bail!(
            "journey stable id '{}' is ambiguous ({} nodes)",
            parsed.id,
            existing.len()
        );
    }

    let (journey, added, changed, invalidated) = {
        let tx = store.begin()?;
        let result = match existing.into_iter().next() {
            Some(journey) => {
                let old_hash = journey.body.get("semantic_hash").and_then(Value::as_str);
                let changed = old_hash != Some(semantic_hash.as_str());
                let invalidated = if changed {
                    refresh_or_invalidate_projections(&store, &journey, &parsed, &semantic_hash)?
                } else {
                    0
                };
                if journey.body != body {
                    store.set_node_body(&journey.id, &body)?;
                }
                let current = store
                    .get_node(&journey.id)?
                    .ok_or_else(|| anyhow!("journey vanished during update"))?;
                (current, false, changed, invalidated)
            }
            None => {
                let description = parsed.description.as_deref().unwrap_or(&parsed.goal);
                let journey =
                    store.add_node(NodeType::Journey, &parsed.id, description, "authored", body)?;
                (journey, true, false, 0)
            }
        };
        tx.commit()?;
        result
    };

    pulse::emit_line(
        &store,
        json,
        json!({
            "added": added,
            "updated": !added,
            "changed": changed,
            "invalidated_projections": invalidated,
            "journey": node_json(&journey),
        }),
        &format!("loom journey derive {}", parsed.id),
        if added {
            format!("added Journey '{}'", parsed.id)
        } else {
            format!("updated Journey '{}'", parsed.id)
        },
    )
}

pub(crate) fn journey_remove(graph: Option<&Path>, id: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let nodes = journey_nodes(&store, id)?;
    let [journey] = nodes.as_slice() else {
        if nodes.is_empty() {
            bail!("no Journey with stable id '{id}'");
        }
        bail!("Journey stable id '{id}' is ambiguous");
    };
    let compiled: Vec<Node> = store
        .edges_with(Some(EdgeKind::Proves), None, Some(&journey.id))?
        .into_iter()
        .filter_map(|edge| store.get_node(&edge.from_id).ok().flatten())
        .filter(|validation| validation.node_type == NodeType::Validation)
        .collect();
    let compiled_count = compiled.len();
    let tx = store.begin()?;
    for validation in compiled {
        store.delete_node(&validation.id)?;
    }
    store.delete_node(&journey.id)?;
    tx.commit()?;
    let cache = store
        .root()
        .join(".loom")
        .join("compiled")
        .join("journeys")
        .join(&journey.name);
    if cache.is_dir() {
        std::fs::remove_dir_all(&cache)
            .with_context(|| format!("removing compiled Journey cache {}", cache.display()))?;
    }
    pulse::emit_line(
        &store,
        json,
        json!({
            "removed": true,
            "journey": node_json(journey),
            "removed_compiled_validations": compiled_count,
        }),
        "loom journey list",
        format!("removed Journey '{id}'"),
    )
}

pub(crate) fn journey_list(
    graph: Option<&Path>,
    limit: usize,
    offset: usize,
    json: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let nodes = store.list_nodes_page(Some(NodeType::Journey), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::Journey))?;
    if json {
        let rows: Vec<_> = nodes.iter().map(node_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&super::pagination_envelope(&rows, offset, limit, total))?
        );
    } else {
        for node in &nodes {
            println!(
                "{}  {}  hash={}",
                crate::model::short(&node.id),
                node.name,
                node.body
                    .get("semantic_hash")
                    .and_then(Value::as_str)
                    .unwrap_or("—")
            );
        }
        if let Some(footer) = super::page_footer(nodes.len(), offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}

pub(crate) fn journey_show(
    graph: Option<&Path>,
    journey_key: &str,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let (journey, spec, _) = load_registered_journey(&store, journey_key)?;
    let readiness = crate::completeness::journey_readiness(&store, &journey)?;
    let mut derivations = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
        derivations.push(json!({
            "edge": edge,
            "intent": store.get_node(&edge.to_id)?.map(|node| node_json(&node)),
            "journey_hash": store.get_facet(&edge.id, TargetKind::Edge, "journey_hash")?,
            "step_ids": edge_json_facet(&store, &edge.id, "step_ids"),
        }));
    }
    let mut surfaces = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        surfaces.push(json!({
            "edge": edge,
            "surface": store.get_node(&edge.to_id)?.map(|node| node_json(&node)),
            "journey_hash": store.get_facet(&edge.id, TargetKind::Edge, "journey_hash")?,
            "setup": edge_json_facet(&store, &edge.id, "setup"),
            "operation_bindings": edge_json_facet(&store, &edge.id, "operation_bindings"),
        }));
    }
    let mut proofs = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Proves), None, Some(&journey.id))? {
        proofs.push(json!({
            "edge": edge,
            "validation": store.get_node(&edge.from_id)?.map(|node| node_json(&node)),
        }));
    }
    let value = json!({
        "journey": node_json(&journey),
        "spec": spec.canonical_value()?,
        "readiness": readiness,
        "derivations": derivations,
        "surfaces": surfaces,
        "proofs": proofs,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "{}  authored={} derived={} implemented={} surfaced={} compiled={} proven={}",
            journey.name,
            readiness.authored,
            readiness.derived,
            readiness.implemented,
            readiness.surfaced,
            readiness.compiled,
            readiness.proven
        );
    }
    Ok(())
}

pub(crate) fn journey_map(graph: Option<&Path>, json_output: bool) -> Result<()> {
    let store = open_read(graph)?;
    let readiness = crate::completeness::all_journey_readiness(&store)?;
    let rooted: BTreeSet<String> = readiness
        .iter()
        .flat_map(|journey| journey.derived_intent_ids.iter().cloned())
        .collect();
    let mut unrooted = Vec::new();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated"
            || rooted.contains(&intent.id)
            || crate::completeness::intent_journey_exempt(&store, &intent.id)?
        {
            continue;
        }
        unrooted.push(node_json(&intent));
    }
    unrooted.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "journeys": readiness,
                "unrooted_intents": unrooted,
            }))?
        );
    } else {
        for journey in &readiness {
            println!(
                "{}  authored={} derived={} implemented={} surfaced={} compiled={} proven={}",
                journey.journey_name,
                journey.authored,
                journey.derived,
                journey.implemented,
                journey.surfaced,
                journey.compiled,
                journey.proven
            );
        }
        for intent in &unrooted {
            println!(
                "unrooted  {}  {}",
                intent.get("id").and_then(Value::as_str).unwrap_or(""),
                intent.get("name").and_then(Value::as_str).unwrap_or("")
            );
        }
    }
    Ok(())
}

/// Emit the strict technical-derivation packet. This function is read-only.
pub(crate) fn journey_derive(
    graph: Option<&Path>,
    journey_key: &str,
    candidate_json: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    let mut existing = Vec::new();
    let mut covered = BTreeSet::new();
    for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
        let Some(intent) = store.get_node(&edge.to_id)? else {
            continue;
        };
        let step_ids = edge_json_facet(&store, &edge.id, "step_ids")
            .unwrap_or_else(|| Value::Array(Vec::new()));
        if let Some(ids) = step_ids.as_array() {
            covered.extend(ids.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        existing.push(json!({
            "intent": node_json(&intent),
            "step_ids": step_ids,
            "rationale": store.get_facet(&edge.id, TargetKind::Edge, "rationale")?,
            "proposal_id": store.get_facet(&edge.id, TargetKind::Edge, "proposal_id")?,
            "manifest_hash": store.get_facet(&edge.id, TargetKind::Edge, "manifest_hash")?,
            "journey_hash": store.get_facet(&edge.id, TargetKind::Edge, "journey_hash")?,
        }));
    }
    let uncovered_step_ids: Vec<_> = spec
        .steps
        .iter()
        .filter(|step| !covered.contains(&step.id))
        .map(|step| step.id.clone())
        .collect();
    let mut packet = json!({
        "mode": "derive",
        "journey": node_json(&journey),
        "semantic_hash": semantic_hash,
        "spec": spec.canonical_value()?,
        "existing_derivations": existing,
        "uncovered_step_ids": uncovered_step_ids,
        "manifest_contract": {
            "schema": crate::journey::DERIVATION_SCHEMA,
            "journey_id": spec.id,
            "journey_hash": semantic_hash,
            "proposal_id": "stable-derivation-proposal-id",
            "proposal_rationale": "Why this exact technical projection is sufficient and minimal",
            "intents": [{
                "id": "stable-technical-intent-id",
                "operation": "create",
                "name": "Behavioral technical intent name",
                "criterion": "Falsifiable technical behavior criterion",
                "level": "feature",
                "visibility": "internal",
                "rationale": "Why this is the smallest independently falsifiable technical behavior for the mapped Journey step(s)",
                "step_ids": ["authored-step-id"]
            }],
            "relationships": [{
                "id": "stable-relationship-id",
                "kind": "requires",
                "from": "stable-technical-intent-id",
                "to": "another-included-create-or-reuse-entry-id",
                "rationale": "Why this dependency or hierarchy is required"
            }],
            "unresolved_question": null
        },
        "rules": [
            "Cover every authored step with at least one technical intent.",
            "Use operation=reuse with intent_id to reuse an existing Intent; every relationship endpoint names an included intent entry id.",
            "Every mapping requires a nonempty rationale.",
            "Declare only requires or hierarchy relationships, each with a stable id and nonempty rationale.",
            "A non-null unresolved_question must be answered before derive-accept.",
            "Do not add product behavior or transport details.",
            "Return only a loom.journey-derivation/v1 JSON manifest."
        ],
        "accept_command": format!(
            "loom journey derive-accept {} --manifest <manifest.json> --human-decision <exact-answer>",
            spec.id
        ),
        "human_gate": crate::workitem::derivation_human_gate(&journey),
        "next_action": "Present the human_gate options with an evidence-backed recommendation, wait for the human's exact answer, and only then run accept_command. Missing human authority is a pause, not a terminal handoff."
    });
    if let Some(candidate_json) = candidate_json {
        let manifest = parse_derivation_candidate(candidate_json)?;
        manifest.validate_for(&spec, &semantic_hash)?;
        packet
            .as_object_mut()
            .expect("derive packet is an object")
            .insert(
                "candidate_state".into(),
                derivation_candidate_state(&store, &journey, &spec, &manifest)?,
            );
    }
    emit_packet(&packet, json_output)
}

fn parse_derivation_candidate(raw: &str) -> Result<crate::journey::DerivationManifest> {
    let trimmed = raw.trim_start();
    let (text, source) = if trimmed.starts_with('{') {
        (raw.to_string(), "inline --candidate-json".to_string())
    } else {
        let path = Path::new(raw);
        (
            std::fs::read_to_string(path).with_context(|| {
                format!("reading candidate derivation manifest {}", path.display())
            })?,
            path.display().to_string(),
        )
    };
    serde_json::from_str(&text)
        .with_context(|| format!("parsing {source} as {}", crate::journey::DERIVATION_SCHEMA))
}

fn derivation_manifest_hash(manifest: &crate::journey::DerivationManifest) -> Result<String> {
    let canonical = canonical_json(serde_json::to_value(manifest)?);
    Ok(crate::artifact::fingerprint(&serde_json::to_string(
        &canonical,
    )?))
}

fn candidate_intent_shape_matches(
    store: &Store,
    item: &crate::journey::DerivedIntent,
    node: &Node,
) -> Result<bool> {
    if item.operation == crate::journey::DerivedIntentOperation::Create
        && (node.name != item.name.as_deref().unwrap_or_default()
            || node.description != item.criterion.as_deref().unwrap_or_default())
    {
        return Ok(false);
    }
    Ok(store
        .get_facet(&node.id, TargetKind::Node, "level")?
        .as_deref()
        == Some(item.level.as_str())
        && store
            .get_facet(&node.id, TargetKind::Node, "visibility")?
            .as_deref()
            == Some(item.visibility.as_str()))
}

fn derivation_candidate_state(
    store: &Store,
    journey: &Node,
    spec: &crate::journey::JourneySpec,
    manifest: &crate::journey::DerivationManifest,
) -> Result<Value> {
    let manifest_hash = derivation_manifest_hash(manifest)?;

    let mut matching_proposals: Vec<_> = derivation_proposals(store, &journey.name)?
        .into_iter()
        .filter(|proposal| {
            proposal.status == "adopted"
                && proposal.body.get("proposal_id").and_then(Value::as_str)
                    == Some(manifest.proposal_id.as_str())
                && proposal.body.get("manifest_hash").and_then(Value::as_str)
                    == Some(manifest_hash.as_str())
                && proposal.body.get("journey_hash").and_then(Value::as_str)
                    == Some(manifest.journey_hash.as_str())
        })
        .collect();
    matching_proposals.sort_by(|a, b| a.id.cmp(&b.id));
    let proposal_ids: BTreeSet<_> = matching_proposals
        .iter()
        .map(|proposal| proposal.id.as_str())
        .collect();

    let mut matched = Vec::new();
    for item in &manifest.intents {
        let intent = match item.operation {
            crate::journey::DerivedIntentOperation::Create => {
                find_derived_intent(store, &journey.name, &item.id)?
            }
            crate::journey::DerivedIntentOperation::Reuse => item
                .intent_id
                .as_deref()
                .and_then(|key| store.resolve_node(key, Some(NodeType::Intent)).ok()),
        };
        if let Some(intent) = intent {
            if candidate_intent_shape_matches(store, item, &intent)? {
                matched.push((item, intent));
            }
        }
    }
    let candidate_ids: BTreeSet<_> = matched
        .iter()
        .map(|(_, intent)| intent.id.as_str())
        .collect();

    let mut derives_edges = Vec::new();
    for (item, intent) in &matched {
        let expected_steps = ordered_subset(spec, &item.step_ids);
        let expected_steps = serde_json::to_string(&expected_steps)?;
        for edge in
            store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), Some(&intent.id))?
        {
            let proposal_id = store.get_facet(&edge.id, TargetKind::Edge, "proposal_id")?;
            if store
                .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
                .as_deref()
                != Some(manifest.journey_hash.as_str())
                || store
                    .get_facet(&edge.id, TargetKind::Edge, "manifest_hash")?
                    .as_deref()
                    != Some(manifest_hash.as_str())
                || store
                    .get_facet(&edge.id, TargetKind::Edge, "step_ids")?
                    .as_deref()
                    != Some(expected_steps.as_str())
                || store
                    .get_facet(&edge.id, TargetKind::Edge, "rationale")?
                    .as_deref()
                    != Some(item.rationale.as_str())
                || !proposal_id
                    .as_deref()
                    .is_some_and(|id| proposal_ids.contains(id))
            {
                continue;
            }
            derives_edges.push(json!({
                "entry_id": item.id,
                "edge": edge,
                "step_ids": edge_json_facet(store, &edge.id, "step_ids"),
                "step_hashes": edge_json_facet(store, &edge.id, "step_hashes"),
                "rationale": item.rationale,
                "proposal_id": proposal_id,
                "manifest_hash": manifest_hash,
                "journey_hash": manifest.journey_hash,
            }));
        }
    }

    let ratification_facts: Vec<_> = store
        .all_facts()?
        .into_iter()
        .filter(|fact| {
            candidate_ids.contains(fact.subject_id.as_str())
                && fact.claim == crate::model::Claim::Ratification
        })
        .collect();
    let build_queue_entries: Vec<_> =
        crate::workitem::queue_items(store, crate::lane::Lane::Build)?
            .into_iter()
            .filter(|entry| candidate_ids.contains(entry.target.id.as_str()))
            .collect();
    let readiness = crate::completeness::journey_readiness(store, journey)?;
    let readiness_derived_candidate_ids: Vec<_> = readiness
        .derived_intent_ids
        .into_iter()
        .filter(|id| candidate_ids.contains(id.as_str()))
        .collect();

    Ok(json!({
        "canonical_manifest_hash": manifest_hash,
        "matching_adopted_proposals": matching_proposals.iter().map(node_json).collect::<Vec<_>>(),
        "candidate_intent_matches": matched.iter().map(|(item, intent)| json!({
            "entry_id": item.id,
            "operation": item.operation,
            "intent": node_json(intent),
        })).collect::<Vec<_>>(),
        "derives_edges": derives_edges,
        "ratification_facts": ratification_facts,
        "build_queue_entries": build_queue_entries,
        "readiness_derived_candidate_ids": readiness_derived_candidate_ids,
    }))
}

#[derive(Clone)]
struct PlannedRelationship {
    id: String,
    kind: EdgeKind,
    from: String,
    to: String,
    rationale: String,
}

type AcceptedRelationship = (PlannedRelationship, crate::model::Edge);

struct ReconciledRelationships {
    accepted: Vec<AcceptedRelationship>,
    created: usize,
    removed: usize,
}

struct DerivationCurrentContext<'a> {
    journey: &'a Node,
    spec: &'a crate::journey::JourneySpec,
    manifest: &'a crate::journey::DerivationManifest,
    manifest_hash: &'a str,
    proposal: &'a Node,
    intents: &'a BTreeMap<String, Node>,
    relationships: &'a [PlannedRelationship],
}

fn plan_relationships(
    store: &Store,
    manifest: &crate::journey::DerivationManifest,
    preexisting: &BTreeMap<String, Node>,
) -> Result<Vec<PlannedRelationship>> {
    let mut planned = Vec::with_capacity(manifest.relationships.len());
    let mut seen = BTreeSet::new();
    for relationship in &manifest.relationships {
        let kind = match relationship.kind {
            crate::journey::DerivedRelationshipKind::Requires => EdgeKind::Requires,
            crate::journey::DerivedRelationshipKind::Hierarchy => EdgeKind::Hierarchy,
        };
        let from_token = intent_plan_token(&relationship.from, preexisting);
        let to_token = intent_plan_token(&relationship.to, preexisting);
        if from_token == to_token {
            bail!(
                "derivation relationship '{}' resolves both endpoints to the same Intent",
                relationship.kind.as_str()
            );
        }
        let key = relationship_key(kind, &from_token, &to_token);
        if !seen.insert(key) {
            bail!(
                "derivation contains a duplicate resolved '{}' relationship",
                relationship.kind.as_str()
            );
        }
        planned.push(PlannedRelationship {
            id: relationship.id.clone(),
            kind,
            from: relationship.from.clone(),
            to: relationship.to.clone(),
            rationale: relationship.rationale.clone(),
        });
    }
    validate_prospective_relationship_cycles(store, &planned, preexisting)?;
    Ok(planned)
}

fn intent_plan_token(local: &str, preexisting: &BTreeMap<String, Node>) -> String {
    preexisting
        .get(local)
        .map(|node| node.id.clone())
        .unwrap_or_else(|| format!("new:{local}"))
}

fn relationship_key(kind: EdgeKind, from: &str, to: &str) -> String {
    format!("{}\0{from}\0{to}", kind.as_str())
}

fn validate_prospective_relationship_cycles(
    store: &Store,
    planned: &[PlannedRelationship],
    preexisting: &BTreeMap<String, Node>,
) -> Result<()> {
    for kind in [EdgeKind::Requires, EdgeKind::Hierarchy] {
        let mut edges: Vec<(String, String)> = store
            .edges_with(Some(kind), None, None)?
            .into_iter()
            .map(|edge| (edge.from_id, edge.to_id))
            .collect();
        let additions: Vec<(String, String)> = planned
            .iter()
            .filter(|relationship| relationship.kind == kind)
            .map(|relationship| {
                (
                    intent_plan_token(&relationship.from, preexisting),
                    intent_plan_token(&relationship.to, preexisting),
                )
            })
            .collect();
        edges.extend(additions.iter().cloned());
        for (from, to) in additions {
            if command_relationship_path_exists(&to, &from, &edges, &mut BTreeSet::new()) {
                bail!(
                    "accepting derivation would create a '{}' relationship cycle through '{}' and '{}'",
                    kind.as_str(),
                    from,
                    to
                );
            }
        }
    }
    Ok(())
}

fn command_relationship_path_exists(
    current: &str,
    target: &str,
    edges: &[(String, String)],
    seen: &mut BTreeSet<String>,
) -> bool {
    if current == target {
        return true;
    }
    if !seen.insert(current.to_string()) {
        return false;
    }
    edges
        .iter()
        .filter(|(from, _)| from == current)
        .any(|(_, to)| command_relationship_path_exists(to, target, edges, seen))
}

fn validate_reused_intent_shape(
    store: &Store,
    item: &crate::journey::DerivedIntent,
    node: &Node,
) -> Result<()> {
    let level = store.get_facet(&node.id, TargetKind::Node, "level")?;
    let visibility = store.get_facet(&node.id, TargetKind::Node, "visibility")?;
    if level.as_deref() != Some(item.level.as_str())
        || visibility.as_deref() != Some(item.visibility.as_str())
    {
        bail!(
            "derived intent '{}' declares level='{}' visibility='{}', but reused Intent '{}' does not match",
            item.id,
            item.level,
            item.visibility,
            node.name
        );
    }
    Ok(())
}

fn derivation_proposals(store: &Store, journey_id: &str) -> Result<Vec<Node>> {
    Ok(store
        .list_nodes(Some(NodeType::Proposal), usize::MAX)?
        .into_iter()
        .filter(|proposal| {
            proposal.body.get("source").and_then(Value::as_str) == Some("journey_derivation")
                && proposal.body.get("journey_id").and_then(Value::as_str) == Some(journey_id)
        })
        .collect())
}

fn proposal_body(
    manifest: &crate::journey::DerivationManifest,
    manifest_hash: &str,
    journey: &Node,
    decision: &crate::ratification::HumanDecision,
    accepted: &[(crate::journey::DerivedIntent, Node)],
    relationships: &[AcceptedRelationship],
) -> Result<Value> {
    let canonical_manifest = canonical_json(serde_json::to_value(manifest)?);
    let raw = serde_json::to_string(&canonical_manifest)?;
    Ok(json!({
        "source": "journey_derivation",
        "source_path": Value::Null,
        "raw": raw,
        "proposal_id": manifest.proposal_id,
        "proposal_rationale": manifest.proposal_rationale,
        "journey_id": journey.name,
        "journey_node_id": journey.id,
        "journey_hash": manifest.journey_hash,
        "manifest_hash": manifest_hash,
        "human_decision": decision,
        "items": accepted.iter().enumerate().map(|(index, (item, node))| json!({
            "number": index + 1,
            "kind": "journey_intent",
            "status": "adopted",
            "text": item.rationale,
            "intent_entry_id": item.id,
            "operation": item.operation,
            "step_ids": item.step_ids,
            "spawned": node.id,
        })).collect::<Vec<_>>(),
        "relationships": relationships.iter().map(|(relationship, edge)| json!({
            "id": relationship.id,
            "kind": relationship.kind.as_str(),
            "from": relationship.from,
            "to": relationship.to,
            "rationale": relationship.rationale,
            "edge_id": edge.id,
        })).collect::<Vec<_>>(),
    }))
}

fn relationship_bindings(store: &Store, edge_id: &str) -> Result<BTreeMap<String, Value>> {
    store
        .get_facet(edge_id, TargetKind::Edge, "journey_derivation_bindings")?
        .map(|raw| serde_json::from_str(&raw).context("decoding relationship ownership"))
        .transpose()
        .map(|value| value.unwrap_or_default())
}

fn reconcile_derivation_relationships(
    store: &Store,
    journey_proposals: &[Node],
    proposal: &Node,
    manifest_hash: &str,
    relationships: &[PlannedRelationship],
    intents: &BTreeMap<String, Node>,
) -> Result<ReconciledRelationships> {
    let replacing: BTreeSet<&str> = journey_proposals
        .iter()
        .map(|proposal| proposal.id.as_str())
        .chain(std::iter::once(proposal.id.as_str()))
        .collect();
    let mut declared = BTreeMap::new();
    for relationship in relationships {
        let from = intents.get(&relationship.from).ok_or_else(|| {
            anyhow!(
                "relationship from intent '{}' was not resolved",
                relationship.from
            )
        })?;
        let to = intents.get(&relationship.to).ok_or_else(|| {
            anyhow!(
                "relationship to intent '{}' was not resolved",
                relationship.to
            )
        })?;
        declared.insert(
            relationship_key(relationship.kind, &from.id, &to.id),
            relationship.clone(),
        );
    }

    let mut accepted = Vec::new();
    let mut seen = BTreeSet::new();
    let mut removed = 0usize;
    for kind in [EdgeKind::Requires, EdgeKind::Hierarchy] {
        for edge in store.edges_with(Some(kind), None, None)? {
            let key = relationship_key(kind, &edge.from_id, &edge.to_id);
            let wanted = declared.get(&key);
            let mut bindings = relationship_bindings(store, &edge.id)?;
            bindings.retain(|owner, _| !replacing.contains(owner.as_str()));
            if let Some(relationship) = wanted {
                bindings.insert(
                    proposal.id.clone(),
                    json!({
                        "relationship_id": relationship.id,
                        "rationale": relationship.rationale,
                        "manifest_hash": manifest_hash,
                    }),
                );
                accepted.push((relationship.clone(), edge.clone()));
                seen.insert(key.clone());
            }
            if bindings.is_empty() {
                let created_by_derivation = store
                    .get_facet(&edge.id, TargetKind::Edge, "journey_derivation_created")?
                    .as_deref()
                    == Some("true");
                if wanted.is_none() && created_by_derivation {
                    store.delete_edge(&edge.id)?;
                    removed += 1;
                } else {
                    store.clear_facet(&edge.id, TargetKind::Edge, "journey_derivation_bindings")?;
                }
            } else {
                store.set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "journey_derivation_bindings",
                    &serde_json::to_string(&bindings)?,
                    TruthClass::Asserted,
                )?;
            }
        }
    }

    let mut created = 0usize;
    for (key, relationship) in declared {
        if seen.contains(&key) {
            continue;
        }
        let from = &intents[&relationship.from];
        let to = &intents[&relationship.to];
        let edge = store.add_edge(relationship.kind, &from.id, &to.id, TruthClass::Asserted)?;
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_derivation_created",
            "true",
            TruthClass::Asserted,
        )?;
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_derivation_bindings",
            &serde_json::to_string(&BTreeMap::from([(
                proposal.id.clone(),
                json!({
                    "relationship_id": relationship.id,
                    "rationale": relationship.rationale,
                    "manifest_hash": manifest_hash,
                }),
            )]))?,
            TruthClass::Asserted,
        )?;
        accepted.push((relationship, edge));
        created += 1;
    }
    accepted.sort_by(|left, right| left.0.id.cmp(&right.0.id));
    Ok(ReconciledRelationships {
        accepted,
        created,
        removed,
    })
}

fn derivation_acceptance_is_current(
    store: &Store,
    context: &DerivationCurrentContext<'_>,
) -> Result<bool> {
    let DerivationCurrentContext {
        journey,
        spec,
        manifest,
        manifest_hash,
        proposal,
        intents,
        relationships,
    } = context;
    if proposal.status != "adopted"
        || proposal.body.get("manifest_hash").and_then(Value::as_str) != Some(manifest_hash)
        || proposal.body.get("journey_hash").and_then(Value::as_str)
            != Some(manifest.journey_hash.as_str())
        || intents.len() != manifest.intents.len()
    {
        return Ok(false);
    }
    let derives = store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)?;
    if derives.len() != manifest.intents.len() {
        return Ok(false);
    }
    let step_hashes = spec.step_hashes()?;
    for item in &manifest.intents {
        let Some(intent) = intents.get(&item.id) else {
            return Ok(false);
        };
        if !super::intent::is_ratified(store, &intent.id)? {
            return Ok(false);
        }
        let Some(edge) = derives.iter().find(|edge| edge.to_id == intent.id) else {
            return Ok(false);
        };
        let ordered_steps = ordered_subset(spec, &item.step_ids);
        let subset: BTreeMap<&str, &str> = ordered_steps
            .iter()
            .filter_map(|id| step_hashes.get(id).map(|hash| (id.as_str(), hash.as_str())))
            .collect();
        if store
            .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
            .as_deref()
            != Some(manifest.journey_hash.as_str())
            || edge_json_facet(store, &edge.id, "step_ids")
                != Some(serde_json::to_value(&ordered_steps)?)
            || edge_json_facet(store, &edge.id, "step_hashes")
                != Some(serde_json::to_value(&subset)?)
            || store
                .get_facet(&edge.id, TargetKind::Edge, "rationale")?
                .as_deref()
                != Some(item.rationale.as_str())
            || store
                .get_facet(&edge.id, TargetKind::Edge, "proposal_id")?
                .as_deref()
                != Some(proposal.id.as_str())
            || store
                .get_facet(&edge.id, TargetKind::Edge, "manifest_hash")?
                .as_deref()
                != Some(manifest_hash)
        {
            return Ok(false);
        }
    }

    let mut declared = BTreeSet::new();
    for relationship in *relationships {
        let from = &intents[&relationship.from];
        let to = &intents[&relationship.to];
        let key = relationship_key(relationship.kind, &from.id, &to.id);
        declared.insert(key);
        let Some(edge) = store
            .edges_with(Some(relationship.kind), Some(&from.id), Some(&to.id))?
            .into_iter()
            .next()
        else {
            return Ok(false);
        };
        let bindings = relationship_bindings(store, &edge.id)?;
        let expected = json!({
            "relationship_id": relationship.id,
            "rationale": relationship.rationale,
            "manifest_hash": manifest_hash,
        });
        if bindings.get(&proposal.id) != Some(&expected) {
            return Ok(false);
        }
    }
    for kind in [EdgeKind::Requires, EdgeKind::Hierarchy] {
        for edge in store.edges_with(Some(kind), None, None)? {
            if relationship_bindings(store, &edge.id)?.contains_key(&proposal.id)
                && !declared.contains(&relationship_key(kind, &edge.from_id, &edge.to_id))
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Atomically accept a human-approved technical derivation manifest.
pub(crate) fn journey_derive_accept(
    graph: Option<&Path>,
    journey_key: &str,
    manifest_path: &Path,
    human_decision: String,
    json_output: bool,
) -> Result<()> {
    let manifest = crate::journey::DerivationManifest::parse_json(manifest_path)?;
    let store = open(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    manifest.validate_for(&spec, &semantic_hash)?;
    if let Some(question) = &manifest.unresolved_question {
        bail!(
            "derivation question '{}' is unresolved: {}; answer it and submit a manifest with unresolved_question:null",
            question.id,
            question.text
        );
    }
    let decision = super::ratification_decision(&journey.name, Some(human_decision))?;
    let manifest_hash = derivation_manifest_hash(&manifest)?;
    let evidence = format!(
        "accepted Journey '{}' derivation at semantic hash {} from canonical manifest {}",
        journey.name, semantic_hash, manifest_hash
    );

    // Resolve all references and provenance replays before the first mutation.
    let mut preexisting = BTreeMap::new();
    let mut resolved_intent_ids = BTreeSet::new();
    for item in &manifest.intents {
        let node = match item.operation {
            crate::journey::DerivedIntentOperation::Reuse => store.resolve_node(
                item.intent_id.as_deref().unwrap_or(""),
                Some(NodeType::Intent),
            )?,
            crate::journey::DerivedIntentOperation::Create => {
                match find_derived_intent(&store, &journey.name, &item.id)? {
                    Some(node) => {
                        if node.name != item.name.as_deref().unwrap_or("")
                            || node.description != item.criterion.as_deref().unwrap_or("")
                        {
                            bail!(
                                "derived intent '{}' already exists with conflicting meaning",
                                item.id
                            );
                        }
                        node
                    }
                    None => continue,
                }
            }
        };
        validate_reused_intent_shape(&store, item, &node)?;
        if !resolved_intent_ids.insert(node.id.clone()) {
            bail!(
                "derived intent '{}' resolves to Intent '{}' already selected by another mapping",
                item.id,
                node.name
            );
        }
        preexisting.insert(item.id.clone(), node);
    }
    let relationships = plan_relationships(&store, &manifest, &preexisting)?;
    let proposals = derivation_proposals(&store, &journey.name)?;
    if let Some(conflict) = proposals.iter().find(|proposal| {
        proposal.body.get("proposal_id").and_then(Value::as_str)
            == Some(manifest.proposal_id.as_str())
            && proposal.body.get("manifest_hash").and_then(Value::as_str)
                != Some(manifest_hash.as_str())
    }) {
        bail!(
            "derivation proposal id '{}' is already adopted by Proposal '{}' with a different manifest",
            manifest.proposal_id,
            conflict.id
        );
    }
    let mut matching: Vec<Node> = proposals
        .iter()
        .filter(|proposal| {
            proposal.body.get("proposal_id").and_then(Value::as_str)
                == Some(manifest.proposal_id.as_str())
                && proposal.body.get("manifest_hash").and_then(Value::as_str)
                    == Some(manifest_hash.as_str())
        })
        .cloned()
        .collect();
    if matching.len() > 1 {
        bail!(
            "Journey '{}' has {} adopted Proposal records for manifest '{}'",
            journey.name,
            matching.len(),
            manifest_hash
        );
    }
    let existing_proposal = matching.pop();

    if let Some(proposal) = &existing_proposal {
        let current = DerivationCurrentContext {
            journey: &journey,
            spec: &spec,
            manifest: &manifest,
            manifest_hash: &manifest_hash,
            proposal,
            intents: &preexisting,
            relationships: &relationships,
        };
        if derivation_acceptance_is_current(&store, &current)? {
            let accepted_nodes: Vec<&Node> = manifest
                .intents
                .iter()
                .filter_map(|item| preexisting.get(&item.id))
                .collect();
            let accepted_ids: Vec<String> =
                accepted_nodes.iter().map(|node| node.id.clone()).collect();
            // Cold import keeps the adopted mapping but drops local journal
            // envelopes. Re-seal the exact derive-accept witness without
            // treating that as a new product decision.
            if !local_derive_accept_envelope_exists(&store, &accepted_ids)? {
                ratification_batch(
                    &store,
                    &accepted_nodes,
                    &evidence,
                    &decision,
                    &semantic_hash,
                )?;
            }
            let accepted: Vec<_> = accepted_nodes.iter().copied().map(node_json).collect();
            return pulse::emit_line(
                &store,
                json_output,
                json!({
                    "accepted": true,
                    "idempotent": true,
                    "journey_id": journey.name,
                    "journey_hash": semantic_hash,
                    "manifest_hash": manifest_hash,
                    "proposal": node_json(proposal),
                    "intents": accepted,
                    "created": 0,
                    "relationships_created": 0,
                    "removed_projection_edges": 0,
                    "removed_relationship_edges": 0,
                }),
                &format!("loom journey surface {}", journey.name),
                format!("derivation for '{}' is already accepted", journey.name),
            );
        }
    }

    let (accepted, proposal, created, removed_edges, relationships_created, relationships_removed) = {
        let tx = store.begin()?;
        let mut accepted: Vec<(crate::journey::DerivedIntent, Node)> = Vec::new();
        let mut accepted_by_local = BTreeMap::new();
        let mut accepted_ids = BTreeSet::new();
        let mut created = 0usize;
        for item in &manifest.intents {
            let node = if let Some(node) = preexisting.get(&item.id) {
                node.clone()
            } else {
                let args = super::intent::IntentAddArgs {
                    name: item.name.clone().unwrap_or_default(),
                    description: item.criterion.clone().unwrap_or_default(),
                    level: item.level.clone(),
                    lifecycle: "planned".into(),
                    visibility: Some(item.visibility.clone()),
                    layer: None,
                    aspect: None,
                    allow_symbol_name: false,
                };
                let node = super::intent::create_intent(&store, &args)
                    .with_context(|| format!("creating derived intent '{}'", item.id))?;
                let mut body = node.body.clone();
                body["source_journey"] = json!(journey.name);
                body["derivation_id"] = json!(item.id);
                body["journey_hash"] = json!(semantic_hash);
                store.set_node_body(&node.id, &body)?;
                created += 1;
                store
                    .get_node(&node.id)?
                    .ok_or_else(|| anyhow!("derived intent vanished after creation"))?
            };
            if !accepted_ids.insert(node.id.clone()) {
                bail!(
                    "derived intent '{}' resolves to Intent '{}' already selected by another mapping",
                    item.id,
                    node.name
                );
            }
            accepted_by_local.insert(item.id.clone(), node.clone());
            accepted.push((item.clone(), node));
        }

        let proposal = match &existing_proposal {
            Some(proposal) => {
                if proposal.status != "adopted" {
                    // loom-stability-exempt: Proposal lifecycle records approval provenance,
                    // not an executable proof outcome.
                    store.set_node_status(&proposal.id, "adopted")?;
                }
                store
                    .get_node(&proposal.id)?
                    .ok_or_else(|| anyhow!("derivation Proposal vanished"))?
            }
            None => store.add_node(
                NodeType::Proposal,
                &format!(
                    "Journey {} derivation {}",
                    journey.name, manifest.proposal_id
                ),
                &manifest.proposal_rationale,
                "adopted",
                proposal_body(&manifest, &manifest_hash, &journey, &decision, &[], &[])?,
            )?,
        };

        let mut to_ratify: Vec<&Node> = Vec::new();
        for (_, node) in &accepted {
            if !super::intent::is_ratified(&store, &node.id)? {
                to_ratify.push(node);
            }
        }
        let batch_id =
            ratification_batch(&store, &to_ratify, &evidence, &decision, &semantic_hash)?;
        for node in &to_ratify {
            match &batch_id {
                Some(batch_id) => store
                    .ratify_intent_from_human_batch(&node.id, &evidence, &decision, batch_id)?,
                None => store.ratify_intent_from_human(&node.id, &evidence, &decision)?,
            }
        }

        let mut wanted_targets = BTreeSet::new();
        for (item, node) in &accepted {
            wanted_targets.insert(node.id.clone());
            let edge = store.ensure_edge(EdgeKind::Derives, &journey.id, &node.id)?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "journey_hash",
                &semantic_hash,
                TruthClass::Asserted,
            )?;
            let ordered_steps = ordered_subset(&spec, &item.step_ids);
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "step_ids",
                &serde_json::to_string(&ordered_steps)?,
                TruthClass::Asserted,
            )?;
            let hashes = spec.step_hashes()?;
            let subset: BTreeMap<&str, &str> = ordered_steps
                .iter()
                .filter_map(|id| hashes.get(id).map(|hash| (id.as_str(), hash.as_str())))
                .collect();
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "step_hashes",
                &serde_json::to_string(&subset)?,
                TruthClass::Asserted,
            )?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "rationale",
                &item.rationale,
                TruthClass::Asserted,
            )?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "proposal_id",
                &proposal.id,
                TruthClass::Asserted,
            )?;
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "manifest_hash",
                &manifest_hash,
                TruthClass::Asserted,
            )?;
        }
        let mut removed_edges = 0usize;
        for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
            if !wanted_targets.contains(&edge.to_id) {
                store.delete_edge(&edge.id)?;
                removed_edges += 1;
            }
        }

        let relationship_result = reconcile_derivation_relationships(
            &store,
            &proposals,
            &proposal,
            &manifest_hash,
            &relationships,
            &accepted_by_local,
        )?;
        let final_body = proposal_body(
            &manifest,
            &manifest_hash,
            &journey,
            &decision,
            &accepted,
            &relationship_result.accepted,
        )?;
        if proposal.body != final_body {
            store.set_node_body(&proposal.id, &final_body)?;
        }
        for prior in &proposals {
            if prior.id != proposal.id && prior.status == "adopted" {
                // loom-stability-exempt: superseding an approval Proposal does not
                // settle or reset any executable proof.
                store.set_node_status(&prior.id, "superseded")?;
            }
        }

        // The adopted Proposal is the durable approval record. Journal only
        // when acceptance actually changes graph state; the exact-state fast
        // path above deliberately emits no duplicate record.
        store.append_journal(
            "journey_derivation_accept",
            &journey.id,
            json!({
                "journey_id": journey.name,
                "journey_hash": semantic_hash,
                "manifest_hash": manifest_hash,
                "proposal_id": proposal.id,
                "subjects": accepted.iter().map(|(_, node)| node.id.clone()).collect::<Vec<_>>(),
                "human_decision": decision,
                "evidence": evidence,
            }),
        )?;
        tx.commit()?;
        let proposal = store
            .get_node(&proposal.id)?
            .ok_or_else(|| anyhow!("derivation Proposal vanished after acceptance"))?;
        (
            accepted,
            proposal,
            created,
            removed_edges,
            relationship_result.created,
            relationship_result.removed,
        )
    };

    pulse::emit_line(
        &store,
        json_output,
        json!({
            "accepted": true,
            "journey_id": journey.name,
            "journey_hash": semantic_hash,
            "manifest_hash": manifest_hash,
            "proposal": node_json(&proposal),
            "intents": accepted.iter().map(|(_, node)| node_json(node)).collect::<Vec<_>>(),
            "created": created,
            "relationships_created": relationships_created,
            "removed_projection_edges": removed_edges,
            "removed_relationship_edges": relationships_removed,
        }),
        &format!("loom journey surface {}", journey.name),
        format!(
            "accepted derivation for '{}' ({} intent(s))",
            journey.name,
            accepted.len()
        ),
    )
}

/// Emit the reusable CLI-surface contract packet. This function is read-only.
pub(crate) fn journey_surface(
    graph: Option<&Path>,
    journey_key: &str,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    let mut derivations = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Derives), Some(&journey.id), None)? {
        if let Some(intent) = store.get_node(&edge.to_id)? {
            derivations.push(json!({
                "intent": node_json(&intent),
                "step_ids": edge_json_facet(&store, &edge.id, "step_ids")
            }));
        }
    }
    let mut surfaces = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        if let Some(surface) = store.get_node(&edge.to_id)? {
            surfaces.push(json!({
                "surface": node_json(&surface),
                "setup": edge_json_facet(&store, &edge.id, "setup"),
                "operation_bindings": edge_json_facet(&store, &edge.id, "operation_bindings")
            }));
        }
    }
    let packet = json!({
        "mode": "surface",
        "journey": node_json(&journey),
        "semantic_hash": semantic_hash,
        "spec": spec.canonical_value()?,
        "accepted_derivations": derivations,
        "existing_surfaces": surfaces,
        "manifest_contract": crate::journey::surface_contract_template(&spec)?,
        "human_decision_binding_contract": {
            "step_id": "authored-human-step-id",
            "human_decision": {
                "operation_id": "prior-bound-presentation-operation-id",
                "pointer": "/work_item"
            }
        },
        "rules": [
            "Bind every authored step exactly once.",
            "Each binding is a strict union: either operation_id, or human_decision naming a prior bound operation and JSON pointer; never both.",
            "A human_decision binding requires graph=local_snapshot, carries no CLI operation or answer, and may leave setup.operations empty.",
            "Operations are reusable structured argv, never shell strings.",
            "The surface exposes exactly one registered CodeFile at a live symbol locator or globally unique attached anchor:<id>.",
            "Optional operation.exercises declare downstream code entries reached through that public operation; they are not additional surface owners and require observed_by to name an assertion in the same operation.",
            "Replace only repository-specific CodeFile keys and locators in the template before acceptance. Operations and bindings are generated from the authored Journey steps.",
            "Source anchors are navigation-only and never prove behavior or create graph relationships.",
            "Every operation emits JSON; do not include HTTP endpoints.",
            "Carry temporary setup from the Journey profile as declarative data only.",
            "When setup is required, bind ordered mutable operations to graph=local_snapshot; the runtime confines every operation to that clone.",
            "Optional git mode=isolated_snapshot may name only nonempty unique registered CodeFile paths; its one-commit fixture and dirty state exist only in the local snapshot.",
            "Optional setup.before_steps maps authored step ids to atomic registered-file transitions in the local snapshot; each action declares expected_hash and exactly one of content or template.",
            "A before_steps template may interpolate only non-secret inputs, run.id, or non-redacted outputs captured by earlier authored steps.",
            "An argv token may be exactly one non-secret scalar ${{ inputs.<id> }} or ${{ steps.<prior-step>.outputs.<id> }} template; mixed token interpolation is forbidden.",
            "Return only a loom.journey.surface/v1 JSON manifest."
        ],
        "accept_command": format!(
            "loom journey surface-accept {} --manifest <manifest.json>",
            spec.id
        )
    });
    emit_packet(&packet, json_output)
}

/// Atomically create/reuse an InterfaceSurface and bind the Journey to it.
pub(crate) fn journey_surface_accept(
    graph: Option<&Path>,
    journey_key: &str,
    manifest_path: &Path,
    json_output: bool,
) -> Result<()> {
    let manifest = crate::journey::SurfaceManifest::parse_json(manifest_path)?;
    let store = open(graph)?;
    let (journey, spec, semantic_hash) = load_registered_journey(&store, journey_key)?;
    manifest.validate_for(&spec, &semantic_hash)?;
    manifest.validate_setup_for_store(&store)?;
    let lint = manifest.lint(&store, &spec, &manifest_path.to_string_lossy())?;
    if let Some(finding) = lint
        .findings
        .iter()
        .find(|finding| finding.severity == crate::journey::JourneyLintSeverity::Blocking)
    {
        let location = finding
            .operation
            .as_deref()
            .map(|operation| format!(" operation '{operation}'"))
            .unwrap_or_default();
        bail!(
            "surface lint blocked acceptance: {}{}: {}",
            finding.rule,
            location,
            finding.message
        );
    }

    let (surface, exposes, created, removed_edges) = {
        let tx = store.begin()?;
        let (surface, exposes, created) =
            super::domain_cmd::create_or_reuse_interface_surface(&store, &manifest.surface)?;
        // loom-stability-exempt: local surface trust state is not a proof verdict.
        store.set_node_status(&surface.id, "declared")?;
        let surface = store
            .get_node(&surface.id)?
            .ok_or_else(|| anyhow!("InterfaceSurface vanished during local authorization"))?;
        let edge = store.ensure_edge(EdgeKind::Surfaces, &journey.id, &surface.id)?;
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "journey_hash",
            &semantic_hash,
            TruthClass::Asserted,
        )?;
        match manifest.canonical_setup()? {
            Some(setup) => store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "setup",
                &serde_json::to_string(&setup)?,
                TruthClass::Asserted,
            )?,
            None => store.clear_facet(&edge.id, TargetKind::Edge, "setup")?,
        }
        let step_hashes = spec.step_hashes()?;
        let binding_hashes: BTreeMap<&str, String> = manifest
            .bindings
            .iter()
            .filter_map(|binding| {
                step_hashes.get(binding.step_id()).map(|step_hash| {
                    (
                        binding.step_id(),
                        crate::artifact::fingerprint(&format!(
                            "{step_hash}\0{}",
                            binding.identity()
                        )),
                    )
                })
            })
            .collect();
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "binding_hashes",
            &serde_json::to_string(&binding_hashes)?,
            TruthClass::Asserted,
        )?;
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "operation_bindings",
            &serde_json::to_string(&manifest.canonical_bindings(&spec))?,
            TruthClass::Asserted,
        )?;
        let mut removed_edges = 0usize;
        for other in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
            if other.id != edge.id {
                store.delete_edge(&other.id)?;
                removed_edges += 1;
            }
        }
        tx.commit()?;
        (surface, exposes, created, removed_edges)
    };

    pulse::emit_line(
        &store,
        json_output,
        json!({
            "accepted": true,
            "journey_id": journey.name,
            "journey_hash": semantic_hash,
            "surface": node_json(&surface),
            "surface_created": created,
            "exposes_edge": exposes,
            "setup": manifest.canonical_setup()?,
            "operation_bindings": manifest.canonical_bindings(&spec),
            "removed_projection_edges": removed_edges,
        }),
        "loom status",
        format!(
            "accepted surface '{}' for Journey '{}'",
            surface.name, journey.name
        ),
    )
}

#[derive(Clone)]
struct CompileSource {
    journey: Node,
    spec: crate::journey::JourneySpec,
    semantic_hash: String,
    surface: Node,
    surface_hash: String,
    setup: Option<crate::journey::SurfaceSetup>,
    bindings: Vec<crate::journey::SurfaceBinding>,
    operations: Vec<crate::journey::CliOperation>,
    derived_intents: Vec<Node>,
}

struct CompileProduct {
    proof: crate::journey_runtime::CompiledJourneyProof,
    spec: crate::journey::JourneySpec,
    validation_id: String,
    root: PathBuf,
    identity: crate::identity::ExecutionIdentity,
    artifact: PathBuf,
    cache_regenerated: bool,
}

#[derive(Clone, Default)]
struct DesiredExerciseTarget {
    surface_locator: Option<String>,
    operation_entries: Vec<crate::journey::JourneyOperationExerciseFacet>,
}

fn compile_source(store: &Store, journey_key: &str, profile: &str) -> Result<CompileSource> {
    let (journey, spec, semantic_hash) = load_registered_journey(store, journey_key)?;
    if !spec.profiles.contains_key(profile) {
        bail!("Journey '{}' has no profile '{profile}'", journey.name);
    }
    let readiness = crate::completeness::journey_readiness(store, &journey)?;
    if !readiness.derived {
        bail!(
            "Journey '{}' is not compile-ready: derivation is absent or stale. Run `loom journey derive {} --json`",
            journey.name,
            spec.id
        );
    }
    if !readiness.derivations_ratified {
        bail!(
            "Journey '{}' is not compile-ready: derivation acceptance is pending. Run `loom journey derive {} --json`, present its structured human_gate options, stop for the human's exact substantive answer, and only then use derive-accept",
            journey.name,
            spec.id
        );
    }
    if !readiness.implemented {
        bail!("Journey '{}' is not compile-ready: accepted technical intents are not implemented and realizing-grounded. Run `loom next --mode build --json`", journey.name);
    }

    let mut current_surfaces = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        if !matches!(
            edge.status,
            InspectionStatus::Uninspected | InspectionStatus::Passing
        ) || store
            .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
            .as_deref()
            != Some(semantic_hash.as_str())
        {
            continue;
        }
        let surface = store
            .get_node(&edge.to_id)?
            .ok_or_else(|| anyhow!("accepted surface target '{}' is missing", edge.to_id))?;
        let bindings = facet_json::<Vec<crate::journey::SurfaceBinding>>(
            store,
            &edge.id,
            "operation_bindings",
        )?;
        let setup = store
            .get_facet(&edge.id, TargetKind::Edge, "setup")?
            .map(|raw| {
                serde_json::from_str::<crate::journey::SurfaceSetup>(&raw)
                    .with_context(|| format!("edge '{}' has invalid setup", edge.id))
            })
            .transpose()?;
        if let Some(setup) = &setup {
            setup.validate_for_store(store)?;
        }
        current_surfaces.push((edge, surface, setup, bindings));
    }
    let [(surface_edge, surface, setup, bindings)] = current_surfaces.as_slice() else {
        bail!(
            "Journey '{}' requires exactly one current hash-bound CLI surface (found {})",
            journey.name,
            current_surfaces.len()
        );
    };
    if surface.status == "quarantined" {
        bail!(
            "Journey '{}' executable CLI surface was imported and is quarantined; locally re-authorize the exact contract with `loom journey surface-accept {} --manifest <manifest.json>` before compile/run",
            journey.name,
            journey.name
        );
    }
    if surface.node_type != NodeType::InterfaceSurface
        || surface.body.get("schema").and_then(Value::as_str)
            != Some(crate::journey::INTERFACE_SURFACE_SCHEMA)
        || surface.body.get("kind").and_then(Value::as_str) != Some("cli")
    {
        bail!(
            "Journey '{}' accepted surface is not a reusable CLI",
            journey.name
        );
    }
    let operations: Vec<crate::journey::CliOperation> = serde_json::from_value(
        surface
            .body
            .get("operations")
            .cloned()
            .ok_or_else(|| anyhow!("InterfaceSurface '{}' has no operations", surface.name))?,
    )
    .with_context(|| format!("decoding InterfaceSurface '{}' operations", surface.name))?;
    let surface_hash = crate::journey::surface_projection_hash(store, &journey)?
        .ok_or_else(|| anyhow!("Journey '{}' has no surface projection hash", journey.name))?;

    let mut exposed_live = 0usize;
    for edge in store.edges_with(Some(EdgeKind::Exposes), Some(&surface.id), None)? {
        if !matches!(
            edge.status,
            InspectionStatus::Uninspected | InspectionStatus::Passing
        ) {
            continue;
        }
        let codefile = store.get_node(&edge.to_id)?.ok_or_else(|| {
            anyhow!(
                "InterfaceSurface exposes missing CodeFile '{}',",
                edge.to_id
            )
        })?;
        if codefile.node_type != NodeType::CodeFile || !store.root().join(&codefile.name).is_file()
        {
            continue;
        }
        exposed_live += 1;
    }
    if exposed_live == 0 {
        bail!(
            "Journey '{}' CLI surface exposes no live CodeFile",
            journey.name
        );
    }

    let mut derived_intents = Vec::new();
    for id in &readiness.derived_intent_ids {
        let intent = store
            .get_node(id)?
            .ok_or_else(|| anyhow!("derived Intent '{id}' is missing"))?;
        derived_intents.push(intent);
    }
    derived_intents.sort_by(|left, right| left.id.cmp(&right.id));

    // The facet is the compiler input; mentioning it here prevents a future
    // refactor from accidentally compiling another surface with the same body.
    let _ = surface_edge;
    Ok(CompileSource {
        journey,
        spec,
        semantic_hash,
        surface: surface.clone(),
        surface_hash,
        setup: setup.clone(),
        bindings: bindings.clone(),
        operations,
        derived_intents,
    })
}

fn compile_internal(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
) -> Result<CompileProduct> {
    let store = open(graph)?;
    let root = store.root().to_path_buf();
    let identity = store.execution_identity();
    let source = compile_source(&store, journey_key, profile)?;
    let proof = crate::journey_runtime::compile_surface(
        &source.spec,
        &source.surface_hash,
        profile,
        source.operations.clone(),
        source.setup.as_ref(),
        &source.bindings,
    )?;
    let cache_regenerated = !crate::journey_runtime::cache_matches(&root, &proof)?;
    let validation_name = format!("journey:{}:{profile}", source.journey.name);
    let body = json!({
        "type": "journey",
        "journey_hash": source.semantic_hash,
        "surface_hash": source.surface_hash,
        "profile": profile,
        "compiler_version": crate::journey::JOURNEY_COMPILER_VERSION,
    });

    let mut candidates: Vec<Node> = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)?
        .into_iter()
        .filter(|node| node.name == validation_name)
        .collect();
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    if candidates
        .iter()
        .any(|node| node.body.get("type").and_then(Value::as_str) != Some("journey"))
    {
        bail!("Validation name '{validation_name}' is occupied by a non-Journey proof");
    }

    // One shared projection: the same canonical operation-exercise
    // expectation compile writes, grading checks, sync reconciles, and
    // doctor reports against. Preview and compile use it too, so their
    // covered-file evidence cannot disagree.
    let projection = crate::journey_exercises::expected_projection(&store, &source.journey)
        .with_context(|| {
            format!(
                "Journey '{}' has no current operation-exercise projection",
                source.journey.name
            )
        })?;
    let validation = {
        let tx = store.begin()?;
        let validation = match candidates.first() {
            Some(node) => node.clone(),
            None => store.add_node(
                NodeType::Validation,
                &validation_name,
                "compiler-owned Journey proof profile",
                "not_run",
                body.clone(),
            )?,
        };
        for duplicate in candidates.iter().skip(1) {
            store.delete_node(&duplicate.id)?;
        }
        let body_changed = validation.body != body;
        if body_changed {
            store.set_node_body(&validation.id, &body)?;
            store.reset_validation_status_for_sync(&validation.id)?;
        }

        let desired_validates: BTreeSet<String> = source
            .derived_intents
            .iter()
            .map(|intent| intent.id.clone())
            .collect();
        let mut exercise_targets: BTreeMap<String, DesiredExerciseTarget> = BTreeMap::new();
        for entry in &projection.public_entries {
            let target = exercise_targets
                .entry(entry.codefile_id.clone())
                .or_default();
            target.surface_locator = entry.locator.clone();
        }
        for exercise in &projection.exercises {
            let target = exercise_targets
                .entry(exercise.codefile_id.clone())
                .or_default();
            target
                .operation_entries
                .push(crate::journey::JourneyOperationExerciseFacet {
                    operation_id: exercise.operation_id.clone(),
                    exercise_id: exercise.exercise_id.clone(),
                    observed_by: exercise.observed_by.clone(),
                    locator: exercise.locator.clone(),
                });
        }
        for target in exercise_targets.values_mut() {
            target.operation_entries.sort_by(|left, right| {
                left.operation_id
                    .cmp(&right.operation_id)
                    .then_with(|| left.exercise_id.cmp(&right.exercise_id))
            });
        }
        let desired_exercises: BTreeSet<String> = exercise_targets.keys().cloned().collect();
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Proves,
            std::iter::once(source.journey.id.clone()).collect(),
            body_changed,
        )?;
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Validates,
            desired_validates,
            body_changed,
        )?;
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Calls,
            std::iter::once(source.surface.id.clone()).collect(),
            body_changed,
        )?;
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Exercises,
            desired_exercises,
            body_changed,
        )?;
        for (codefile_id, target) in &exercise_targets {
            let edge = store.ensure_edge(EdgeKind::Exercises, &validation.id, codefile_id)?;
            let mut locators = BTreeSet::new();
            if let Some(locator) = &target.surface_locator {
                locators.insert(locator.clone());
                store.set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "surface_locator",
                    locator,
                    TruthClass::Asserted,
                )?;
            } else {
                store.clear_facet(&edge.id, TargetKind::Edge, "surface_locator")?;
            }
            for entry in &target.operation_entries {
                locators.insert(entry.locator.clone());
            }
            let aggregated = locators.into_iter().collect::<Vec<_>>().join(";");
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "locator",
                &aggregated,
                TruthClass::Asserted,
            )?;
            if target.operation_entries.is_empty() {
                store.clear_facet(&edge.id, TargetKind::Edge, "journey_operation_exercises")?;
            } else {
                store.set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "journey_operation_exercises",
                    &serde_json::to_string(&target.operation_entries)?,
                    TruthClass::Asserted,
                )?;
            }
        }
        tx.commit()?;
        store
            .get_node(&validation.id)?
            .ok_or_else(|| anyhow!("compiled Validation vanished"))?
    };
    crate::journey::resettle_uninspected_compiler_topology(&store, &validation.id)?;
    let artifact = crate::journey_runtime::write_proof(&root, &proof)?;
    Ok(CompileProduct {
        proof,
        spec: source.spec,
        validation_id: validation.id,
        root,
        identity,
        artifact,
        cache_regenerated,
    })
}

fn reconcile_topology(
    store: &Store,
    validation_id: &str,
    kind: EdgeKind,
    desired: BTreeSet<String>,
    reset: bool,
) -> Result<()> {
    let existing = store.edges_with(Some(kind), Some(validation_id), None)?;
    for edge in &existing {
        let current = matches!(
            edge.status,
            InspectionStatus::Uninspected | InspectionStatus::Passing
        );
        if reset || !desired.contains(&edge.to_id) || !current {
            store.delete_edge(&edge.id)?;
        }
    }
    for target in desired {
        store.ensure_edge(kind, validation_id, &target)?;
    }
    Ok(())
}

fn facet_json<T: serde::de::DeserializeOwned>(
    store: &Store,
    edge_id: &str,
    key: &str,
) -> Result<T> {
    let raw = store
        .get_facet(edge_id, TargetKind::Edge, key)?
        .ok_or_else(|| anyhow!("edge '{}' has no {key} facet", edge_id))?;
    serde_json::from_str(&raw).with_context(|| format!("edge '{}' has invalid {key}", edge_id))
}

pub(crate) fn journey_compile(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    json_output: bool,
) -> Result<()> {
    let product = compile_internal(graph, journey_key, profile)?;
    emit_runtime_value(
        json!({
            "compiled": true,
            "journey_id": product.proof.journey_id,
            "profile": product.proof.profile,
            "journey_hash": product.proof.journey_hash,
            "surface_hash": product.proof.surface_hash,
            "compiler_version": product.proof.compiler_version,
            "validation_id": product.validation_id,
            "artifact": product.artifact.strip_prefix(&product.root).unwrap_or(&product.artifact),
            "cache_regenerated": product.cache_regenerated,
        }),
        json_output,
        "compiled Journey proof",
    )
}

pub(crate) fn journey_run(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    json_output: bool,
) -> Result<()> {
    let product = compile_internal(graph, journey_key, profile)
        .map_err(|error| run_failure(json_output, journey_key, profile, "compile", error))?;
    let store = crate::store::Store::open_with_identity(&product.root, product.identity)
        .map_err(|error| run_failure(json_output, journey_key, profile, "open", error))?;
    match crate::journey::run_interactive_and_settle_compiled_validation(
        &store,
        &product.validation_id,
        &BTreeMap::new(),
    ) {
        Ok(crate::journey::InteractiveJourneyRun::Completed(report)) => {
            emit_report(&report, json_output)
        }
        Ok(crate::journey::InteractiveJourneyRun::Pending(pending)) => emit_runtime_value(
            serde_json::to_value(&pending)?,
            json_output,
            &format!(
                "Journey '{}:{}' is waiting for a human decision",
                pending.binding.journey_id, pending.binding.profile
            ),
        ),
        Err(error) => Err(run_failure(
            json_output,
            journey_key,
            profile,
            "settle",
            error,
        )),
    }
}

/// `journey run` failures must stay machine-readable: with `--json`, print one
/// structured envelope to stdout before exiting non-zero, so consumers parse
/// the stage and reason instead of scraping stderr.
fn run_failure(
    json_output: bool,
    journey_key: &str,
    profile: &str,
    stage: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    if json_output {
        let envelope = json!({
            "status": "error",
            "journey": journey_key,
            "profile": profile,
            "stage": stage,
            "detail": format!("{error:#}"),
        });
        if let Ok(rendered) = serde_json::to_string_pretty(&envelope) {
            println!("{rendered}");
        }
    }
    error.context(format!(
        "journey run '{journey_key}:{profile}' failed during {stage}"
    ))
}

pub(crate) fn journey_diagnose(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    raw_inputs: &[String],
    json_output: bool,
) -> Result<()> {
    let overrides = crate::journey_runtime::parse_overrides(raw_inputs)?;
    let product = compile_internal(graph, journey_key, profile)?;
    let report = match crate::harness::acquire(&product.root, "journey diagnose", &product.identity)
    {
        Ok(_harness) => crate::journey_runtime::execute(
            &product.root,
            &product.spec,
            &product.proof,
            &overrides,
        ),
        Err(error) => blocked_report(&product.proof, error.to_string()),
    };
    emit_report(&report, json_output)
}

pub(crate) fn journey_freeze(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    json_output: bool,
) -> Result<()> {
    let product = compile_internal(graph, journey_key, profile)?;
    let report = match crate::harness::acquire(&product.root, "journey freeze", &product.identity) {
        Ok(_harness) => crate::journey_runtime::execute(
            &product.root,
            &product.spec,
            &product.proof,
            &BTreeMap::new(),
        ),
        Err(error) => blocked_report(&product.proof, error.to_string()),
    };
    if report.status != crate::journey_runtime::RuntimeStatus::Passed {
        return emit_report(&report, json_output);
    }
    let baseline = crate::journey_runtime::write_baseline(&product.root, &report)?;
    emit_runtime_value(
        json!({
            "frozen": true,
            "journey_id": report.journey_id,
            "profile": report.profile,
            "baseline": baseline.strip_prefix(&product.root).unwrap_or(&baseline),
            "assertions_passed": report.assertions_passed,
        }),
        json_output,
        "froze Journey baseline",
    )
}

pub(crate) fn journey_drift(
    graph: Option<&Path>,
    journey_key: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let journeys = match journey_key {
        Some(key) => vec![resolve_journey(&store, key)?],
        None => store.list_nodes(Some(NodeType::Journey), usize::MAX)?,
    };
    let mut rows = Vec::new();
    for journey in journeys {
        let artifact = journey
            .body
            .get("artifact")
            .and_then(Value::as_str)
            .unwrap_or("");
        let spec = match crate::journey::parse(&store.root().join(artifact)) {
            Ok(spec) => spec,
            Err(error) => {
                rows.push(json!({
                    "journey_id": journey.name,
                    "current": false,
                    "repair_class": "authored_artifact_absent_or_invalid",
                    "next_command": Value::Null,
                    "next_action": format!("Inspect product evidence and repair the authored Journey artifact at '{}'; no automatic mutation is safe", artifact),
                    "detail": error.to_string(),
                }));
                continue;
            }
        };
        let readiness = crate::completeness::journey_readiness(&store, &journey)?;
        for profile_id in spec.profiles.keys() {
            match compile_source(&store, &journey.id, profile_id).and_then(|source| {
                crate::journey_runtime::compile_surface(
                    &source.spec,
                    &source.surface_hash,
                    profile_id,
                    source.operations,
                    source.setup.as_ref(),
                    &source.bindings,
                )
            }) {
                Ok(proof) => {
                    let cache_current =
                        crate::journey_runtime::cache_matches(store.root(), &proof)?;
                    let baseline_current =
                        crate::journey_runtime::baseline_current(store.root(), &proof)?;
                    rows.push(json!({
                        "journey_id": journey.name,
                        "profile": profile_id,
                        "cache_current": cache_current,
                        "baseline_current": baseline_current,
                        "current": cache_current && baseline_current != Some(false),
                        "repair_class": if !cache_current { "compiled_cache_stale" } else if baseline_current == Some(false) { "baseline_stale" } else { "current" },
                        "next_command": if !cache_current {
                            Some(format!("loom journey compile {} --profile {}", crate::workitem::q(&journey.name), crate::workitem::q(profile_id)))
                        } else if baseline_current == Some(false) {
                            Some(format!("loom journey freeze {} --profile {}", crate::workitem::q(&journey.name), crate::workitem::q(profile_id)))
                        } else { None },
                    }));
                }
                Err(error) => {
                    let detail = error.to_string();
                    let (repair_class, next_command) =
                        classify_drift_repair(&journey.name, &readiness);
                    let next_action = next_command.is_none().then_some(
                        "Inspect the reported blocker and product/code evidence; no automatic mutation is safe",
                    );
                    rows.push(json!({
                        "journey_id": journey.name,
                        "profile": profile_id,
                        "current": false,
                        "repair_class": repair_class,
                        "next_command": next_command,
                        "next_action": next_action,
                        "detail": detail,
                    }))
                }
            }
        }
    }
    let stale = rows
        .iter()
        .filter(|row| row.get("current").and_then(Value::as_bool) != Some(true))
        .count();
    emit_runtime_value(
        json!({"journeys": rows, "stale": stale}),
        json_output,
        if stale == 0 {
            "Journey compiled artifacts are current"
        } else {
            "Journey compiled artifact drift detected"
        },
    )
}

fn classify_drift_repair(
    journey: &str,
    readiness: &crate::completeness::JourneyReadiness,
) -> (&'static str, Option<String>) {
    let journey = crate::workitem::q(journey);
    if !readiness.derived {
        // Derive is inspect-only. It may reveal an authority gate, but drift
        // must not request authority before Loom says acceptance is pending.
        return (
            "derivation_absent_or_stale",
            Some(format!("loom journey derive {journey} --json")),
        );
    }
    if !readiness.derivations_ratified {
        return (
            "derivation_acceptance_pending",
            Some(format!("loom journey derive {journey} --json")),
        );
    }
    if !readiness.implemented {
        return (
            "not_implemented_or_ungrounded",
            Some("loom next --mode build --json".into()),
        );
    }
    if !readiness.surfaced {
        return (
            "no_accepted_current_surface",
            Some(format!("loom journey surface {journey} --json")),
        );
    }
    ("inspection_required", None)
}

fn blocked_report(
    proof: &crate::journey_runtime::CompiledJourneyProof,
    detail: String,
) -> crate::journey_runtime::RuntimeReport {
    crate::journey_runtime::RuntimeReport {
        journey_id: proof.journey_id.clone(),
        profile: proof.profile.clone(),
        journey_hash: proof.journey_hash.clone(),
        surface_hash: proof.surface_hash.clone(),
        status: crate::journey_runtime::RuntimeStatus::Blocked,
        assertions_passed: 0,
        assertions_failed: 0,
        detail: Some(detail),
        setup: Vec::new(),
        file_transitions: Vec::new(),
        steps: Vec::new(),
        captures: BTreeMap::new(),
        passed_assertions: Vec::new(),
    }
}

fn emit_report(report: &crate::journey_runtime::RuntimeReport, json_output: bool) -> Result<()> {
    emit_runtime_value(
        serde_json::to_value(report)?,
        json_output,
        &format!(
            "Journey '{}:{}' {} ({} assertion(s) passed, {} failed)",
            report.journey_id,
            report.profile,
            report.status.as_str(),
            report.assertions_passed,
            report.assertions_failed
        ),
    )
}

fn emit_runtime_value(value: Value, json_output: bool, text: &str) -> Result<()> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}

fn journey_body(spec: &crate::journey::JourneySpec, artifact: &str, semantic_hash: &str) -> Value {
    let output_ids = spec.steps.iter().flat_map(|step| {
        step.produces
            .keys()
            .map(move |output| format!("steps.{}.outputs.{output}", step.id))
    });
    json!({
        "schema": crate::journey::JOURNEY_SCHEMA,
        "stable_id": spec.id,
        "name": spec.name,
        "actor": spec.actor,
        "goal": spec.goal,
        "description": spec.description,
        "artifact": artifact,
        "semantic_hash": semantic_hash,
        "step_order_hash": spec.step_order_hash(),
        "step_semantics_hash": spec.step_semantics_hash().expect("validated Journey serializes"),
        "step_hashes": spec.step_hashes().expect("validated Journey serializes"),
        "root_semantics_hash": spec.root_semantics_hash().expect("validated Journey serializes"),
        "input_ids": sorted_ids(spec.inputs.keys().map(String::as_str)),
        "preconditions": spec.preconditions,
        "step_ids": spec.step_ids(),
        "output_ids": output_ids.collect::<Vec<_>>(),
        "profile_ids": sorted_ids(spec.profiles.keys().map(String::as_str)),
    })
}

fn refresh_or_invalidate_projections(
    store: &Store,
    journey: &Node,
    spec: &crate::journey::JourneySpec,
    semantic_hash: &str,
) -> Result<usize> {
    let old_root_hash = journey
        .body
        .get("root_semantics_hash")
        .and_then(Value::as_str);
    let root_hash = spec.root_semantics_hash()?;
    let old_step_hashes: BTreeMap<String, String> = journey
        .body
        .get("step_hashes")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    let step_hashes = spec.step_hashes()?;
    let changed_steps: BTreeSet<String> = old_step_hashes
        .keys()
        .chain(step_hashes.keys())
        .filter(|id| old_step_hashes.get(*id) != step_hashes.get(*id))
        .cloned()
        .collect();
    let global_changed = old_root_hash != Some(root_hash.as_str()) || old_step_hashes.is_empty();
    let all_steps: BTreeSet<&str> = spec.steps.iter().map(|step| step.id.as_str()).collect();
    let order: BTreeMap<&str, usize> = spec
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.id.as_str(), index))
        .collect();
    let mut invalidated = 0;

    for kind in [EdgeKind::Derives, EdgeKind::Surfaces] {
        for edge in store.edges_with(Some(kind), Some(&journey.id), None)? {
            let (mut bound_steps, bindings) = match kind {
                EdgeKind::Derives => {
                    let steps: Vec<String> = edge_json_facet(store, &edge.id, "step_ids")
                        .and_then(|value| serde_json::from_value(value).ok())
                        .unwrap_or_default();
                    (steps, None)
                }
                EdgeKind::Surfaces => {
                    let bindings: Vec<crate::journey::SurfaceBinding> =
                        edge_json_facet(store, &edge.id, "operation_bindings")
                            .and_then(|value| serde_json::from_value(value).ok())
                            .unwrap_or_default();
                    let steps = bindings
                        .iter()
                        .map(|binding| binding.step_id().to_string())
                        .collect();
                    (steps, Some(bindings))
                }
                _ => unreachable!(),
            };
            let bound: BTreeSet<&str> = bound_steps.iter().map(String::as_str).collect();
            let malformed_derivation = kind == EdgeKind::Derives && bound.is_empty();
            let incomplete_surface = kind == EdgeKind::Surfaces && bound != all_steps;
            let touches_changed = changed_steps.iter().any(|id| bound.contains(id.as_str()));
            if global_changed || malformed_derivation || incomplete_surface || touches_changed {
                store.delete_edge(&edge.id)?;
                invalidated += 1;
                continue;
            }

            bound_steps.sort_by_key(|id| order.get(id.as_str()).copied().unwrap_or(usize::MAX));
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "journey_hash",
                semantic_hash,
                TruthClass::Asserted,
            )?;
            let subset_hashes: BTreeMap<&str, &str> = bound_steps
                .iter()
                .filter_map(|id| step_hashes.get(id).map(|hash| (id.as_str(), hash.as_str())))
                .collect();
            match (kind, bindings) {
                (EdgeKind::Derives, _) => {
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "step_ids",
                        &serde_json::to_string(&bound_steps)?,
                        TruthClass::Asserted,
                    )?;
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "step_hashes",
                        &serde_json::to_string(&subset_hashes)?,
                        TruthClass::Asserted,
                    )?;
                }
                (EdgeKind::Surfaces, Some(mut bindings)) => {
                    bindings.sort_by_key(|binding| {
                        order.get(binding.step_id()).copied().unwrap_or(usize::MAX)
                    });
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "operation_bindings",
                        &serde_json::to_string(&bindings)?,
                        TruthClass::Asserted,
                    )?;
                    let binding_hashes: BTreeMap<&str, String> = bindings
                        .iter()
                        .filter_map(|binding| {
                            step_hashes.get(binding.step_id()).map(|step_hash| {
                                (
                                    binding.step_id(),
                                    crate::artifact::fingerprint(&format!(
                                        "{step_hash}\0{}",
                                        binding.identity()
                                    )),
                                )
                            })
                        })
                        .collect();
                    store.set_facet(
                        &edge.id,
                        TargetKind::Edge,
                        "binding_hashes",
                        &serde_json::to_string(&binding_hashes)?,
                        TruthClass::Asserted,
                    )?;
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(invalidated)
}

fn sorted_ids<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut ids: Vec<String> = ids.map(str::to_owned).collect();
    ids.sort();
    ids
}

fn journey_nodes(store: &Store, stable_id: &str) -> Result<Vec<Node>> {
    Ok(store
        .list_nodes(Some(NodeType::Journey), usize::MAX)?
        .into_iter()
        .filter(|node| {
            node.name == stable_id
                || node.body.get("stable_id").and_then(Value::as_str) == Some(stable_id)
        })
        .collect())
}

fn resolve_journey(store: &Store, key: &str) -> Result<Node> {
    if let Ok(node) = store.resolve_node(key, Some(NodeType::Journey)) {
        return Ok(node);
    }
    let nodes = journey_nodes(store, key)?;
    match nodes.as_slice() {
        [node] => Ok(node.clone()),
        [] => bail!("no Journey matches '{key}'"),
        _ => bail!("Journey key '{key}' is ambiguous"),
    }
}

fn load_registered_journey(
    store: &Store,
    key: &str,
) -> Result<(Node, crate::journey::JourneySpec, String)> {
    let journey = resolve_journey(store, key)?;
    let artifact = journey
        .body
        .get("artifact")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Journey '{}' has no artifact", journey.name))?;
    let path = store.root().join(artifact);
    let spec = crate::journey::parse(&path)?;
    if spec.id != journey.name {
        bail!(
            "Journey artifact '{}' now declares stable id '{}', not '{}'",
            artifact,
            spec.id,
            journey.name
        );
    }
    let hash = spec.semantic_hash()?;
    let registered_hash = journey
        .body
        .get("semantic_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Journey '{}' has no semantic_hash", journey.name))?;
    if registered_hash != hash {
        bail!(
            "Journey artifact '{}' changed semantically; run `loom journey add {artifact}` before projecting it",
            journey.name
        );
    }
    Ok((journey, spec, hash))
}

fn confined_artifact(store: &Store, path: &Path) -> Result<String> {
    let root = store
        .root()
        .canonicalize()
        .with_context(|| format!("resolving graph root {}", store.root().display()))?;
    let artifact = path
        .canonicalize()
        .with_context(|| format!("resolving Journey artifact {}", path.display()))?;
    if !artifact.starts_with(&root) {
        bail!(
            "Journey artifact '{}' is outside graph root {}",
            path.display(),
            store.root().display()
        );
    }
    Ok(artifact.strip_prefix(root)?.to_string_lossy().into_owned())
}

fn node_json(node: &Node) -> Value {
    json!({
        "id": node.id,
        "type": node.node_type.as_str(),
        "name": node.name,
        "description": node.description,
        "status": node.status,
        "body": node.body,
    })
}

fn edge_json_facet(store: &Store, edge_id: &str, key: &str) -> Option<Value> {
    store
        .get_facet(edge_id, TargetKind::Edge, key)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn ordered_subset(spec: &crate::journey::JourneySpec, ids: &[String]) -> Vec<String> {
    let wanted: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    spec.steps
        .iter()
        .filter(|step| wanted.contains(step.id.as_str()))
        .map(|step| step.id.clone())
        .collect()
}

fn find_derived_intent(
    store: &Store,
    journey_id: &str,
    derivation_id: &str,
) -> Result<Option<Node>> {
    let mut matches: Vec<_> = store
        .list_nodes(Some(NodeType::Intent), usize::MAX)?
        .into_iter()
        .filter(|node| {
            node.body.get("source_journey").and_then(Value::as_str) == Some(journey_id)
                && node.body.get("derivation_id").and_then(Value::as_str) == Some(derivation_id)
        })
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => bail!(
            "derived intent id '{}' is ambiguous for Journey '{}' ({count} nodes)",
            derivation_id,
            journey_id
        ),
    }
}

fn local_derive_accept_envelope_exists(store: &Store, intent_ids: &[String]) -> Result<bool> {
    if intent_ids.is_empty() {
        return Ok(true);
    }
    let expected_command = format!("journey-derive-accept:{}", intent_ids.len());
    let expected_subjects: BTreeSet<&str> = intent_ids.iter().map(String::as_str).collect();
    for (_, envelope) in crate::batch_auth::load_envelopes(store.root())? {
        let subjects: BTreeSet<&str> = envelope.subjects.iter().map(String::as_str).collect();
        if envelope.command_id == expected_command
            && envelope.claim == crate::batch_auth::BatchClaim::Ratification
            && envelope.operation == "ratify"
            && subjects == expected_subjects
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ratification_batch(
    store: &Store,
    targets: &[&Node],
    evidence: &str,
    decision: &crate::ratification::HumanDecision,
    journey_hash: &str,
) -> Result<Option<String>> {
    if targets.is_empty() {
        return Ok(None);
    }
    let subjects: Vec<String> = targets.iter().map(|node| node.id.clone()).collect();
    let digest = crate::batch_auth::subject_digest(&subjects);
    let pre = store.append_journal(
        "batch_journey_derivation",
        &digest,
        json!({
            "operation": "ratify",
            "subjects": subjects,
            "human_decision": decision,
            "evidence": evidence,
            "journey_hash": journey_hash,
        }),
    )?;
    let now = crate::journal::now_iso();
    let executor = store.execution_identity().actor();
    let envelope = crate::batch_auth::BatchAuthorization::seal(
        crate::batch_auth::BatchClaim::Ratification,
        "ratify",
        subjects,
        "human",
        &executor,
        evidence,
        vec![format!("journal:{}", pre.id)],
    )?
    .with_command_id(format!("journey-derive-accept:{}", targets.len()))
    .with_time_bounds(&now, &now)
    .with_human_decision(decision.clone());
    Ok(Some(
        crate::batch_auth::append_envelope(store, &envelope)?.id,
    ))
}

fn emit_packet(packet: &Value, _json_output: bool) -> Result<()> {
    // Packet commands are JSON operations in both modes: the non-global-json
    // form remains directly pipeable to an LLM or a manifest-writing tool.
    println!("{}", serde_json::to_string_pretty(packet)?);
    Ok(())
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(object) => {
            let sorted: BTreeMap<String, Value> = object
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}
