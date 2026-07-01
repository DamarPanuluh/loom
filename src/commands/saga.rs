//! `loom saga` command family (composition proofs).

use super::open;
use crate::cli::SagaCmd;
use crate::model::{EdgeKind, NodeType};
use crate::store::Store;
use crate::Result;
use anyhow::anyhow;
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
    let text =
        std::fs::read_to_string(&spec).map_err(|e| anyhow!("reading {}: {e}", spec.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| anyhow!("saga spec must be JSON: {e}"))?;
    let name = parsed
        .get("saga")
        .and_then(|v| v.as_str())
        .unwrap_or("saga");
    let steps = parsed
        .get("steps")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let val = store.add_node(
        NodeType::Validation,
        name,
        "",
        "not_run",
        serde_json::json!({ "type": "saga", "command": format!("loom saga run {name}") }),
    )?;
    let mut prev: Option<String> = None;
    let mut linked = 0usize;
    for step in &steps {
        let Some(intent_key) = step.get("intent").and_then(|v| v.as_str()) else {
            continue;
        };
        let intent = store.resolve_node(intent_key, Some(NodeType::Intent))?;
        store.ensure_edge(EdgeKind::Validates, &val.id, &intent.id)?;
        linked += 1;
        if let Some(p) = &prev {
            // sequence edge between consecutive step intents
            let _ = store.ensure_edge(EdgeKind::Sequence, p, &intent.id);
        }
        prev = Some(intent.id);
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "added": true,
                "validation": val,
                "linked_steps": linked,
            }))?
        );
    } else {
        println!("added saga '{}' ({linked} step intent(s))", name);
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
