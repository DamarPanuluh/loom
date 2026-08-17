use super::*;

pub(crate) fn validate_cmd(graph: Option<&Path>, key: &str, all: bool, json: bool) -> Result<()> {
    let store = open(graph)?;
    let vals = collect_validations(&store, key, all)?;
    if vals.is_empty() {
        return pulse::emit_line(
            &store,
            json,
            serde_json::json!({
                "ran": [],
                "summary": { "passed": 0, "failed": 0, "blocked": 0, "skipped": 0 },
            }),
            "loom status",
            "no validations to run",
        );
    }
    refuse_compiler_owned(&store, &vals)?;
    let root = store.root().to_path_buf();
    let execution = store.execution_identity();
    drop(store);
    run_validation_batch(&root, execution, &vals, json)
}

fn collect_validations(store: &Store, key: &str, all: bool) -> Result<Vec<crate::model::Node>> {
    if all {
        return Ok(store
            .list_nodes(Some(NodeType::Validation), usize::MAX)?
            .into_iter()
            .filter(|v| v.status == "not_run")
            .collect());
    }
    match store.resolve_node(key, Some(NodeType::Intent)) {
        Ok(intent) => {
            let mut out = Vec::new();
            for e in store.edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))? {
                if let Some(v) = store.get_node(&e.from_id)? {
                    out.push(v);
                }
            }
            Ok(out)
        }
        Err(intent_error) => match store.resolve_node(key, Some(NodeType::Validation)) {
            Ok(validation) => Ok(vec![validation]),
            Err(_) => Err(intent_error),
        },
    }
}

fn refuse_compiler_owned(store: &Store, vals: &[crate::model::Node]) -> Result<()> {
    for validation in vals {
        if let Some((journey, profile)) =
            crate::completeness::compiler_owned_journey_validation(store, validation)?
        {
            bail!(
                "compiler-owned Journey validations cannot run through validation run; use `loom journey run {} --profile {}`",
                journey.id,
                profile
            );
        }
        if validation
            .body
            .get("type")
            .and_then(serde_json::Value::as_str)
            == Some("journey")
        {
            bail!("Journey validations cannot run through validation run; remove an orphaned proof or use `loom journey run <journey> --profile <profile>`");
        }
    }
    Ok(())
}

fn run_validation_batch(
    root: &Path,
    execution: crate::identity::ExecutionIdentity,
    vals: &[crate::model::Node],
    json: bool,
) -> Result<()> {
    // Serialize proof EXECUTION (not graph writes): a second runner would
    // share ports/processes with this one and mint false failing verdicts.
    let _harness = crate::harness::acquire(root, "validation run", &execution)?;

    let mut results = Vec::new();
    let mut human_lines = Vec::new();
    // Cache observations, not assessments. Several Validation nodes may name
    // one exact execution plan; the subprocess runs once, while every node
    // below still earns its own verdict, covered hashes, stability record,
    // grade, and journal entry.
    let mut observations: std::collections::HashMap<
        crate::proof::CommandExecutionPlan,
        crate::proof::ProofOutcome,
    > = std::collections::HashMap::new();
    for v in vals {
        let command = v
            .body
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let ty = v
            .body
            .get("type")
            .and_then(|t| t.as_str())
            .and_then(|s| s.parse::<crate::model::ValidationType>().ok())
            .unwrap_or(crate::model::ValidationType::Test);
        // The engine records every outcome uniformly; the runner keyed by the
        // validation type owns how the proof is actually attempted.
        let prepared = crate::proof::runner_for(ty).prepare(root, v);
        let execution_plan = prepared.execution_plan().cloned();
        let outcome = match execution_plan {
            Some(plan) => match observations.get(&plan) {
                Some(observed) => observed.clone(),
                None => {
                    let observed = prepared.run();
                    observations.insert(plan, observed.clone());
                    observed
                }
            },
            None => prepared.run(),
        };
        apply_validation_outcome(
            root,
            &execution,
            v,
            OutcomeWrite {
                command: &command,
                ty,
                outcome,
                results: &mut results,
                human_lines: &mut human_lines,
            },
        )?;
    }
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
    let store = open(Some(root))?;
    pulse::emit(
        &store,
        json,
        serde_json::json!({
            "ran": results,
            "summary": {
                "passed": passed,
                "failed": failed,
                "blocked": blocked,
                "skipped": skipped,
            }
        }),
        "loom status",
        || {
            for line in human_lines {
                println!("{line}");
            }
            Ok(())
        },
    )
}

struct OutcomeWrite<'a> {
    command: &'a str,
    ty: crate::model::ValidationType,
    outcome: crate::proof::ProofOutcome,
    results: &'a mut Vec<serde_json::Value>,
    human_lines: &'a mut Vec<String>,
}

