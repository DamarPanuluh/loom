use super::*;

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
        } => validation_add(&store, json, name, r#type, command, intent),
        ValidationCmd::Verdict {
            key,
            outcome,
            evidence,
            reason,
        } => validation_verdict(&store, json, key, outcome, evidence, reason),
        ValidationCmd::Show { key } => validation_show(&store, json, key),
        ValidationCmd::Update {
            key,
            r#type,
            command,
        } => validation_update(&store, json, key, r#type, command),
        ValidationCmd::Unlink { validation, intent } => {
            validation_unlink(&store, json, validation, intent)
        }
        ValidationCmd::Remove { key } => validation_remove(&store, json, key),
        ValidationCmd::List { limit, offset } => validation_list(&store, json, limit, offset),
        // Intercepted before the store is opened (validate_cmd owns its lock).
        ValidationCmd::Run { .. } => unreachable!("`validation run` is handled above"),
    }
}

fn validation_add(
    store: &Store,
    json: bool,
    name: String,
    r#type: String,
    command: String,
    intent: String,
) -> Result<()> {
    // Enforce the validation-type vocabulary (M-15/I-5): the CLI advertises
    // a finite set, so reject a typo instead of storing an arbitrary string.
    let vtype = match r#type.parse::<crate::model::ValidationType>() {
        Ok(t) => t,
        Err(_) => bail!(
            "unknown validation type '{}' (use test|assertion|benchmark|manual_check|journey|scenario|contract)",
            r#type
        ),
    };
    if matches!(vtype, crate::model::ValidationType::Journey) {
        bail!("Journey validations are compiler-owned; use `loom journey compile <journey> --profile <profile>` instead of validation add");
    }
    let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
    let collision = warn_if_command_already_proves_another(store, &command, &i.id, None)?;
    // Registration is one fact: a Validation without its Validates
    // edge proves nothing and becomes an orphan that blocks a clean
    // retry. Fail the lane gate before the first mutation, then keep
    // node + edge in one transaction so any later edge error rolls the
    // node back as well.
    store.require_edge_kind_owner(EdgeKind::Validates)?;
    let body = serde_json::json!({ "type": vtype.as_str(), "command": command });
    let tx = store.begin()?;
    let val = store.add_node(NodeType::Validation, &name, "", "not_run", body)?;
    let edge = store.ensure_edge(EdgeKind::Validates, &val.id, &i.id)?;
    tx.commit()?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "validation": node_json(&val),
            "intent": node_json(&i),
            "edge": edge,
            "collision": collision,
        }),
        "loom status",
        format!("added validation '{}' → '{}'", val.name, i.name),
    )
}

fn validation_verdict(
    store: &Store,
    json: bool,
    key: String,
    outcome: String,
    evidence: String,
    reason: String,
) -> Result<()> {
    let val = store.resolve_node(&key, Some(NodeType::Validation))?;
    if let Some((journey, profile)) =
        crate::completeness::compiler_owned_journey_validation(store, &val)?
    {
        bail!(
            "compiler-owned Journey validations cannot receive manual verdicts; use `loom journey run {} --profile {}`",
            journey.id,
            profile
        );
    }
    let validation_type = val
        .body
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if validation_type == "journey" {
        bail!("Journey validations cannot receive manual verdicts; remove an orphaned proof or use `loom journey run <journey> --profile <profile>`");
    }
    let has_command = val
        .body
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|c| !c.trim().is_empty());
    if validation_type != "manual_check" && !has_command {
        bail!(
            "non-manual validation '{}' has no runnable command and cannot receive a manual verdict",
            val.name
        );
    }
    mark_validation(store, &val.id, &outcome, &evidence, &reason, None)?;
    regrade(store, &val.id)?;
    pulse::emit_line(
        store,
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
    )
}

fn validation_show(store: &Store, json: bool, key: String) -> Result<()> {
    let val = store.resolve_node(&key, Some(NodeType::Validation))?;
    let validates = validation_targets(store, &val.id)?;
    // The grade, with every conjunct that produced it. A number nobody
    // can argue with is a number nobody can act on.
    let witness: Option<crate::proofstrength::StrengthWitness> =
        match store.get_facet(&val.id, crate::model::TargetKind::Node, "proof_strength")? {
            Some(j) => Some(serde_json::from_str(&j).with_context(|| {
                format!(
                    "proof_strength facet on '{}' is malformed — run `loom sync` to regrade",
                    val.name
                )
            })?),
            None => None,
        };
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
        return Ok(());
    }
    println!("{} [{}]", val.name, val.id);
    println!("  status: {}", val.status);
    println!("  {}", val.body);
    if let Some(w) = &witness {
        print_strength_witness(w);
    }
    for i in validates {
        println!("  validates: {}", i["name"].as_str().unwrap_or(""));
    }
    Ok(())
}

