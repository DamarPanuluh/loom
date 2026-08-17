fn intent_ratify(graph: Option<&Path>, args: RatifyArgs, json: bool) -> Result<()> {
    let store = open(graph)?;
    let evidence = args.evidence.ok_or_else(|| {
        anyhow::anyhow!("--evidence is required: say why this behavior is wanted")
    })?;
    let targets: Vec<Node> = match (&args.key, args.all) {
        (Some(_), true) => bail!("pass a key or --all, not both"),
        (None, false) => bail!("pass an intent key, or --all to ratify every unratified intent"),
        (Some(k), false) => vec![store.resolve_node(k, Some(NodeType::Intent))?],
        (None, true) => {
            let mut v = Vec::new();
            for n in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
                if n.status == "deprecated" {
                    continue;
                }
                if !is_ratified(&store, &n.id)? {
                    v.push(n);
                }
            }
            v
        }
    };
    if targets.is_empty() {
        pulse::emit_line(
            &store,
            json,
            serde_json::json!({ "ratified": [] }),
            "loom status",
            "nothing to ratify — every active intent is already ratified",
        )?;
        return Ok(());
    }
    // ONE human decision authorizes this invocation, whether direct or host
    // mediated, not one prompt per intent. Asking 51 times is not 51 times the
    // assurance — it is how a worker facing 51 prompts ends up forging the
    // records instead, which is exactly what happened to 39 of this graph's own
    // ratifications.
    //
    let subject = match targets.as_slice() {
        [one] => one.name.clone(),
        many => format!("ratify {}", many.len()),
    };
    let decision = super::ratification_decision(&subject, args.human_decision)?;
    let batch_id = if targets.len() > 1 {
        let subjects: Vec<String> = targets.iter().map(|n| n.id.clone()).collect();
        let executor = store.execution_identity().actor();
        // Contemporaneous set record before the per-intent writes.
        let digest = crate::batch_auth::subject_digest(&subjects);
        let pre = store.append_journal(
            "batch_intent",
            &digest,
            serde_json::json!({
                "operation": "ratify",
                "subjects": subjects,
                "human_decision": decision,
                "evidence": evidence,
            }),
        )?;
        let now = crate::journal::now_iso();
        let envelope = crate::batch_auth::BatchAuthorization::seal(
            crate::batch_auth::BatchClaim::Ratification,
            "ratify",
            subjects,
            "human",
            &executor,
            &evidence,
            vec![format!("journal:{}", pre.id)],
        )?
        .with_command_id(format!("intent-ratify-all:{}", targets.len()))
        .with_time_bounds(&now, &now)
        .with_human_decision(decision.clone());
        let entry = crate::batch_auth::append_envelope(&store, &envelope)?;
        Some(entry.id)
    } else {
        None
    };
    let mut ratified = Vec::new();
    for n in &targets {
        match &batch_id {
            Some(bid) => store.ratify_intent_from_human_batch(&n.id, &evidence, &decision, bid)?,
            None => store.ratify_intent_from_human(&n.id, &evidence, &decision)?,
        }
        ratified.push(serde_json::json!({ "id": n.id, "name": n.name }));
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({ "ratified": ratified, "evidence": evidence }),
        "loom status",
        if targets.len() == 1 {
            format!("ratified '{}'", targets[0].name)
        } else {
            format!("ratified {} intent(s)", targets.len())
        },
    )?;
    Ok(())
}

/// Say a behavior is not wanted.
///
/// Deliberately cheaper than ratifying: presence is required, but no typed
/// challenge. Writing a substantive reason IS the deliberate act, and making
/// refusal expensive is how you get a graph nobody ever refuses anything in.
///
/// A rejection is not a delete. Every place the code still performs the
/// behavior becomes a finding, so removing it enters triage as ordinary work
/// with the evidence already attached — and until it is gone, the intent is a
/// `ZombieBehavior` the ladder blocks on.
fn intent_reject(
    graph: Option<&Path>,
    key: &str,
    reason: &str,
    human_decision: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    if crate::model::is_placeholder(reason) {
        bail!("--reason must say why this is not wanted, substantively");
    }
    let decision = match human_decision {
        Some(response) => super::mediated_decision(response)?,
        None if super::human_present() => crate::ratification::HumanDecision::direct("tty")?,
        None => bail!(
            "INV-8: only a human may judge whether a behavior is wanted — ask the human, then pass their exact answer with --human-decision"
        ),
    };
    let intent = store.resolve_node(key, Some(NodeType::Intent))?;
    let minted = reject_intent_core(&store, &intent, reason, &decision)?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "rejected": { "id": intent.id, "name": intent.name },
            "reason": reason,
            "removal_work": minted,
        }),
        "loom next --mode triage",
        format!(
            "rejected '{}' — {} place(s) still perform it",
            intent.name,
            minted.len()
        ),
    )?;
    Ok(())
}

