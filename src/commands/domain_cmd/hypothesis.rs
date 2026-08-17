use super::*;

pub(crate) fn hypothesis(graph: Option<&Path>, cmd: HypothesisCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        HypothesisCmd::Add {
            name,
            claim,
            proposal,
            predicted_outcome,
            target,
        } => hypothesis_add(
            &store,
            json,
            HypothesisAddArgs {
                name,
                claim,
                proposal,
                predicted_outcome,
                target,
            },
        ),
        HypothesisCmd::Prove {
            key,
            outcome,
            evidence,
        } => hypothesis_prove(&store, json, key, outcome, evidence),
        HypothesisCmd::Adopt { key, spawned } => hypothesis_adopt(&store, json, key, spawned),
        HypothesisCmd::Reject { key, reason } => hypothesis_reject(&store, json, key, reason),
        HypothesisCmd::Update {
            key,
            claim,
            proposal,
            predicted_outcome,
            reason,
        } => hypothesis_update(
            &store,
            json,
            HypothesisUpdateArgs {
                key,
                claim,
                proposal,
                predicted_outcome,
                reason,
            },
        ),
        HypothesisCmd::Remove { key } => hypothesis_remove(&store, json, key),
        HypothesisCmd::Show { key } => hypothesis_show(&store, json, key),
        HypothesisCmd::List { limit, offset } => hypothesis_list(&store, json, limit, offset),
    }
}

struct HypothesisAddArgs {
    name: String,
    claim: String,
    proposal: String,
    predicted_outcome: String,
    target: String,
}

fn hypothesis_add(store: &Store, json: bool, args: HypothesisAddArgs) -> Result<()> {
    let t = store.resolve_node(&args.target, Some(NodeType::Intent))?;
    let h = store.add_node(
        NodeType::Hypothesis,
        &args.name,
        &args.claim,
        "proposed",
        serde_json::json!({
            "proposal": args.proposal,
            "predicted_outcome": args.predicted_outcome
        }),
    )?;
    let edge = store.ensure_edge(EdgeKind::Targets, &h.id, &t.id)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "hypothesis": node_json(&h),
            "target": node_json(&t),
            "edge": edge,
        }),
        "loom status",
        format!("hypothesis '{}' targets '{}'", h.name, t.name),
    )?;
    Ok(())
}

fn hypothesis_prove(
    store: &Store,
    json: bool,
    key: String,
    outcome: String,
    evidence: String,
) -> Result<()> {
    let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
    let status = match outcome.as_str() {
        "supported" => "supported",
        "refuted" => "refuted",
        other => bail!("unknown outcome '{other}' (use supported|refuted)"),
    };
    if crate::model::is_placeholder(&evidence) {
        bail!("{status} verdict requires substantive evidence (not a placeholder like '…' or '<reason>')");
    }
    // loom-stability-exempt: moves a hypothesis through its lifecycle
    store.set_node_status(&h.id, status)?;
    store.add_note(&h.id, "decision", &format!("{status}: {evidence}"))?;
    // Teach the follow-through where it is most needed: a supported
    // claim is not work until adopted (nothing re-queues it), so point
    // straight at adoption; a refuted claim stands as an honest record.
    let next_step = if status == "supported" {
        format!(
                    "loom hypothesis adopt {} — promotes the proven idea to build work (optionally add --spawned '<behavioral intent name>' to rename the spawned intent)",
                    crate::workitem::q(&h.name)
                )
    } else {
        "loom status  (the refuted claim stands as an honest record — no adoption)".to_string()
    };
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "hypothesis": {
                "id": h.id,
                "name": h.name,
                "status": status,
            },
            "evidence": evidence,
        }),
        &next_step,
        format!("hypothesis '{}' {status}", h.name),
    )?;
    Ok(())
}

fn hypothesis_adopt(store: &Store, json: bool, key: String, spawned: Option<String>) -> Result<()> {
    let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
    if h.status != "supported" {
        bail!(
            "only a supported hypothesis can be adopted (current: {})",
            h.status
        );
    }
    let name = spawned.unwrap_or_else(|| format!("{} (adopted)", h.name));
    if name.trim().is_empty() {
        bail!("adopted intent name must be non-empty");
    }
    if looks_like_symbol(&name) && h.description.trim().is_empty() {
        bail!(
            "intent name '{name}' looks like a code symbol. Hypothesis adoption \
                     requires a non-empty hypothesis description for symbol-like intent names."
        );
    }
    // The experiment record must survive the handoff: the spawned
    // intent's build packet reaches notes, not the hypothesis body,
    // so copy proposal/prediction/evidence onto the intent itself.
    let proposal = h
        .body
        .get("proposal")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let predicted = h
        .body
        .get("predicted_outcome")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let evidence = store
        .notes_for(&h.id)?
        .into_iter()
        .find_map(|n| {
            (n.status == "decision")
                .then_some(n.description)
                .and_then(|d| d.strip_prefix("supported: ").map(str::to_string))
        })
        .unwrap_or_else(|| "(proof evidence unavailable)".into());
    // loom-stability-exempt: adopts a supported hypothesis
    store.set_node_status(&h.id, "adopted")?;
    let intent = store.add_node(
        NodeType::Intent,
        &name,
        &h.description,
        "planned",
        serde_json::json!({ "level": "feature" }),
    )?;
    store.set_facet(
        &intent.id,
        TargetKind::Node,
        "visibility",
        "internal",
        TruthClass::Asserted,
    )?;
    store.add_note(
                &intent.id,
                "decision",
                &format!(
                    "adopted from hypothesis '{}' [{}] — proposal: {proposal}; predicted: {predicted}; evidence: {evidence}",
                    h.name,
                    crate::model::short(&h.id)
                ),
            )?;
    store.add_note(
        &h.id,
        "decision",
        &format!("adopted → spawned intent {}", intent.id),
    )?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "hypothesis": {
                "id": h.id,
                "name": h.name,
                "status": "adopted",
            },
            "spawned_intent": node_json(&intent),
        }),
        "loom status",
        format!("adopted '{}' → planned intent '{}'", h.name, intent.name),
    )?;
    Ok(())
}

