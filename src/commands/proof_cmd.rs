//! Proof command family — quality rules, validations, and their verdicts.
//!
//! Plane: CLI surface over the judgment plane. Every settled state written
//! here flows through `Store::record_verdict`, so the evidence gates
//! (INV-4/5/6) and the role gate (INV-7) apply unchanged — this module shapes
//! arguments, resolves names to nodes/edges, and renders output; it must never
//! offer a path around the store's write boundary.

use super::*;

pub(crate) fn rule(graph: Option<&Path>, cmd: RuleCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        RuleCmd::Seed { pack } => {
            let n = crate::packs::seed(&store, &pack)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "pack": pack,
                    "seeded_rules": n,
                }),
                "loom status",
                format!("seeded pack '{pack}': {n} rule(s)"),
            )?;
            Ok(())
        }
        RuleCmd::Verdict {
            rule,
            intent,
            outcome,
            criterion,
            evidence,
            confidence,
        } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            let edge = store.ensure_edge(EdgeKind::Governs, &r.id, &i.id)?;
            let st = verdict_status_quality(&outcome)?;
            let verdict_edge =
                store.record_verdict(&edge.id, st, &criterion, &evidence, confidence, "llm")?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "rule": node_json(&r),
                    "intent": node_json(&i),
                    "edge": verdict_edge,
                    "outcome": &outcome,
                    "criterion": criterion,
                    "evidence": evidence,
                    "confidence": confidence,
                }),
                "loom status",
                format!("rule '{}' {} on '{}'", r.name, st, i.name),
            )?;
            Ok(())
        }
        RuleCmd::List { limit, offset } => {
            let rules = store.list_nodes_page(Some(NodeType::QualityRule), limit, offset)?;
            if json {
                let rows: Vec<_> = rules.iter().map(node_json).collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                let shown = rules.len();
                for n in rules {
                    let cat = n
                        .body
                        .get("category")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    println!("{:<14} {} [{}]", cat, n.name, &n.id[..8]);
                }
                if let Some(footer) = super::page_footer(
                    shown,
                    offset,
                    store.count_nodes(Some(NodeType::QualityRule))?,
                ) {
                    println!("{footer}");
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
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "rule": node_json(&r),
                }),
                "loom status",
                format!("added quality rule '{}' [{}]", r.name, &r.id[..8]),
            )?;
            Ok(())
        }
        RuleCmd::Update {
            key,
            description,
            category,
            severity,
            effort,
            guide,
            hint,
            pattern,
            reason,
        } => {
            if reason.trim().is_empty() {
                bail!("rule update needs substantive --reason");
            }
            if description.is_none()
                && category.is_none()
                && severity.is_none()
                && effort.is_none()
                && guide.is_none()
                && hint.is_empty()
                && pattern.is_empty()
            {
                bail!("nothing to update — pass a rule field to change");
            }
            let r = store.resolve_node(&key, Some(NodeType::QualityRule))?;
            let mut body = r.body.clone();
            if let Some(v) = &category {
                body["category"] = serde_json::json!(v);
            }
            if let Some(v) = &severity {
                body["severity"] = serde_json::json!(v);
            }
            if let Some(v) = &effort {
                body["effort"] = serde_json::json!(v);
            }
            if let Some(v) = &guide {
                body["inspection_guide"] = serde_json::json!(v);
            }
            if !hint.is_empty() {
                body["detection_hints"] = serde_json::json!(hint);
            }
            if !pattern.is_empty() {
                body["patterns"] = serde_json::json!(pattern);
            }
            let updated = if let Some(v) = &description {
                store.update_node(&r.id, None, Some(v), None)?
            } else {
                r.clone()
            };
            store.set_node_body(&r.id, &body)?;
            store.add_note(
                &r.id,
                "decision",
                &format!("updated quality rule: {reason}"),
            )?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "rule": {
                        "id": r.id,
                        "name": r.name,
                        "description": updated.description,
                        "body": body,
                    },
                    "reason": reason,
                }),
                "loom status",
                format!("updated quality rule '{}'", r.name),
            )?;
            Ok(())
        }
        RuleCmd::Remove { key } => {
            let r = store.resolve_node(&key, Some(NodeType::QualityRule))?;
            store.delete_node(&r.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "rule": node_json(&r),
                }),
                "loom status",
                format!("removed quality rule '{}'", r.name),
            )?;
            Ok(())
        }
        RuleCmd::Unlink { rule, intent } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
            match store
                .edges_with(Some(EdgeKind::Governs), Some(&r.id), Some(&i.id))?
                .into_iter()
                .next()
            {
                Some(e) => {
                    store.delete_edge(&e.id)?;
                    pulse::emit_line(
                        &store,
                        json,
                        serde_json::json!({
                            "removed": true,
                            "edge": e,
                            "rule": node_json(&r),
                            "intent": node_json(&i),
                        }),
                        "loom status",
                        format!("'{}' no longer governs '{}'", r.name, i.name),
                    )?;
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
    // `validation run` executes stored proof commands; validate_cmd manages its
    // own store/lock lifecycle, so it must not run under this handler's store.
    let cmd = match cmd {
        ValidationCmd::Run { key, all } => return validate_cmd(graph, &key, all, json),
        other => other,
    };
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
            // Enforce the validation-type vocabulary (M-15/I-5): the CLI advertises
            // a finite set, so reject a typo instead of storing an arbitrary string.
            let vtype = match r#type.parse::<crate::model::ValidationType>() {
                Ok(t) => t,
                Err(_) => bail!(
                    "unknown validation type '{}' (use test|assertion|benchmark|manual_check|journey|scenario|contract)",
                    r#type
                ),
            };
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
            let mut body = serde_json::json!({ "type": vtype.as_str(), "command": command });
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
            let edge = store.ensure_edge(EdgeKind::Validates, &val.id, &i.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "validation": node_json(&val),
                    "intent": node_json(&i),
                    "edge": edge,
                }),
                "loom status",
                format!("added validation '{}' → '{}'", val.name, i.name),
            )?;
            Ok(())
        }
        ValidationCmd::Verdict {
            key,
            outcome,
            evidence,
            reason,
        } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            mark_validation(&store, &val.id, &outcome, &evidence, &reason, None)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "validation": {
                        "id": val.id,
                        "name": val.name,
                        "outcome": &outcome,
                    },
                    "evidence": evidence,
                    "reason": reason,
                }),
                "loom status",
                format!("validation '{}' → {outcome}", val.name),
            )?;
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
                // Re-entering the command through the local CLI is the explicit
                // approval step for a command quarantined during import.
                if let Some(object) = body.as_object_mut() {
                    object.remove("command_trusted");
                }
            }
            store.set_node_body(&val.id, &body)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "validation": {
                        "id": val.id,
                        "name": val.name,
                        "status": val.status,
                        "body": body,
                    },
                }),
                "loom status",
                format!("updated validation '{}'", val.name),
            )?;
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
                    pulse::emit_line(
                        &store,
                        json,
                        serde_json::json!({
                            "removed": true,
                            "edge": e,
                            "validation": node_json(&v),
                            "intent": node_json(&i),
                        }),
                        "loom status",
                        format!("unlinked '{}' from '{}'", v.name, i.name),
                    )?;
                }
                None => bail!("'{}' does not validate '{}'", v.name, i.name),
            }
            Ok(())
        }
        ValidationCmd::Remove { key } => {
            let val = store.resolve_node(&key, Some(NodeType::Validation))?;
            store.delete_node(&val.id)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "removed": true,
                    "validation": node_json(&val),
                }),
                "loom status",
                format!("removed validation '{}'", val.name),
            )?;
            Ok(())
        }
        ValidationCmd::List { limit, offset } => {
            let vals = store.list_nodes_page(Some(NodeType::Validation), limit, offset)?;
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
                let shown = vals.len();
                for n in vals {
                    println!("{:<10} {} [{}]", n.status, n.name, &n.id[..8]);
                }
                if let Some(footer) = super::page_footer(
                    shown,
                    offset,
                    store.count_nodes(Some(NodeType::Validation))?,
                ) {
                    println!("{footer}");
                }
            }
            Ok(())
        }
        // Intercepted before the store is opened (validate_cmd owns its lock).
        ValidationCmd::Run { .. } => unreachable!("`validation run` is handled above"),
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
/// Record a validation's outcome.
///
/// `run` is the observation loom made. When it is `Some`, the verdict is
/// anchored `verified` — loom watched this happen. When it is `None`, the
/// caller is REPORTING an outcome, which for a command-shaped proof is exactly
/// the move that produced 54 unearned green proofs in this graph; the anchor
/// floor refuses it.
/// The file→hash set a proof over this intent depends on: every file grounding
/// it. This is what makes a passing proof expire when the code moves.
fn covered_hashes(
    store: &Store,
    intent_id: &str,
) -> Result<std::collections::BTreeMap<String, String>> {
    let files = crate::runner::files_grounding(store, intent_id)?;
    Ok(files
        .into_iter()
        .map(|f| {
            let hash = std::fs::read_to_string(store.root().join(&f))
                .map(|c| crate::artifact::fingerprint(&c))
                .unwrap_or_default();
            (f, hash)
        })
        .collect())
}