fn intent_add(graph: Option<&Path>, args: IntentAddArgs, json: bool) -> Result<()> {
    let store = open(graph)?;
    let node = create_intent(&store, &args)?;
    let visibility = store.get_facet(&node.id, TargetKind::Node, "visibility")?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&node),
            "level": args.level,
            "visibility": visibility,
            "layer": args.layer,
            "aspect": args.aspect,
            "allow_symbol_name": args.allow_symbol_name,
        }),
        "loom status",
        format!(
            "added intent '{}' [{}]",
            node.name,
            crate::model::short(&node.id)
        ),
    )?;
    Ok(())
}

fn intent_show(graph: Option<&Path>, key: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    let level = store.get_facet(&n.id, TargetKind::Node, "level")?;
    let visibility = store.get_facet(&n.id, TargetKind::Node, "visibility")?;
    let layer = store.get_facet(&n.id, TargetKind::Node, "layer")?;
    let aspect = store.get_facet(&n.id, TargetKind::Node, "aspect")?;
    let origin = store.get_facet(&n.id, TargetKind::Node, "origin")?;
    let ratification = store
        .ratification(&n.id)
        .map(Some)?
        .unwrap_or_else(|| "unratified".into());
    let ratified_by = store.get_facet(&n.id, TargetKind::Node, "ratified_by")?;
    let ratified_at = store.get_facet(&n.id, TargetKind::Node, "ratified_at")?;
    let journey_exemption = store
        .get_facet(&n.id, TargetKind::Node, "journey_exemption")?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    let tags = store.tags_of(&n.id, TargetKind::Node)?;

    if json {
        let mut intent = node_json(&n);
        intent["level"] = serde_json::json!(level);
        intent["visibility"] = serde_json::json!(visibility);
        intent["layer"] = serde_json::json!(layer);
        intent["aspect"] = serde_json::json!(aspect);
        intent["origin"] = serde_json::json!(origin);
        intent["ratification"] = serde_json::json!(ratification);
        intent["ratified_by"] = serde_json::json!(ratified_by);
        intent["ratified_at"] = serde_json::json!(ratified_at);
        intent["journey_exemption"] = serde_json::json!(journey_exemption);
        intent["tags"] = serde_json::json!(tags);
        println!("{}", serde_json::to_string_pretty(&intent)?);
        return Ok(());
    }

    println!("{} [{}]", n.name, n.id);
    println!("  lifecycle: {}", n.status);
    if !n.description.is_empty() {
        println!("  description: {}", n.description);
    }
    if let Some(level) = level {
        println!("  level: {level}");
    }
    if let Some(vis) = visibility {
        println!("  visibility: {vis}");
    }
    if let Some(layer) = layer {
        println!("  layer: {layer}");
    }
    if let Some(aspect) = aspect {
        println!("  aspect: {aspect}");
    }
    println!("  origin: {}", origin.unwrap_or_else(|| "unknown".into()));
    println!("  ratification: {ratification}");
    if let Some(by) = ratified_by {
        println!("  ratified_by: {by}");
    }
    if let Some(at) = ratified_at {
        println!("  ratified_at: {at}");
    }
    if let Some(exemption) = journey_exemption {
        println!(
            "  journey_exemption: {}",
            serde_json::to_string(&exemption)?
        );
    }
    if !tags.is_empty() {
        println!("  tags: {}", tags.join(", "));
    }
    Ok(())
}

/// Deliberately close a completeness axis for this intent. The waiver is an
/// asserted facet (`waiver:<axis>` = reason) plus a decision note, and it
/// re-opens automatically when the intent is redefined — a waiver outliving
/// the meaning it waived would be a silent lie.
fn intent_waive(
    graph: Option<&Path>,
    key: String,
    axis: String,
    reason: String,
    json: bool,
) -> Result<()> {
    crate::completeness::check_axis(&axis)?;
    if axis == "questions" {
        bail!(
            "the questions axis is never waivable: answer the question or withdraw it \
             (loom inbox mark <id> rejected --reason '…')"
        );
    }
    if reason.trim().is_empty() {
        bail!("a waiver needs a substantive --reason");
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    store.set_facet(
        &n.id,
        TargetKind::Node,
        &format!("waiver:{axis}"),
        &reason,
        TruthClass::Asserted,
    )?;
    store.add_note(&n.id, "decision", &format!("waived {axis}: {reason}"))?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "waived_axis": axis,
            "reason": reason,
        }),
        "loom status",
        format!("waived {axis} for '{}'", n.name),
    )?;
    Ok(())
}