fn hypothesis_reject(store: &Store, json: bool, key: String, reason: String) -> Result<()> {
    let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
    // loom-stability-exempt: rejects a hypothesis
    store.set_node_status(&h.id, "rejected")?;
    store.add_note(&h.id, "decision", &format!("rejected: {reason}"))?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "hypothesis": {
                "id": h.id,
                "name": h.name,
                "status": "rejected",
            },
            "reason": reason,
        }),
        "loom status",
        format!("rejected '{}'", h.name),
    )?;
    Ok(())
}

struct HypothesisUpdateArgs {
    key: String,
    claim: Option<String>,
    proposal: Option<String>,
    predicted_outcome: Option<String>,
    reason: String,
}

fn hypothesis_update(store: &Store, json: bool, args: HypothesisUpdateArgs) -> Result<()> {
    if args.reason.trim().is_empty() {
        bail!("hypothesis update needs substantive --reason");
    }
    if args.claim.is_none() && args.proposal.is_none() && args.predicted_outcome.is_none() {
        bail!("nothing to update — pass --claim, --proposal, and/or --predicted-outcome");
    }
    let h = store.resolve_node(&args.key, Some(NodeType::Hypothesis))?;
    if h.status != "proposed" {
        bail!(
                    "only proposed hypotheses can be updated (current: {}); proven/adopted/rejected hypotheses are history",
                    h.status
                );
    }
    let mut body = h.body.clone();
    if let Some(v) = &args.claim {
        body["claim"] = serde_json::json!(v);
        store.update_node(&h.id, None, Some(v), None)?;
    }
    if let Some(v) = &args.proposal {
        body["proposal"] = serde_json::json!(v);
    }
    if let Some(v) = &args.predicted_outcome {
        body["predicted_outcome"] = serde_json::json!(v);
    }
    store.set_node_body(&h.id, &body)?;
    store.add_note(
        &h.id,
        "decision",
        &format!("refined hypothesis: {}", args.reason),
    )?;
    let display_claim = args.claim.as_deref().unwrap_or(&h.description);
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "hypothesis": {
                "id": h.id,
                "name": h.name,
                "status": h.status,
                "claim": display_claim,
                "body": body,
            },
            "reason": args.reason,
        }),
        "loom status",
        format!("updated proposed hypothesis '{}'", h.name),
    )?;
    Ok(())
}

fn hypothesis_remove(store: &Store, json: bool, key: String) -> Result<()> {
    let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
    store.delete_node(&h.id)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "removed": true,
            "hypothesis": node_json(&h),
        }),
        "loom status",
        format!("removed mistaken hypothesis '{}'", h.name),
    )?;
    Ok(())
}

fn hypothesis_show(store: &Store, json: bool, key: String) -> Result<()> {
    let h = store.resolve_node(&key, Some(NodeType::Hypothesis))?;
    let targets: Vec<_> = store
        .edges_with(Some(EdgeKind::Targets), Some(&h.id), None)?
        .into_iter()
        .map(|e| {
            let name = store
                .get_node(&e.to_id)?
                .map(|n| n.name)
                .unwrap_or_else(|| e.to_id.clone());
            Ok(serde_json::json!({
                "id": e.to_id,
                "name": name,
                "edge_id": e.id,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    if json {
        let mut row = node_json(&h);
        row["targets"] = serde_json::json!(targets);
        println!("{}", serde_json::to_string_pretty(&row)?);
    } else {
        println!("{} [{}]", h.name, h.id);
        println!("  status: {}", h.status);
        if !h.description.is_empty() {
            println!("  claim: {}", h.description);
        }
        println!("  {}", h.body);
        for t in targets {
            if let Some(name) = t.get("name").and_then(|v| v.as_str()) {
                println!("  targets: {name}");
            }
        }
    }
    Ok(())
}

fn hypothesis_list(store: &Store, json: bool, limit: usize, offset: usize) -> Result<()> {
    let hypotheses = store.list_nodes_page(Some(NodeType::Hypothesis), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::Hypothesis))?;
    if json {
        let rows: Vec<_> = hypotheses.iter().map(node_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&pagination_envelope(&rows, offset, limit, total))?
        );
    } else {
        let shown = hypotheses.len();
        for n in hypotheses {
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
