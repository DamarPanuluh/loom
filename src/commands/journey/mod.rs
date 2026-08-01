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
//!
//! Plane: CLI surface over the judgment plane — asserted journey nodes and
//! links; covered/verified state is always derived on read, never stored.

use super::{open, pulse};
use crate::cli::JourneyCmd;
use crate::model::{EdgeKind, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use anyhow::bail;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Dispatch entry point for the `loom journey` family.
pub fn dispatch(graph: Option<&Path>, cmd: JourneyCmd, json: bool) -> Result<()> {
    match cmd {
        JourneyCmd::Add { spec } => journey_add(graph, spec, json),
        JourneyCmd::Remove { id } => journey_remove(graph, &id, json),
        JourneyCmd::List { limit, offset } => journey_list(graph, limit, offset, json),
        JourneyCmd::Map => journey_map(graph, json),
        JourneyCmd::Run { spec, base_url } => journey_run(graph, spec, base_url.as_deref(), json),
        JourneyCmd::Freeze { spec } => journey_freeze(graph, spec, json),
        JourneyCmd::Diagnose { spec, base_url } => {
            journey_diagnose(&spec, base_url.as_deref(), json)
        }
        JourneyCmd::Coverage { cmd } => coverage::coverage(graph, cmd, json),
        JourneyCmd::Invariant { cmd } => invariants::invariant(graph, cmd, json),
        JourneyCmd::Prompt { intent } => prompt::prompt(graph, &intent, json),
    }
}

fn outcome_json(o: &crate::journey::StepOutcome) -> Value {
    json!({
        "name": o.name,
        "step": o.name,
        "intent": o.intent,
        "passed": o.passed,
        "detail": o.detail,
    })
}

pub(super) fn is_journey_validation(node: &Node) -> bool {
    matches!(
        node.body.get("type").and_then(|t| t.as_str()),
        Some("journey")
    ) || node.body.get("proof_kind").and_then(|t| t.as_str()) == Some("journey")
}

fn journey_add(graph: Option<&Path>, spec: PathBuf, json: bool) -> Result<()> {
    let store = open(graph)?;
    let (parsed, kind) = crate::journey::parse_with_kind(&spec)?;
    let artifact = spec.display().to_string();
    // The spec's raw bytes fold into the body as spec_hash, so an edited spec at
    // the SAME path changes the body (a path-only body would miss the common
    // "fixed the spec, re-add" case).
    let raw = std::fs::read_to_string(&spec)
        .map_err(|e| anyhow::anyhow!("reading journey spec {}: {e}", spec.display()))?;
    let spec_hash = crate::artifact::fingerprint(&raw);
    let body = json!({
        "type": "journey",
        "command": format!("loom journey run {artifact}"),
        "proof_kind": "journey",
        "journey_id": parsed.journey,
        "repo_native_kind": kind.as_str(),
        "artifact": artifact,
        "spec_hash": spec_hash,
    });
    // Idempotent upsert by journey_id: reuse the canonical validation for this id,
    // remove any duplicates a prior non-idempotent add left, and — when the spec
    // changed — reset the proof to not_run (a fixed spec must be re-run, never
    // left stale at its old result). Makes add→fix→add→run safe and repairs
    // graphs that already accumulated duplicates.
    let existing = crate::journey::journey_validations(&store, &parsed.journey)?;
    let (val, updated) = match existing.split_first() {
        Some((keep, dups)) => {
            for dup in dups {
                store.delete_node(&dup.id)?;
            }
            if keep.body != body {
                store.set_node_body(&keep.id, &body)?;
                if keep.status != "not_run" {
                    store.set_node_status(&keep.id, "not_run")?;
                }
            }
            let node = store
                .get_node(&keep.id)?
                .ok_or_else(|| anyhow::anyhow!("journey validation vanished after upsert"))?;
            (node, true)
        }
        None => (
            store.add_node(NodeType::Validation, &parsed.journey, "", "not_run", body)?,
            false,
        ),
    };
    // Reconcile step links: ensure the current spec's steps, then drop validates
    // edges for steps the spec no longer names (a renamed/removed step must not
    // keep a stale proof claim).
    let mut linked = 0usize;
    let mut unmatched_steps = Vec::new();
    let mut wanted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for step in &parsed.steps {
        match store.resolve_node(&step.intent, Some(NodeType::Intent)) {
            Ok(intent) => {
                wanted.insert(intent.id.clone());
                store.ensure_edge(EdgeKind::Validates, &val.id, &intent.id)?;
                linked += 1;
            }
            // Soft resolution: consumer specs use human-readable intent text, not
            // Loom ids — report the miss, don't fail.
            Err(_) => unmatched_steps.push(json!({ "step": step.name, "intent": step.intent })),
        }
    }
    for e in store.edges_with(Some(EdgeKind::Validates), Some(&val.id), None)? {
        if !wanted.contains(&e.to_id) {
            store.delete_edge(&e.id)?;
        }
    }
    let unmatched_count = unmatched_steps.len();
    let verb = if updated { "updated" } else { "added" };
    let payload = json!({
        "added": !updated,
        "updated": updated,
        "validation": store.get_node(&val.id)?,
        "linked_steps": linked,
        "unmatched_steps": unmatched_steps,
    });
    let next_step = format!("run `loom journey run {artifact}` to record the proof");
    let line = if unmatched_count > 0 {
        format!(
            "{verb} journey '{}' ({linked} step intent(s)); warning: {unmatched_count} route/step intent(s) were not linked",
            val.name
        )
    } else {
        format!("{verb} journey '{}' ({linked} step intent(s))", val.name)
    };
    pulse::emit_line(&store, json, payload, &next_step, line)
}

fn journey_remove(graph: Option<&Path>, id: &str, json: bool) -> Result<()> {
    let store = open(graph)?;
    let vals = crate::journey::journey_validations(&store, id)?;
    if vals.is_empty() {
        bail!("no journey validation '{id}' to remove");
    }
    let removed = vals.len();
    for v in &vals {
        store.delete_node(&v.id)?;
    }
    pulse::emit_line(
        &store,
        json,
        json!({ "removed": removed, "journey_id": id }),
        "loom status",
        format!("removed journey '{id}' ({removed} validation node(s))"),
    )
}

fn journey_list(graph: Option<&Path>, limit: usize, offset: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    // Journeys are the subset of Validation nodes that pass the filter, so page
    // over the filtered set (fetch all, filter, then skip/take) — a store-level
    // limit would count non-journey validations against the page.
    let all: Vec<_> = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)?
        .into_iter()
        .filter(is_journey_validation)
        .collect();
    let total = all.len();
    let journeys: Vec<_> = all.into_iter().skip(offset).take(limit).collect();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::commands::pagination_envelope(
                &journeys, offset, limit, total
            ))?
        );
    } else {
        for n in &journeys {
            println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
        }
        if let Some(footer) = crate::commands::page_footer(journeys.len(), offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}

fn journey_applicability(
    lifecycle: &str,
    level: Option<&str>,
    visibility: Option<&str>,
) -> (&'static str, &'static str) {
    match (visibility, lifecycle) {
        (Some("user_visible"), "implemented") => (
            "required",
            "implemented user_visible intent has no journey validation",
        ),
        (Some("user_visible"), _) => (
            "not_applicable",
            "journey proof waits until lifecycle is implemented",
        ),
        (Some("internal"), _) => (
            "not_applicable",
            "internal intent — journeys prove user-visible flows",
        ),
        (Some(_), _) => ("unknown_visibility", "visibility facet is not recognized"),
        (None, "implemented") => match level {
            Some("system" | "feature" | "component") => (
                "unknown_visibility",
                "missing visibility on implemented system/feature/component intent",
            ),
            Some("behavior") => (
                "not_applicable",
                "behavior-level intent without user_visible facet is treated as internal",
            ),
            _ => (
                "unknown_visibility",
                "missing visibility on implemented intent",
            ),
        },
        (None, _) => (
            "not_applicable",
            "journey proof waits until lifecycle is implemented",
        ),
    }
}

fn journey_proof_status(store: &Store, intent_id: &str) -> Result<&'static str> {
    if !coverage::current_l5_journey_validations(store, intent_id)?.is_empty() {
        return Ok("passed");
    }
    let mut saw_journey = false;
    let mut saw_failed = false;
    let mut saw_stale = false;
    let mut saw_not_run = false;
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id))? {
        let Some(v) = store.get_node(&e.from_id)? else {
            continue;
        };
        if !is_journey_validation(&v) {
            continue;
        }
        saw_journey = true;
        match (v.status.as_str(), e.status.as_str()) {
            ("failed", _) | (_, "failing") => saw_failed = true,
            (_, "needs_reverification") => saw_stale = true,
            ("not_run", _) | (_, "uninspected") => saw_not_run = true,
            _ => {}
        }
    }
    Ok(if saw_failed {
        "failed"
    } else if saw_stale {
        "stale"
    } else if saw_not_run {
        "not_run"
    } else if saw_journey {
        "unproven"
    } else {
        "missing"
    })
}