/// Record the one canonical exception to Journey ancestry. The facet carries
/// only the decision's digest; the full human answer remains in the append-only
/// journal so ordinary graph reads do not expose or duplicate authority text.
fn intent_journey_exempt(
    graph: Option<&Path>,
    key: String,
    kind: String,
    reason: String,
    human_decision: Option<String>,
    json: bool,
) -> Result<()> {
    if crate::model::is_placeholder(&kind) {
        bail!("journey exemption needs a substantive --kind");
    }
    if crate::model::is_placeholder(&reason) {
        bail!("journey exemption needs a substantive --reason");
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    let decision =
        super::ratification_decision(&format!("journey-exempt {}", n.name), human_decision)?;
    let decision_json = serde_json::to_string(&decision)?;
    let decision_digest = crate::artifact::fingerprint(&decision_json);
    // serde_json's default map is key-sorted; serializing this object therefore
    // produces the exact canonical representation completeness accepts.
    let exemption = serde_json::json!({
        "kind": kind.trim(),
        "reason": reason.trim(),
        "human_decision_digest": decision_digest,
    });
    let canonical = serde_json::to_string(&exemption)?;
    let tx = store.begin()?;
    store.set_facet(
        &n.id,
        TargetKind::Node,
        "journey_exemption",
        &canonical,
        TruthClass::Asserted,
    )?;
    store.append_journal(
        "intent_journey_exempt",
        &n.id,
        serde_json::json!({
            "kind": kind.trim(),
            "reason": reason.trim(),
            "human_decision_digest": decision_digest,
            "human_decision": decision,
        }),
    )?;
    tx.commit()?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "journey_exemption": exemption,
        }),
        "loom status",
        format!("Journey exemption recorded for '{}'", n.name),
    )
}

/// Withdraw a Journey exemption under the same human-decision gate that
/// created it. The journal preserves who authorized the semantic reversal.
fn intent_journey_require(
    graph: Option<&Path>,
    key: String,
    reason: String,
    human_decision: Option<String>,
    json: bool,
) -> Result<()> {
    if crate::model::is_placeholder(&reason) {
        bail!("journey requirement needs a substantive --reason");
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    if store
        .get_facet(&n.id, TargetKind::Node, "journey_exemption")?
        .is_none()
    {
        bail!("intent '{}' has no Journey exemption to withdraw", n.name);
    }
    let decision =
        super::ratification_decision(&format!("journey-require {}", n.name), human_decision)?;
    let decision_json = serde_json::to_string(&decision)?;
    let decision_digest = crate::artifact::fingerprint(&decision_json);
    let tx = store.begin()?;
    store.clear_facet(&n.id, TargetKind::Node, "journey_exemption")?;
    store.append_journal(
        "intent_journey_require",
        &n.id,
        serde_json::json!({
            "reason": reason.trim(),
            "human_decision_digest": decision_digest,
            "human_decision": decision,
        }),
    )?;
    tx.commit()?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "journey_required": true,
            "reason": reason.trim(),
            "human_decision_digest": decision_digest,
        }),
        "loom next --mode derive",
        format!("Journey ancestry required again for '{}'", n.name),
    )
}

fn intent_reactivate(graph: Option<&Path>, key: String, reason: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    if n.status != "deprecated" {
        bail!(
            "intent '{}' is not retired (status: {}) — nothing to reactivate",
            n.name,
            n.status
        );
    }
    // loom-stability-exempt: reactivates a retired intent
    store.set_node_status(&n.id, "planned")?;
    store.add_note(&n.id, "transition", &format!("reactivated: {reason}"))?;
    let mut intent = node_json(&n);
    intent["status"] = serde_json::json!("planned");
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": intent,
            "reason": reason,
        }),
        "loom status",
        format!("reactivated intent '{}' → planned", n.name),
    )?;
    Ok(())
}

fn intent_list(graph: Option<&Path>, limit: usize, offset: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intents = store.list_nodes_page(Some(NodeType::Intent), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::Intent))?;
    if json {
        let rows: Vec<_> = intents
            .iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id,
                    "name": n.name,
                    "status": n.status,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&super::pagination_envelope(&rows, offset, limit, total))?
        );
        return Ok(());
    }
    if intents.is_empty() && offset == 0 {
        println!("no intents");
    }
    for n in &intents {
        println!(
            "{:<12} {} [{}]",
            n.status,
            n.name,
            crate::model::short(&n.id)
        );
    }
    if let Some(footer) = super::page_footer(intents.len(), offset, total) {
        println!("{footer}");
    }
    Ok(())
}