fn apply_validation_outcome(
    root: &Path,
    execution: &crate::identity::ExecutionIdentity,
    v: &crate::model::Node,
    write: OutcomeWrite<'_>,
) -> Result<()> {
    match write.outcome {
        crate::proof::ProofOutcome::Passed { evidence, run } => {
            record_observed_validation(
                root,
                execution,
                v,
                ObservedMark {
                    outcome: "passed",
                    evidence: &evidence,
                    reason: "",
                    run: Some(*run),
                },
            )?;
            write.results.push(serde_json::json!({
                "id": v.id,
                "name": v.name,
                "status": "passed",
                "command": write.command,
            }));
            write.human_lines.push(format!("PASS {}", v.name));
        }
        crate::proof::ProofOutcome::Failed {
            evidence,
            exit_code,
            output,
            run,
        } => {
            record_observed_validation(
                root,
                execution,
                v,
                ObservedMark {
                    outcome: "failed",
                    evidence: &evidence,
                    reason: "",
                    run: Some(*run),
                },
            )?;
            let mut row = serde_json::json!({
                "id": v.id,
                "name": v.name,
                "status": "failed",
                "command": write.command,
                "exit_code": exit_code,
            });
            row["output"] = output;
            write.results.push(row);
            write
                .human_lines
                .push(format!("FAIL {} (exit {exit_code})", v.name));
        }
        crate::proof::ProofOutcome::Blocked { reason } => {
            record_observed_validation(
                root,
                execution,
                v,
                ObservedMark {
                    outcome: "blocked",
                    evidence: "",
                    reason: &reason,
                    run: None,
                },
            )?;
            write.results.push(serde_json::json!({
                "id": v.id,
                "name": v.name,
                "status": "blocked",
                "command": write.command,
                "reason": reason,
            }));
            write
                .human_lines
                .push(format!("BLOCKED {} ({reason})", v.name));
        }
        crate::proof::ProofOutcome::Manual { reason } => {
            let next_command = if matches!(write.ty, crate::model::ValidationType::ManualCheck) {
                format!(
                    "loom validation verdict '{}' <passed|failed|blocked> --evidence '<observed evidence>'",
                    v.name
                )
            } else {
                format!(
                    "loom validation update '{}' --command '<runnable-command>'; loom validation run '{}'",
                    v.name, v.name
                )
            };
            write.results.push(serde_json::json!({
                "id": v.id,
                "name": v.name,
                "status": "skipped",
                "reason": reason,
                "next_command": next_command,
            }));
            write
                .human_lines
                .push(format!("skip '{}' ({reason} — {next_command})", v.name));
        }
    }
    Ok(())
}

struct ObservedMark<'a> {
    outcome: &'a str,
    evidence: &'a str,
    reason: &'a str,
    run: Option<crate::evidence::RunRecord>,
}

fn record_observed_validation(
    root: &Path,
    execution: &crate::identity::ExecutionIdentity,
    validation: &crate::model::Node,
    mark: ObservedMark<'_>,
) -> Result<()> {
    let store = crate::store::Store::open_with_identity(root, execution.clone())?;
    mark_validation(
        &store,
        &validation.id,
        mark.outcome,
        mark.evidence,
        mark.reason,
        mark.run,
    )?;
    regrade(&store, &validation.id)?;
    Ok(())
}

/// Run a command loom watches, and keep what it saw.
///
/// The friction collapse. Every other route into the graph asks the agent to
/// describe work it has already done, in loom's vocabulary, after the fact —
/// which is the tax that gets loom skipped. This one asks for a prefix.
///
/// With `--for`, the run binds to that behavior's proof immediately: loom
/// registers a validation if none exists, records the verdict from what it
/// observed, and grades it. Without one, the run is journaled and offered, so a
/// stray `loom observe -- cargo test` still leaves something re-checkable
/// behind rather than nothing.
///
/// The outcome is never taken from the caller. loom ran it; loom says what
/// happened.
pub(crate) fn observe_cmd(
    graph: Option<&Path>,
    target: Option<&str>,
    timeout: u64,
    command: &[String],
    json: bool,
) -> Result<()> {
    let value = observe_run(graph, target, timeout, command)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if value["observed"] == serde_json::json!(false) {
        println!("blocked: {}", value["blocked"].as_str().unwrap_or(""));
        return Ok(());
    }
    // Say what was RECORDED, not only what exited. A reader should not have to
    // know that 101 means a cargo test failed to learn that loom just wrote a
    // failing verdict against their behavior.
    let code = value["exit_code"].as_i64().unwrap_or(-1);
    println!(
        "observed `{}` → {} (exit {code}, {} file(s) covered)",
        value["command"].as_str().unwrap_or(""),
        if code == 0 { "PASSED" } else { "FAILED" },
        value["covered"].as_array().map(|a| a.len()).unwrap_or(0)
    );
    match value["bound_to"].as_str() {
        Some(name) => {
            println!(
                "  recorded against proof '{name}' [{}]",
                value["strength"].as_str().unwrap_or("-")
            );
            if code != 0 {
                println!("  the behavior is now failing — `loom next --mode fix`");
            }
        }
        None => println!(
            "  recorded as journal:{} — bind it with `loom observe --for <behavior> -- …`",
            value["journal"].as_str().unwrap_or("")
        ),
    }
    Ok(())
}