fn print_strength_witness(w: &crate::proofstrength::StrengthWitness) {
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
    if let Some(evidence) = &w.call_evidence {
        let mut detail = format!(
            "{} {}{}",
            evidence.source,
            evidence.file,
            evidence
                .entry_symbol
                .as_deref()
                .map(|symbol| format!("::{symbol}"))
                .unwrap_or_default(),
        );
        if let (Some(operation), Some(exercise), Some(observed)) = (
            evidence.operation_id.as_deref(),
            evidence.exercise_id.as_deref(),
            evidence.observed_by.as_deref(),
        ) {
            detail.push_str(&format!(
                " via operation '{operation}' exercise '{exercise}' observed_by '{observed}'"
            ));
        }
        if !evidence.s3_eligible {
            detail.push_str(" (not S3-eligible)");
        }
        println!("    call evidence: {detail}");
    }
    if !w.next.is_empty() {
        println!("    next: {}", w.next);
    }
}

fn validation_update(
    store: &Store,
    json: bool,
    key: String,
    r#type: Option<String>,
    command: Option<String>,
) -> Result<()> {
    let val = store.resolve_node(&key, Some(NodeType::Validation))?;
    if let Some((journey, profile)) =
        crate::completeness::compiler_owned_journey_validation(store, &val)?
    {
        bail!(
            "compiler-owned Journey validations cannot be updated generically; use `loom journey compile {} --profile {}`",
            journey.id,
            profile
        );
    }
    let mut body = val.body.clone();
    let mut collision = ProofCommandCollision::none("");
    if let Some(t) = &r#type {
        body["type"] = serde_json::json!(t);
    }
    if let Some(c) = &command {
        collision = warn_if_command_already_proves_another(store, c, "", Some(&val.id))?;
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
            store.reset_validation_status_for_sync(&val.id)?;
            for validates in store.edges_with(Some(EdgeKind::Validates), Some(&val.id), None)? {
                store.stale_edge(&validates.id, "validation command changed")?;
            }
        }
    }
    if r#type.as_deref() != val.body.get("type").and_then(|v| v.as_str()) && r#type.is_some() {
        store.reset_validation_status_for_sync(&val.id)?;
        for validates in store.edges_with(Some(EdgeKind::Validates), Some(&val.id), None)? {
            store.stale_edge(&validates.id, "validation type changed")?;
        }
    }
    store.set_node_body(&val.id, &body)?;
    let current = store
        .get_node(&val.id)?
        .ok_or_else(|| anyhow!("validation '{}' vanished mid-update", val.name))?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "validation": {
                "id": current.id,
                "name": current.name,
                "status": current.status,
                "body": body,
            },
            "collision": collision,
        }),
        "loom status",
        format!("updated validation '{}'", current.name),
    )
}

fn validation_unlink(store: &Store, json: bool, validation: String, intent: String) -> Result<()> {
    let v = store.resolve_node(&validation, Some(NodeType::Validation))?;
    if let Some((journey, profile)) =
        crate::completeness::compiler_owned_journey_validation(store, &v)?
    {
        bail!(
            "compiler-owned Journey validation topology cannot be unlinked generically; use `loom journey compile {} --profile {}`",
            journey.id,
            profile
        );
    }
    let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
    match store
        .edges_with(Some(EdgeKind::Validates), Some(&v.id), Some(&i.id))?
        .into_iter()
        .next()
    {
        Some(e) => {
            store.delete_edge(&e.id)?;
            pulse::emit_line(
                store,
                json,
                serde_json::json!({
                    "removed": true,
                    "edge": e,
                    "validation": node_json(&v),
                    "intent": node_json(&i),
                }),
                "loom status",
                format!("unlinked '{}' from '{}'", v.name, i.name),
            )
        }
        None => bail!("'{}' does not validate '{}'", v.name, i.name),
    }
}

fn validation_remove(store: &Store, json: bool, key: String) -> Result<()> {
    let val = store.resolve_node(&key, Some(NodeType::Validation))?;
    if let Some((journey, profile)) =
        crate::completeness::compiler_owned_journey_validation(store, &val)?
    {
        if !store.is_local_snapshot() {
            bail!(
                "compiler-owned Journey validations cannot be removed generically; recompile with `loom journey compile {} --profile {}`",
                journey.name,
                profile
            );
        }
    }
    store.delete_node(&val.id)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "removed": true,
            "validation": node_json(&val),
        }),
        "loom status",
        format!("removed validation '{}'", val.name),
    )
}

fn validation_list(store: &Store, json: bool, limit: usize, offset: usize) -> Result<()> {
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
            serde_json::to_string_pretty(&pagination_envelope(&rows, offset, limit, total))?
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
        if let Some(footer) = page_footer(shown, offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}
