use super::*;
use process_control::{ChildExt, Control};
use std::process::Stdio;
use std::time::Duration;

const DEFAULT_VALIDATION_TIMEOUT_SECS: u64 = 300;

pub(crate) fn rule(graph: Option<&Path>, cmd: RuleCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        RuleCmd::Seed { pack } => {
            let n = crate::packs::seed(&store, &pack)?;
            println!("seeded pack '{pack}': {n} rule(s)");
            Ok(())
        }
        RuleCmd::Verdict {
            rule,
            intent,
            status,
            criterion,
            evidence,
            confidence,
        } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            let edge = store.ensure_edge(EdgeKind::Governs, &r.id, &i.id)?;
            let st = verdict_status_quality(&status)?;
            store.record_verdict(&edge.id, st, &criterion, &evidence, confidence, "llm")?;
            println!("rule '{}' {} on '{}'", r.name, st, i.name);
            print_next_move(&store)?;
            Ok(())
        }
        RuleCmd::List { limit } => {
            let rules = store.list_nodes(Some(NodeType::QualityRule), limit)?;
            if json {
                let rows: Vec<_> = rules.iter().map(node_json).collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for n in rules {
                    let cat = n
                        .body
                        .get("category")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    println!("{:<14} {} [{}]", cat, n.name, &n.id[..8]);
                }
            }
            Ok(())
        }
        RuleCmd::Show { key } => {
            let n = store.resolve_node(&key, Some(NodeType::QualityRule))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&node_json(&n))?);
            } else {
                println!("{} [{}]", n.name, n.id);
                println!("  {}", n.description);
                if let Some(g) = n.body.get("inspection_guide").and_then(|v| v.as_str()) {
                    println!("  inspection_guide: {g}");
                }
                if let Some(t) = n.body.get("evidence_template") {
                    println!("  evidence_template: {t}");
                }
            }
            Ok(())
        }
        RuleCmd::Add {
            name,
            category,
            description,
        } => {
            let r = store.add_node(
                NodeType::QualityRule,
                &name,
                &description,
                "",
                serde_json::json!({ "category": category }),
            )?;
            println!("added quality rule '{}' [{}]", r.name, &r.id[..8]);
            Ok(())
        }
        RuleCmd::Remove { key } => {
            let r = store.resolve_node(&key, Some(NodeType::QualityRule))?;
            store.delete_node(&r.id)?;
            println!("removed quality rule '{}'", r.name);
            Ok(())
        }
        RuleCmd::Ungovern { rule, intent } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            match store
                .edges_with(Some(EdgeKind::Governs), Some(&r.id), Some(&i.id))?
                .into_iter()
                .next()
            {
                Some(e) => {
                    store.delete_edge(&e.id)?;
                    println!("'{}' no longer governs '{}'", r.name, i.name);
                }
                None => bail!("'{}' does not govern '{}'", r.name, i.name),
            }
            Ok(())
        }
    }
}
fn verdict_status_quality(s: &str) -> Result<InspectionStatus> {
    match s {
        "passing" => Ok(InspectionStatus::Passing),
        "failing" => Ok(InspectionStatus::Failing),
        "independent" => Ok(InspectionStatus::Independent),
        other => bail!("unknown status '{other}' (use passing|failing|independent)"),
    }
}
pub(crate) fn validation(graph: Option<&Path>, cmd: ValidationCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        ValidationCmd::Add {
            name,
            r#type,
            command,
            intent,
            proof_level,
            proof_kind,
            journey_id,
            repo_native_kind,
            artifact,
        } => {
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            if let Some(level) = &proof_level {
                if !matches!(
                    level.as_str(),
                    "L0" | "L1" | "L2" | "L3" | "L4" | "L5" | "L6"
                ) {
                    bail!("unknown proof level '{level}' (use L0..L6)");
                }
            }
            let has_journey_metadata =
                journey_id.is_some() || repo_native_kind.is_some() || artifact.is_some();
            if has_journey_metadata && proof_kind.as_deref() != Some("journey") {
                bail!(
                    "--journey-id, --repo-native-kind, and --artifact require --proof-kind journey"
                );
            }
            let mut body = serde_json::json!({ "type": r#type, "command": command });
            if let Some(v) = proof_level {
                body["proof_level"] = serde_json::json!(v);
            }
            if let Some(v) = proof_kind {
                body["proof_kind"] = serde_json::json!(v);
            }
            if let Some(v) = journey_id {
                body["journey_id"] = serde_json::json!(v);
            }
            if let Some(v) = repo_native_kind {
                body["repo_native_kind"] = serde_json::json!(v);
            }
            if let Some(v) = artifact {
                body["artifact"] = serde_json::json!(v);
            }
            let val = store.add_node(NodeType::Validation, &name, "", "not_run", body)?;
            store.ensure_edge(EdgeKind::Validates, &val.id, &i.id)?;
            println!("added validation '{}' → '{}'", val.name, i.name);
            Ok(())
        }
        ValidationCmd::Mark {
            key,
            result,
            evidence,
            reason,
        } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            mark_validation(&store, &val.id, &result, &evidence, &reason)?;
            println!("validation '{}' → {result}", val.name);
            print_next_move(&store)?;
            Ok(())
        }
        ValidationCmd::Show { key } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            let validates = validation_targets(&store, &val.id)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": val.id,
                        "name": val.name,
                        "status": val.status,
                        "body": val.body,
                        "validates": validates,
                    }))?
                );
            } else {
                println!("{} [{}]", val.name, val.id);
                println!("  status: {}", val.status);
                println!("  {}", val.body);
                for i in validates {
                    println!("  validates: {}", i["name"].as_str().unwrap_or(""));
                }
            }
            Ok(())
        }
        ValidationCmd::Update {
            key,
            r#type,
            command,
        } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            let mut body = val.body.clone();
            if let Some(t) = &r#type {
                body["type"] = serde_json::json!(t);
            }
            if let Some(c) = &command {
                body["command"] = serde_json::json!(c);
            }
            store.set_node_body(&val.id, &body)?;
            println!("updated validation '{}'", val.name);
            Ok(())
        }
        ValidationCmd::Unlink { validation, intent } => {
            let v = store.resolve_node(&validation, Some(NodeType::Validation))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            match store
                .edges_with(Some(EdgeKind::Validates), Some(&v.id), Some(&i.id))?
                .into_iter()
                .next()
            {
                Some(e) => {
                    store.delete_edge(&e.id)?;
                    println!("unlinked '{}' from '{}'", v.name, i.name);
                }
                None => bail!("'{}' does not validate '{}'", v.name, i.name),
            }
            Ok(())
        }
        ValidationCmd::Delete { key } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            store.delete_node(&val.id)?;
            println!("deleted validation '{}'", val.name);
            Ok(())
        }
        ValidationCmd::List { limit } => {
            let vals = store.list_nodes(Some(NodeType::Validation), limit)?;
            if json {
                let rows: Vec<_> = vals
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "id": n.id,
                            "name": n.name,
                            "status": n.status,
                            "body": n.body,
                            "created_at": n.created_at,
                            "updated_at": n.updated_at,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for n in vals {
                    println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
                }
            }
            Ok(())
        }
    }
}
fn validation_targets(store: &Store, val_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut out = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Validates), Some(val_id), None)? {
        let target = store.get_node(&e.to_id)?;
        out.push(serde_json::json!({
            "id": e.to_id,
            "name": target.as_ref().map(|n| n.name.as_str()).unwrap_or(e.to_id.as_str()),
            "edge_id": e.id,
            "edge_status": e.status,
        }));
    }
    Ok(out)
}
fn mark_validation(
    store: &Store,
    val_id: &str,
    result: &str,
    evidence: &str,
    reason: &str,
) -> Result<()> {
    let (node_status, edge_status, ev) = match result {
        "passed" => ("passed", InspectionStatus::Passing, evidence),
        "failed" => ("failed", InspectionStatus::Failing, evidence),
        "blocked" => ("blocked", InspectionStatus::Blocked, reason),
        other => bail!("unknown result '{other}' (use passed|failed|blocked)"),
    };
    // Record the edge verdicts FIRST: record_verdict enforces INV-6 (a
    // passing/failing verdict needs non-empty evidence) and will bail on, e.g.,
    // an empty `--evidence`. Setting the node status only after they all succeed
    // keeps the mark atomic — a rejected verdict never leaves the validation
    // showing `passed` while the command exits non-zero.
    for e in store.edges_with(Some(EdgeKind::Validates), Some(val_id), None)? {
        store.record_verdict(&e.id, edge_status, "proof", ev, 1.0, "llm")?;
    }
    store.set_node_status(val_id, node_status)?;
    Ok(())
}
pub(crate) fn validate_cmd(
    graph: Option<&Path>,
    intent: &str,
    all: bool,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let vals: Vec<_> = if all {
        store
            .list_nodes(Some(NodeType::Validation), usize::MAX)?
            .into_iter()
            .filter(|v| v.status == "not_run")
            .collect()
    } else {
        let i = store.resolve_node(intent, Some(NodeType::Intent))?;
        let mut out = Vec::new();
        for e in store.edges_with(Some(EdgeKind::Validates), None, Some(&i.id))? {
            if let Some(v) = store.get_node(&e.from_id)? {
                out.push(v);
            }
        }
        out
    };
    if vals.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "ran": [],
                    "summary": { "passed": 0, "failed": 0, "blocked": 0, "skipped": 0 },
                }))?
            );
        } else {
            println!("no validations to run");
        }
        return Ok(());
    }
    let root = store.root().to_path_buf();
    let mut results = Vec::new();
    for v in &vals {
        let command = v.body.get("command").and_then(|c| c.as_str()).unwrap_or("");
        if command.is_empty() {
            if json {
                results.push(serde_json::json!({
                    "id": v.id,
                    "name": v.name,
                    "status": "skipped",
                    "reason": "manual_check",
                }));
            } else {
                println!(
                    "skip '{}' (manual_check — use loom validation mark)",
                    v.name
                );
            }
            continue;
        }
        let timeout_secs = validation_timeout_secs(v);
        let out = run_validation_command(&root, command, timeout_secs);
        match out {
            Ok(Some(o)) if o.status.success() => {
                mark_validation(&store, &v.id, "passed", &format!("`{command}` exit 0"), "")?;
                if json {
                    results.push(serde_json::json!({
                        "id": v.id,
                        "name": v.name,
                        "status": "passed",
                        "command": command,
                    }));
                } else {
                    println!("PASS {}", v.name);
                }
            }
            Ok(Some(o)) => {
                let code = o.status.code().unwrap_or(-1);
                mark_validation(
                    &store,
                    &v.id,
                    "failed",
                    &format!("`{command}` exit {code}"),
                    "",
                )?;
                if json {
                    results.push(serde_json::json!({
                        "id": v.id,
                        "name": v.name,
                        "status": "failed",
                        "command": command,
                        "exit_code": code,
                    }));
                } else {
                    println!("FAIL {} (exit {code})", v.name);
                }
            }
            Ok(None) => {
                let reason = format!("`{command}` timed out after {timeout_secs}s");
                mark_validation(&store, &v.id, "blocked", "", &reason)?;
                if json {
                    results.push(serde_json::json!({
                        "id": v.id,
                        "name": v.name,
                        "status": "blocked",
                        "command": command,
                        "reason": reason,
                    }));
                } else {
                    println!("BLOCKED {} (timed out after {timeout_secs}s)", v.name);
                }
            }
            Err(e) => {
                mark_validation(&store, &v.id, "blocked", "", &format!("could not run: {e}"))?;
                if json {
                    results.push(serde_json::json!({
                        "id": v.id,
                        "name": v.name,
                        "status": "blocked",
                        "command": command,
                        "reason": e.to_string(),
                    }));
                } else {
                    println!("BLOCKED {} ({e})", v.name);
                }
            }
        }
    }
    if json {
        let passed = results
            .iter()
            .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("passed"))
            .count();
        let failed = results
            .iter()
            .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("failed"))
            .count();
        let blocked = results
            .iter()
            .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("blocked"))
            .count();
        let skipped = results
            .iter()
            .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("skipped"))
            .count();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ran": results,
                "summary": {
                    "passed": passed,
                    "failed": failed,
                    "blocked": blocked,
                    "skipped": skipped,
                }
            }))?
        );
    }
    Ok(())
}

fn validation_timeout_secs(v: &crate::model::Node) -> u64 {
    v.body
        .get("timeout_seconds")
        .and_then(|value| value.as_u64())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_VALIDATION_TIMEOUT_SECS)
}

fn run_validation_command(
    root: &Path,
    command: &str,
    timeout_secs: u64,
) -> std::io::Result<Option<process_control::Output>> {
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    child
        .controlled_with_output()
        .time_limit(Duration::from_secs(timeout_secs))
        .terminate_for_timeout()
        .wait()
}