fn proof_gap_reason(base: &str, proof_status: &str, coverage_status: &str) -> String {
    match (proof_status, coverage_status) {
        ("missing", "planned_unproven") => {
            "coverage node exists, but no journey validation covers this intent".into()
        }
        ("missing", _) => base.into(),
        ("not_run", _) => "journey validation exists, but has not been run".into(),
        ("failed", _) => "journey validation exists, but current journey proof is failing".into(),
        ("stale", _) => "journey validation exists, but proof needs reverification".into(),
        ("unproven", _) => {
            "journey validation exists, but no current passing L5/L6 proof covers this intent"
                .into()
        }
        _ => base.into(),
    }
}
/// Joined read view: every journey validation with the intents its Validates
/// edges exercise, plus every active intent no journey touches. Both sections
/// are deliberately unbounded — a truncated map would hide exactly the gaps
/// it exists to expose. Linked intents are sorted by name — Validates edges
/// carry no order, and step order lives in the journey spec, not the graph.
fn journey_map(graph: Option<&Path>, json: bool) -> Result<()> {
    const ALL: usize = i64::MAX as usize;
    let store = open(graph)?;
    let journeys: Vec<Node> = store
        .list_nodes(Some(NodeType::Validation), ALL)?
        .into_iter()
        .filter(is_journey_validation)
        .collect();
    let coverage_node_count = store
        .list_nodes(Some(NodeType::JourneyCoverage), ALL)?
        .len();
    let mut journeyed_intent_ids = std::collections::BTreeSet::new();
    let mut journey_rows: Vec<Value> = Vec::new();
    for j in &journeys {
        let mut intents: Vec<(String, Value)> = Vec::new();
        for e in store.edges_with(Some(EdgeKind::Validates), Some(&j.id), None)? {
            let Some(intent) = store.get_node(&e.to_id)? else {
                continue;
            };
            if intent.node_type != NodeType::Intent {
                continue;
            }
            let intent_id = intent.id.clone();
            let sort_key = intent.name.clone();
            journeyed_intent_ids.insert(intent_id.clone());
            let row = json!({
                "id": intent_id,
                "name": intent.name,
                "lifecycle": intent.status,
                "edge_status": e.status.as_str(),
                "journey_proof_status": journey_proof_status(&store, &e.to_id)?,
                "effective_coverage": coverage::effective_coverage(&store, &e.to_id),
            });
            intents.push((sort_key, row));
        }
        intents.sort_by(|a, b| a.0.cmp(&b.0));
        journey_rows.push(json!({
            "id": j.id,
            "name": j.name,
            "status": j.status,
            "artifact": j.body.get("artifact").and_then(|v| v.as_str()),
            "intents": intents.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        }));
    }
    let mut unjourneyed: Vec<Value> = Vec::new();
    let mut journey_gap_intents: Vec<Value> = Vec::new();
    for n in store.list_nodes(Some(NodeType::Intent), ALL)? {
        if n.status == "deprecated" {
            continue;
        }
        let level = store.get_facet(&n.id, TargetKind::Node, "level")?;
        let visibility = store.get_facet(&n.id, TargetKind::Node, "visibility")?;
        let aspect = store.get_facet(&n.id, TargetKind::Node, "aspect")?;
        let (journey_applicability, base_gap_reason) =
            journey_applicability(&n.status, level.as_deref(), visibility.as_deref());
        let proof_status = journey_proof_status(&store, &n.id)?;
        let coverage = coverage::coverage_context(&store, &n.id)?;
        let coverage_status = coverage
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("none");
        let gap_reason = proof_gap_reason(base_gap_reason, proof_status, coverage_status);
        let has_journey = journeyed_intent_ids.contains(&n.id);
        let row = json!({
            "id": n.id,
            "name": n.name,
            "lifecycle": n.status,
            "level": level,
            "visibility": visibility,
            "aspect": aspect,
            "journey_applicability": journey_applicability,
            "journey_gap_reason": gap_reason,
            "journey_proof_status": proof_status,
            "coverage": coverage,
        });
        if !has_journey {
            unjourneyed.push(row.clone());
        }
        if journey_applicability == "required" && proof_status != "passed" {
            journey_gap_intents.push(row);
        }
    }
    journey_gap_intents.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    let passing_journey_intents = journeyed_intent_ids
        .iter()
        .filter(|id| journey_proof_status(&store, id).ok() == Some("passed"))
        .count();
    let unproven_journey_intents = journeyed_intent_ids
        .len()
        .saturating_sub(passing_journey_intents);
    let unknown_visibility = unjourneyed
        .iter()
        .filter(|i| {
            i.get("journey_applicability").and_then(|v| v.as_str()) == Some("unknown_visibility")
        })
        .count();
    let not_applicable = unjourneyed
        .iter()
        .filter(|i| {
            i.get("journey_applicability").and_then(|v| v.as_str()) == Some("not_applicable")
        })
        .count();
    let summary = json!({
        "journeys": journey_rows.len(),
        "coverage_nodes": coverage_node_count,
        "journeyed_intents": journeyed_intent_ids.len(),
        "passing_journey_intents": passing_journey_intents,
        "unproven_journey_intents": unproven_journey_intents,
        "unjourneyed_intents": unjourneyed.len(),
        "journey_required_gaps": journey_gap_intents.len(),
        "unknown_visibility": unknown_visibility,
        "not_applicable": not_applicable,
    });
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "summary": summary,
                "journeys": journey_rows,
                "journey_gap_intents": journey_gap_intents,
                "unjourneyed_intents": unjourneyed,
            }))?
        );
    } else {
        println!(
            "summary: journeys={} coverage_nodes={} journeyed_intents={} passing_journey_intents={} unproven_journey_intents={} unjourneyed_intents={} journey_required_gaps={} unknown_visibility={} not_applicable={}",
            summary["journeys"],
            summary["coverage_nodes"],
            summary["journeyed_intents"],
            summary["passing_journey_intents"],
            summary["unproven_journey_intents"],
            summary["unjourneyed_intents"],
            summary["journey_required_gaps"],
            summary["unknown_visibility"],
            summary["not_applicable"]
        );
        for j in &journey_rows {
            println!(
                "{:<10} {} [{}]",
                j["status"].as_str().unwrap_or(""),
                j["name"].as_str().unwrap_or(""),
                &j["id"].as_str().unwrap_or("")[..8]
            );
            for i in j["intents"].as_array().into_iter().flatten() {
                println!(
                    "    -> {} [{}] edge={} proof={} coverage={}",
                    i["name"].as_str().unwrap_or(""),
                    &i["id"].as_str().unwrap_or("")[..8],
                    i["edge_status"].as_str().unwrap_or(""),
                    i["journey_proof_status"].as_str().unwrap_or(""),
                    i["effective_coverage"].as_str().unwrap_or("")
                );
            }
        }
        println!("journey-required gaps: {}", journey_gap_intents.len());
        for i in &journey_gap_intents {
            println!(
                "{}  {}  proof={} coverage={} reason={}",
                &i["id"].as_str().unwrap_or("")[..8],
                i["name"].as_str().unwrap_or(""),
                i["journey_proof_status"].as_str().unwrap_or(""),
                i["coverage"]["status"].as_str().unwrap_or(""),
                i["journey_gap_reason"].as_str().unwrap_or("")
            );
        }
        println!("intents with no journey: {}", unjourneyed.len());
        for i in &unjourneyed {
            println!(
                "{}  {}  lifecycle={} visibility={} level={} applicability={} proof={} coverage={} reason={}",
                &i["id"].as_str().unwrap_or("")[..8],
                i["name"].as_str().unwrap_or(""),
                i["lifecycle"].as_str().unwrap_or(""),
                i["visibility"].as_str().unwrap_or("—"),
                i["level"].as_str().unwrap_or("—"),
                i["journey_applicability"].as_str().unwrap_or(""),
                i["journey_proof_status"].as_str().unwrap_or(""),
                i["coverage"]["status"].as_str().unwrap_or(""),
                i["journey_gap_reason"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

fn journey_run(
    graph: Option<&Path>,
    spec: PathBuf,
    base_url: Option<&str>,
    json: bool,
) -> Result<()> {
    // Drop the exclusive lock before CLI steps run — nested `loom …` (or any
    // other graph writer) must be able to open the same graph. Same pattern as
    // `validation run`.
    let store = open(graph)?;
    let mut parsed = crate::journey::parse(&spec)?;
    if let Some(base) = base_url {
        parsed.base = base.to_string();
    }
    crate::journey::require(&store, &parsed.journey)?;
    let cwd = store.root().to_path_buf();
    drop(store);

    let mut outcomes = crate::journey::execute_steps(&parsed, Some(&cwd), false)?;
    let store = open(Some(&cwd))?;
    crate::journey::record_outcomes(&store, &parsed, &mut outcomes)?;
    let deviations = crate::journey::read_baseline(&cwd, &parsed.journey)?
        .map(|baseline| crate::journey::deviations(&baseline, &outcomes))
        .unwrap_or_default();
    crate::journal::append(
        store.root(),
        "journey_run",
        &parsed.journey,
        json!({ "outcomes": outcomes, "deviations": deviations }),
    )?;
    let rows: Vec<_> = outcomes.iter().map(outcome_json).collect();
    let passed = outcomes.iter().filter(|o| o.passed).count();
    let failed = outcomes.len().saturating_sub(passed);
    let payload = json!({
        "journey": parsed.journey,
        "passed": passed,
        "failed": failed,
        "total": rows.len(),
        "outcomes": rows,
        "deviations": deviations,
    });
    let next_step = if failed > 0 {
        format!(
            "fix the failing step and rerun `loom journey run {}`",
            spec.display()
        )
    } else {
        "run `loom journey list` to review journey validations".to_string()
    };
    let emitted = pulse::emit(&store, json, payload, &next_step, || {
        for o in &outcomes {
            println!(
                "{} {} — {}",
                if o.passed { "PASS" } else { "FAIL" },
                o.name,
                o.detail
            );
        }
        println!(
            "journey '{}': {}/{} step(s) passed",
            parsed.journey,
            passed,
            outcomes.len()
        );
        Ok(())
    });
    if failed > 0 {
        emitted?;
        bail!(
            "journey '{}' failed ({failed} step(s) failed)",
            parsed.journey
        );
    }
    emitted
}

/// Freeze a differential baseline. Baselines are local files rather than
/// journal lookups so replays stay cheap and deterministic; the freeze event
/// itself is journaled for audit.
fn journey_freeze(graph: Option<&Path>, spec: PathBuf, json: bool) -> Result<()> {
    let store = open(graph)?;
    let parsed = crate::journey::parse(&spec)?;
    let cwd = store.root().to_path_buf();
    drop(store);
    let outcomes = crate::journey::execute_steps(&parsed, Some(&cwd), false)?;
    let path = crate::journey::write_baseline(&cwd, &parsed.journey, &outcomes)?;
    let store = open(Some(&cwd))?;
    let entry = crate::journal::append(
        store.root(),
        "journey_freeze",
        &parsed.journey,
        json!({ "spec": spec, "baseline": path, "outcomes": outcomes }),
    )?;
    pulse::emit_line(
        &store,
        json,
        json!({ "journey": parsed.journey, "baseline": path, "journal": crate::journal::reference(&entry) }),
        "loom journey run",
        format!("froze journey baseline '{}'", parsed.journey),
    )
}

fn journey_diagnose(spec: &Path, base_url: Option<&str>, json: bool) -> Result<()> {
    let mut parsed = crate::journey::parse(spec)?;
    if let Some(base) = base_url {
        parsed.base = base.to_string();
    }
    let hints = crate::journey::diagnose_hints(&parsed);
    let outcomes = crate::journey::execute(None, &parsed, false)?;
    let rows: Vec<_> = outcomes.iter().map(outcome_json).collect();
    let passed = outcomes.iter().filter(|o| o.passed).count();
    let failed = outcomes.len().saturating_sub(passed);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "journey": parsed.journey,
                "passed": passed,
                "failed": failed,
                "total": rows.len(),
                "hints": hints,
                "outcomes": rows,
            }))?
        );
    } else {
        for h in hints {
            println!("hint: {h}");
        }
        for o in &outcomes {
            println!(
                "{} {} — {}",
                if o.passed { "PASS" } else { "FAIL" },
                o.name,
                o.detail
            );
        }
        println!(
            "journey '{}': {}/{} step(s) passed",
            parsed.journey,
            passed,
            outcomes.len()
        );
    }
    if failed > 0 {
        bail!(
            "journey '{}' failed ({} step(s) failed)",
            parsed.journey,
            failed
        )
    } else {
        Ok(())
    }
}

mod coverage;
mod invariants;
mod prompt;
