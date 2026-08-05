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
            let total = store.count_nodes(Some(NodeType::QualityRule))?;
            if json {
                let rows: Vec<_> = rules.iter().map(node_json).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                let shown = rules.len();
                for n in rules {
                    let cat = n
                        .body
                        .get("category")
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    println!("{:<14} {} [{}]", cat, n.name, crate::model::short(&n.id));
                }
                if let Some(footer) = super::page_footer(shown, offset, total) {
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
                format!(
                    "added quality rule '{}' [{}]",
                    r.name,
                    crate::model::short(&r.id)
                ),
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
        RuleCmd::Suppress {
            rule,
            excerpt,
            reason,
        } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let row = store.suppress_hit(&r.name, &excerpt, &reason)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "suppression": row }),
                "loom rule suppressions",
                format!(
                    "suppressed '{}' hit [{}] — answers the same matched text on every future scan",
                    r.name,
                    crate::model::short(&row.content_hash)
                ),
            )?;
            Ok(())
        }
        RuleCmd::Unsuppress { rule, key } => {
            let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
            let row = store.unsuppress_hit(&r.name, &key)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({ "withdrawn": row }),
                "loom rule suppressions",
                format!(
                    "withdrew suppression [{}] on '{}' — the hit re-opens on the next scan",
                    crate::model::short(&row.content_hash),
                    r.name
                ),
            )?;
            Ok(())
        }
        RuleCmd::Suppressions { rule } => {
            let rule_name = match rule {
                Some(k) => Some(store.resolve_node(&k, Some(NodeType::QualityRule))?.name),
                None => None,
            };
            let rows = store.hit_adjudications(rule_name.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else if rows.is_empty() {
                println!("no hit suppressions recorded");
            } else {
                for row in &rows {
                    println!(
                        "{} [{}] {}",
                        row.rule_name,
                        crate::model::short(&row.content_hash),
                        row.excerpt
                    );
                    println!(
                        "  reason: {} — {} ({})",
                        row.reason, row.actor, row.created_at
                    );
                }
                println!("\n{} suppression(s)", rows.len());
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
            warn_if_command_already_proves_another(&store, &command, &i.id, None)?;
            let has_journey_metadata =
                journey_id.is_some() || repo_native_kind.is_some() || artifact.is_some();
            if has_journey_metadata && proof_kind.as_deref() != Some("journey") {
                bail!(
                    "--journey-id, --repo-native-kind, and --artifact require --proof-kind journey"
                );
            }
            let mut body = serde_json::json!({ "type": vtype.as_str(), "command": command });
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
            // The grade, with every conjunct that produced it. A number nobody
            // can argue with is a number nobody can act on.
            let witness: Option<crate::proofstrength::StrengthWitness> = store
                .get_facet(&val.id, crate::model::TargetKind::Node, "proof_strength")?
                .and_then(|j| serde_json::from_str(&j).ok());
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": val.id,
                        "name": val.name,
                        "status": val.status,
                        "body": val.body,
                        "validates": validates,
                        "strength": witness,
                    }))?
                );
            } else {
                println!("{} [{}]", val.name, val.id);
                println!("  status: {}", val.status);
                println!("  {}", val.body);
                if let Some(w) = &witness {
                    println!("  strength: {}", w.grade);
                    println!(
                        "    ran and passed: {} | content assertions: {} | call witness: {} | \
                         baseline clean: {} | boundary: {}",
                        w.ran_and_passed,
                        w.content_assertions,
                        w.call_witness.as_deref().unwrap_or("none"),
                        w.baseline_clean,
                        w.boundary.as_deref().unwrap_or("none"),
                    );
                    if !w.next.is_empty() {
                        println!("    next: {}", w.next);
                    }
                }
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
                warn_if_command_already_proves_another(&store, c, "", Some(&val.id))?;
                body["command"] = serde_json::json!(c);
                // Re-entering the command through the local CLI is the explicit
                // approval step for a command quarantined during import.
                if let Some(object) = body.as_object_mut() {
                    object.remove("command_trusted");
                }
                // A different command is a different proof, so the outcome
                // history is about something else now. Clearing it here is the
                // one place a reset is honest — and it is the flip COMPARISON
                // that resets, never the instability record, which stays until
                // a person adjudicates it.
                if val.body.get("command").and_then(|v| v.as_str()) != Some(c.as_str()) {
                    store.clear_facet(&val.id, TargetKind::Node, "proof_last_outcome")?;
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
            let total = store.count_nodes(Some(NodeType::Validation))?;
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
                println!(
                    "{}",
                    serde_json::to_string_pretty(&super::pagination_envelope(
                        &rows, offset, limit, total
                    ))?
                );
            } else {
                let shown = vals.len();
                for n in vals {
                    println!(
                        "{:<10} {} [{}]",
                        n.status,
                        n.name,
                        crate::model::short(&n.id)
                    );
                }
                if let Some(footer) = super::page_footer(shown, offset, total) {
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
    let files = store.files_grounding(intent_id)?;
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

/// Say so when a command is already registered as another behavior's proof.
///
/// A warning, never a refusal. A ring genuinely covering several behaviors is a
/// legitimate shape — fifteen of this repo's shared commands are exactly that —
/// so refusing would break honest work to catch dishonest work. But the shape
/// is also how a claim goes green over code it never touches: an intent
/// claiming "a locator that cannot resolve falls back to file-scope reopening"
/// carried two passing validations, both running `cargo test --test ring6 -q`,
/// while the behavior did not exist at all.
///
/// Said at write time, which is the only moment it is cheap. Afterwards it
/// costs a smell, a triage verdict, and eventually someone re-deriving why.
fn warn_if_command_already_proves_another(
    store: &Store,
    command: &str,
    intent_id: &str,
    skip_validation: Option<&str>,
) -> Result<()> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }
    let mut others: Vec<String> = Vec::new();
    for val_id in store.validations_with_command(command, skip_validation)? {
        for e in store.edges_with(Some(EdgeKind::Validates), Some(&val_id), None)? {
            if e.to_id == intent_id {
                continue;
            }
            if let Some(other) = store.get_node(&e.to_id)? {
                others.push(other.name);
            }
        }
    }
    others.sort();
    others.dedup();
    if others.is_empty() {
        return Ok(());
    }
    eprintln!(
        "warning: `{command}` is already the proof of {} other behavior(s): {}.\n\
         \x20        One command exercises at most one of them; the rest stand on whatever it\n\
         \x20        really tests. Narrow this proof to the test that asserts THIS behavior,\n\
         \x20        or accept it knowingly — `loom smells` will keep reporting it.",
        others.len(),
        others
            .iter()
            .map(|n| format!("'{n}'"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
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
    store.record_proof_stability(val_id, node_status)?;
    store.set_node_status(val_id, node_status)?;
    crate::journal::append(
        store.root(),
        "validation_verdict",
        val_id,
        serde_json::json!({ "outcome": result, "evidence": ev, "reason": reason }),
    )?;
    Ok(())
}
/// Run one validation through loom and record what loom observed.
///
/// The library path behind `loom validation run` — the ONLY way a `validates`
/// verdict reaches `verified`. Public because "let loom run it" is the correct
/// move for every caller, not just the CLI: `absorb` binds observed runs, and a
/// test fixture that wants a proven graph should get one the same way a
/// production graph does, rather than through a seam that fabricates the record.
pub fn observe_validation(
    store: &Store,
    val: &crate::model::Node,
) -> Result<crate::proof::ProofOutcome> {
    use crate::proof::ProofOutcome;
    let ty = val
        .body
        .get("type")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<crate::model::ValidationType>().ok())
        .unwrap_or(crate::model::ValidationType::Test);
    let outcome = crate::proof::runner_for(ty).run(store.root(), val);
    match &outcome {
        ProofOutcome::Passed { evidence, run } => {
            mark_validation(
                store,
                &val.id,
                "passed",
                evidence,
                "",
                Some((**run).clone()),
            )?;
        }
        ProofOutcome::Failed { evidence, run, .. } => {
            mark_validation(
                store,
                &val.id,
                "failed",
                evidence,
                "",
                Some((**run).clone()),
            )?;
        }
        ProofOutcome::Blocked { reason } => {
            mark_validation(store, &val.id, "blocked", "", reason, None)?;
        }
        // No runner applies. loom records nothing rather than guessing — a
        // manual check is attested by a human, never inferred.
        ProofOutcome::Manual { .. } => {}
    }
    // Running a proof changes the inputs to its own grade, so re-grade it here
    // rather than leaving a stale figure until the next sync. Without this,
    // `loom validation run` followed by any command that reads strength would
    // report the grade from BEFORE the run.
    regrade(store, &val.id)?;
    Ok(outcome)
}

/// Recompute one validation's derived grade in place.
///
/// Must be called by EVERY path that settles a proof's outcome. It was called
/// only from `observe_validation`, which the `loom validation run` CLI bypasses
/// (see the dispatch at `ValidationCmd::Run`), so the documented way to run a
/// proof left the grade at whatever it was before the run. That is not a
/// cosmetic staleness: `sync` grades a reset validation S0, the run then passes
/// it, and the S0 stands — this session watched `proven` report 19 unproven
/// intents with all 189 proofs green, and a bare `loom sync` fix 26 grades at
/// once. Grade where the status is written, or the two drift.
fn regrade(store: &Store, validation_id: &str) -> Result<()> {
    let Some(val) = store.get_node(validation_id)? else {
        return Ok(());
    };
    let graph = crate::callgraph::build(store)?;
    let root = store.root().to_path_buf();
    let mut best: Option<crate::proofstrength::StrengthWitness> = None;
    for e in store.edges_with(Some(EdgeKind::Validates), Some(validation_id), None)? {
        let w = crate::proofstrength::grade(store, &root, &val, &e.to_id, &graph)?;
        let better = best
            .as_ref()
            .map(|b| {
                crate::proofstrength::Strength::parse(&w.grade)
                    > crate::proofstrength::Strength::parse(&b.grade)
            })
            .unwrap_or(true);
        if better {
            best = Some(w);
        }
    }
    if let Some(witness) = best {
        store.set_facet(
            validation_id,
            crate::model::TargetKind::Node,
            "proof_strength",
            &serde_json::to_string(&witness)?,
            crate::model::TruthClass::Derived,
        )?;
    }
    Ok(())
}

/// Register a command-shaped proof for an intent and run it. One call for the
/// common case: "this behavior is proven, and here is loom watching it be so."
pub fn prove_intent(store: &Store, intent_id: &str, name: &str, command: &str) -> Result<()> {
    let val = store.add_node(
        NodeType::Validation,
        name,
        "",
        "not_run",
        serde_json::json!({ "type": "test", "command": command }),
    )?;
    store.ensure_edge(EdgeKind::Validates, &val.id, intent_id)?;
    observe_validation(store, &val)?;
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

    // Serialize proof EXECUTION (not graph writes): a second runner would
    // share ports/processes with this one and mint false failing verdicts.
    let _harness = crate::harness::acquire(&root, "validation run")?;

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
                regrade(&store, &v.id)?;
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
                regrade(&store, &v.id)?;
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
                regrade(&store, &v.id)?;
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

/// Observe a command and return what loom saw. The shared core: the CLI prints
/// it, the MCP tool returns it, and neither can report an outcome loom did not
/// witness because neither is given the chance to supply one.
pub(crate) fn observe_run(
    graph: Option<&Path>,
    target: Option<&str>,
    timeout: u64,
    command: &[String],
) -> Result<serde_json::Value> {
    // Re-quote every argument. Joining on spaces looks right and is wrong: it
    // hands `python3 -c "import sys; ..."` to the shell as several statements,
    // so the command loom "observed" is not the command the caller asked for.
    let command = command
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");

    // Resolve what we need, then CLOSE the graph before running anything.
    //
    // Holding the write lock across the child is fatal for the commands most
    // worth observing: loom's own journeys are `loom journey run …`, and a
    // child blocked on the lock its parent holds exits non-zero. That does not
    // merely fail — it records a FALSE FAILING verdict against a behavior that
    // passes, which is the one outcome this whole spine exists to prevent.
    let (intent, covered, root) = {
        let store = open(graph)?;
        let root = store.root().to_path_buf();
        // What this run covers: the files the target behavior is grounded in,
        // so an edit to any of them expires it. With no target, the run covers
        // nothing and stands only as a journal record — honest about being
        // unattached.
        match target {
            Some(key) => {
                let node = store.resolve_node(key, Some(NodeType::Intent))?;
                let files = store.files_grounding(&node.id)?;
                (Some(node), files, root)
            }
            None => (None, Vec::new(), root),
        }
    };

    let _harness = crate::harness::acquire(&root, "observe")?;
    let observation = crate::runner::observe_command(
        &root,
        crate::model::RunProducer::Command,
        &command,
        &covered,
        0,
        timeout,
    )?;
    let run = match &observation {
        crate::runner::Observation::Ran(run) => (**run).clone(),
        crate::runner::Observation::Blocked { reason } => {
            // Keep the store open through the journal append so the graph lock
            // is held while the blocked proof is recorded; the binding is
            // intentionally unused beyond its drop.
            let _store = open(graph)?;
            // A command loom could not run is not a failing proof. Recorded as
            // blocked, visible, never green.
            crate::journal::append(
                &root,
                "observe",
                intent.as_ref().map(|n| n.id.as_str()).unwrap_or(""),
                serde_json::json!({ "command": command, "blocked": reason }),
            )?;
            return Ok(serde_json::json!({ "observed": false, "blocked": reason }));
        }
    };

    // The child is done; take the lock back to record what happened.
    let store = open(graph)?;
    let entry = crate::journal::append(
        &root,
        "observe",
        intent.as_ref().map(|n| n.id.as_str()).unwrap_or(""),
        serde_json::json!({
            "command": command,
            "exit_code": run.exit_code,
            "covered": run.covered.len(),
        }),
    )?;

    // Bind it, when there is something to bind it to.
    let mut bound: Option<String> = None;
    let mut bound_id: Option<String> = None;
    if let Some(node) = &intent {
        let validation = existing_or_new_proof(&store, &node.id, &command)?;
        let result = if run.exit_code == 0 {
            "passed"
        } else {
            "failed"
        };
        mark_validation(
            &store,
            &validation.id,
            result,
            &format!("observed by loom: `{command}` exited {}", run.exit_code),
            "",
            Some(run.clone()),
        )?;
        regrade(&store, &validation.id)?;
        bound = Some(validation.name.clone());
        bound_id = Some(validation.id.clone());
    }

    // Read the grade off the proof this run actually bound to. Looking it up by
    // the name loom WOULD have minted reports S0 for every run that reused an
    // existing proof — which is most of them, since the proof is keyed on the
    // command precisely so repeat runs land on one node.
    let grade = match &bound_id {
        Some(id) => crate::proofstrength::of(&store, id)?.as_str(),
        None => "-",
    };

    Ok(serde_json::json!({
        "observed": true,
        "command": command,
        "exit_code": run.exit_code,
        "covered": run.covered.keys().collect::<Vec<_>>(),
        "journal": entry.id,
        "bound_to": bound,
        "strength": grade,
    }))
}

/// Quote one argument for `sh -c`, so what runs is what was typed.
fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_alphanumeric() || "._-/=:@+,".contains(c))
    {
        return arg.to_string();
    }
    // Single quotes protect everything except a single quote, which has to be
    // closed, escaped, and reopened.
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The stable name loom gives a proof it minted from an observed command.
///
/// Short and stable: the full command lives in `body.command`, and a node name
/// that is 200 characters of shell is unreadable everywhere it appears.
fn command_proof_name(command: &str) -> String {
    let head: String = command
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    let head: String = head.chars().take(48).collect();
    format!(
        "observed: {head} [{}]",
        &crate::artifact::fingerprint(command)[..8]
    )
}

/// The validation this command already proves, or a new one for it.
///
/// Keyed on the COMMAND, so running the same command twice updates one proof
/// instead of littering the graph with near-duplicates.
fn existing_or_new_proof(
    store: &Store,
    intent_id: &str,
    command: &str,
) -> Result<crate::model::Node> {
    for e in store.edges_with(Some(EdgeKind::Validates), None, Some(intent_id))? {
        if let Some(v) = store.get_node(&e.from_id)? {
            if v.body.get("command").and_then(|c| c.as_str()) == Some(command) {
                return Ok(v);
            }
        }
    }
    let val = store.add_node(
        NodeType::Validation,
        &command_proof_name(command),
        "registered by `loom observe`",
        "not_run",
        serde_json::json!({ "type": "test", "command": command }),
    )?;
    store.ensure_edge(EdgeKind::Validates, &val.id, intent_id)?;
    Ok(val)
}
