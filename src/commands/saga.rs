//! `loom saga` command family (composition proofs).

use super::open;
use crate::cli::SagaCmd;
use crate::model::{EdgeKind, NodeType};
use crate::store::Store;
use crate::Result;
use std::path::{Path, PathBuf};

pub fn dispatch(graph: Option<&Path>, cmd: SagaCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        SagaCmd::Add { spec } => saga_add(&store, spec, json),
        SagaCmd::List { limit } => saga_list(&store, limit, json),
        SagaCmd::Run { spec } => saga_run(&store, spec, json),
        SagaCmd::Diagnose { spec } => saga_diagnose(&store, spec, json),
    }
}

fn outcome_json(o: &crate::saga::StepOutcome) -> serde_json::Value {
    serde_json::json!({
        "name": o.name,
        "intent": o.intent,
        "passed": o.passed,
        "detail": o.detail,
    })
}

fn saga_add(store: &Store, spec: PathBuf, json: bool) -> Result<()> {
    let (parsed, kind) = crate::saga::parse_with_kind(&spec)?;
    let artifact = spec.display().to_string();
    let val = store.add_node(
        NodeType::Validation,
        &parsed.saga,
        "",
        "not_run",
        serde_json::json!({
            "type": "saga",
            "command": format!("loom saga run {artifact}"),
            "proof_level": "L5",
            "proof_kind": "journey",
            "journey_id": parsed.saga,
            "repo_native_kind": kind.as_str(),
            "artifact": artifact,
        }),
    )?;
    let mut prev: Option<String> = None;
    let mut linked = 0usize;
    let mut unmatched_steps = Vec::new();
    for step in &parsed.steps {
        let intent = match store.resolve_node(&step.intent, Some(NodeType::Intent)) {
            Ok(intent) => intent,
            Err(_) => {
                // Soft resolution: report but don't fail. Consumer-facing specs
                // use human-readable intent text, not Loom intent IDs.
                unmatched_steps.push(serde_json::json!({
                    "step": step.name,
                    "intent": step.intent,
                }));
                continue;
            }
        };
        store.ensure_edge(EdgeKind::Validates, &val.id, &intent.id)?;
        linked += 1;
        if let Some(p) = &prev {
            // sequence edge between consecutive step intents
            let _ = store.ensure_edge(EdgeKind::Sequence, p, &intent.id);
        }
        prev = Some(intent.id);
    }
    let unmatched_count = unmatched_steps.len();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "added": true,
                "validation": val,
                "linked_steps": linked,
                "unmatched_steps": unmatched_steps,
            }))?
        );
    } else {
        println!("added saga '{}' ({linked} step intent(s))", val.name);
        if unmatched_count > 0 {
            println!("  warning: {unmatched_count} route/step intent(s) were not linked");
        }
    }
    Ok(())
}

fn saga_list(store: &Store, limit: usize, json: bool) -> Result<()> {
    let sagas: Vec<_> = store
        .list_nodes(Some(NodeType::Validation), limit)?
        .into_iter()
        .filter(|n| n.body.get("type").and_then(|t| t.as_str()) == Some("saga"))
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&sagas)?);
    } else {
        for n in &sagas {
            println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
        }
    }
    Ok(())
}

fn saga_run(store: &Store, spec: PathBuf, json: bool) -> Result<()> {
    let parsed = crate::saga::parse(&spec)?;
    crate::saga::require(store, &parsed.saga)?;
    let outcomes = crate::saga::execute(store, &parsed, true)?;
    let passed = outcomes.iter().filter(|o| o.passed).count();
    if json {
        let rows: Vec<_> = outcomes.iter().map(outcome_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "saga": parsed.saga,
                "passed": passed,
                "total": rows.len(),
                "outcomes": rows,
            }))?
        );
        return Ok(());
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
        "saga '{}': {}/{} step(s) passed",
        parsed.saga,
        passed,
        outcomes.len()
    );
    Ok(())
}

fn saga_diagnose(store: &Store, spec: PathBuf, json: bool) -> Result<()> {
    let parsed = crate::saga::parse(&spec)?;
    let hints = crate::saga::diagnose_hints(&parsed);
    let outcomes = crate::saga::execute(store, &parsed, false)?;
    if json {
        let rows: Vec<_> = outcomes.iter().map(outcome_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "saga": parsed.saga,
                "hints": hints,
                "outcomes": rows,
            }))?
        );
        return Ok(());
    }
    for h in hints {
        println!("hint: {h}");
    }
    for o in &outcomes {
        println!(
            "{} {} — {}",
            if o.passed { "ok" } else { "FAIL" },
            o.name,
            o.detail
        );
    }
    Ok(())
}