fn mark_validation(
    store: &Store,
    val_id: &str,
    result: &str,
    evidence: &str,
    reason: &str,
    run: Option<crate::evidence::RunRecord>,
) -> Result<()> {
    let (node_status, edge_status, ev) = match result {
        "passed" => ("passed", InspectionStatus::Passing, evidence),
        "failed" => ("failed", InspectionStatus::Failing, evidence),
        // A blocked mark's reason lives in --reason, but a worker following the
        // packet may pass it as --evidence; accept either so the blocker text is
        // never silently dropped (M-2). record_verdict still requires it non-empty.
        "blocked" => (
            "blocked",
            InspectionStatus::Blocked,
            if reason.trim().is_empty() {
                evidence
            } else {
                reason
            },
        ),
        other => bail!("unknown result '{other}' (use passed|failed|blocked)"),
    };
    // Record the edge verdicts FIRST: record_verdict enforces INV-6 (a
    // passing/failing verdict needs non-empty evidence) and will bail on, e.g.,
    // an empty `--evidence`. Setting the node status only after they all succeed
    // keeps the mark atomic — a rejected verdict never leaves the validation
    // showing `passed` while the command exits non-zero.
    for e in store.edges_with(Some(EdgeKind::Validates), Some(val_id), None)? {
        // A proof run anchors the code it exercised: every file grounding the
        // intent it validates. Any later edit to one of those expires the run,
        // so a passing proof stops counting the moment the behavior moves
        // beneath it.
        let mut assertion = crate::store::Assertion::new(
            crate::store::Subject::Edge(e.id.clone()),
            crate::model::Claim::Verdict,
            edge_status.as_str(),
            "loom",
        )
        .criterion("proof")
        .confidence(1.0)
        .cited(crate::evidence::cite(store.root(), ev)?);
        if let Some(run) = run.clone() {
            let mut run = run;
            run.covered = covered_hashes(store, &e.to_id)?;
            assertion = assertion.observed(run);
        }
        store.assert_fact(assertion)?;
    }
    store.set_node_status(val_id, node_status)?;
    crate::journal::append(
        store.root(),
        "validation_verdict",
        val_id,
        serde_json::json!({ "outcome": result, "evidence": ev, "reason": reason }),
    )?;
    Ok(())
}
pub(crate) fn validate_cmd(graph: Option<&Path>, key: &str, all: bool, json: bool) -> Result<()> {
    let store = open(graph)?;
    let vals: Vec<_> = if all {
        store
            .list_nodes(Some(NodeType::Validation), usize::MAX)?
            .into_iter()
            .filter(|v| v.status == "not_run")
            .collect()
    } else {
        match store.resolve_node(key, Some(NodeType::Intent)) {
            Ok(intent) => {
                let mut out = Vec::new();
                for e in store.edges_with(Some(EdgeKind::Validates), None, Some(&intent.id))? {
                    if let Some(v) = store.get_node(&e.from_id)? {
                        out.push(v);
                    }
                }
                out
            }
            Err(intent_error) => match store.resolve_node(key, Some(NodeType::Validation)) {
                Ok(validation) => vec![validation],
                Err(_) => return Err(intent_error),
            },
        }
    };
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
    let root = store.root().to_path_buf();
    drop(store);

    let mut results = Vec::new();
    let mut human_lines = Vec::new();
    for v in &vals {
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
        match crate::proof::runner_for(ty).run(&root, v) {
            crate::proof::ProofOutcome::Passed { evidence, run } => {
                let store = open(Some(&root))?;
                mark_validation(&store, &v.id, "passed", &evidence, "", Some(*run))?;
                drop(store);
                results.push(serde_json::json!({
                    "id": v.id,
                    "name": v.name,
                    "status": "passed",
                    "command": command,
                }));
                human_lines.push(format!("PASS {}", v.name));
            }
            crate::proof::ProofOutcome::Failed {
                evidence,
                exit_code,
                output,
                run,
            } => {
                let store = open(Some(&root))?;
                mark_validation(&store, &v.id, "failed", &evidence, "", Some(*run))?;
                drop(store);
                let mut row = serde_json::json!({
                    "id": v.id,
                    "name": v.name,
                    "status": "failed",
                    "command": command,
                    "exit_code": exit_code,
                });
                row["output"] = output;
                results.push(row);
                human_lines.push(format!("FAIL {} (exit {exit_code})", v.name));
            }
            crate::proof::ProofOutcome::Blocked { reason } => {
                let store = open(Some(&root))?;
                mark_validation(&store, &v.id, "blocked", "", &reason, None)?;
                drop(store);
                results.push(serde_json::json!({
                    "id": v.id,
                    "name": v.name,
                    "status": "blocked",
                    "command": command,
                    "reason": reason,
                }));
                human_lines.push(format!("BLOCKED {} ({reason})", v.name));
            }
            crate::proof::ProofOutcome::Manual { reason } => {
                results.push(serde_json::json!({
                    "id": v.id,
                    "name": v.name,
                    "status": "skipped",
                    "reason": reason,
                }));
                human_lines.push(format!(
                    "skip '{}' ({reason} — use loom validation verdict)",
                    v.name
                ));
            }
        }
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
    let store = open(Some(&root))?;
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
