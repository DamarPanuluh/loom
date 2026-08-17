fn intent_remove(graph: Option<&Path>, key: String, reason: String, json: bool) -> Result<()> {
    if reason.trim().is_empty() {
        bail!("intent remove needs substantive --reason");
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    let children = store.edges_with(Some(EdgeKind::Hierarchy), Some(&n.id), None)?;
    if !children.is_empty() {
        bail!(
            "intent '{}' has {} hierarchy child edge(s); retire it or re-parent/remove the children first",
            n.name,
            children.len()
        );
    }
    store.delete_node(&n.id)?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "removed": true,
            "intent": node_json(&n),
            "reason": reason,
        }),
        "loom status",
        format!("removed mistaken intent '{}'", n.name),
    )?;
    Ok(())
}

fn intent_retire(
    graph: Option<&Path>,
    key: String,
    reason: String,
    replaced_by: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    let rb = match replaced_by.as_deref() {
        Some(r) => Some(store.resolve_node(r, Some(NodeType::Intent))?.id),
        None => None,
    };
    store.retire_intent(&n.id, &reason, rb.as_deref())?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": {
                "id": n.id,
                "name": n.name,
                "status": "deprecated",
            },
            "reason": reason,
            "replaced_by": rb,
        }),
        "loom status",
        format!("retired '{}'", n.name),
    )?;
    Ok(())
}

fn intent_confirm(graph: Option<&Path>, key: String, json: bool) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    store.add_note(&n.id, "confirm", "meaning re-affirmed")?;
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "confirmed": true,
        }),
        "loom status",
        format!("confirmed '{}'", n.name),
    )?;
    Ok(())
}

/// Record the builder's post-change semantic assessment. This is deliberately
/// not ratification: a changed criterion only stales wantedness and hands the
/// decision back to the terminal-gated human ratify queue (INV-8).
fn intent_impact(
    graph: Option<&Path>,
    key: String,
    classification: String,
    evidence: String,
    json: bool,
) -> Result<()> {
    if !matches!(
        classification.as_str(),
        "preserved" | "changed_within_intent" | "criterion_changed"
    ) {
        bail!(
            "impact classification must be preserved, changed_within_intent, or criterion_changed"
        );
    }
    if crate::model::is_placeholder(&evidence) {
        bail!("intent impact requires substantive --evidence");
    }
    let store = open(graph)?;
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    store.set_facet(
        &n.id,
        TargetKind::Node,
        "semantic_impact",
        &classification,
        TruthClass::Asserted,
    )?;
    store.set_facet(
        &n.id,
        TargetKind::Node,
        "semantic_impact_evidence",
        &evidence,
        TruthClass::Asserted,
    )?;
    store.add_note(
        &n.id,
        "decision",
        &format!("semantic impact {classification}: {evidence}"),
    )?;
    let mut reconfirmation_required = false;
    if classification == "criterion_changed"
        && store.ratification(&n.id).map(Some)?.as_deref() == Some("ratified")
    {
        store.assert_fact(
            loom_assertion(&n.id, "needs_reconfirmation")
                .criterion("criterion changed since ratification")
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    evidence.trim().to_string(),
                )]),
        )?;
        store.add_note(
            &n.id,
            "ratify",
            "ratification staled by semantic impact assessment",
        )?;
        reconfirmation_required = true;
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "classification": classification,
            "evidence": evidence,
            "reconfirmation_required": reconfirmation_required,
        }),
        if reconfirmation_required {
            "loom next --mode ratify"
        } else {
            "loom status"
        },
        format!(
            "semantic impact for '{}' recorded as {classification}",
            n.name
        ),
    )?;
    Ok(())
}

/// Tag an intent with a registered vocab term, through the one gate the CLI
/// enforces — the term must already be in the vocab registry. Returns the
/// resolved intent. Shared by `loom intent tag add` and the `loom apply` tags
/// batch so the batch can never accept what the per-verb command rejects.
pub(crate) fn tag_intent(store: &Store, key: &str, term: &str) -> Result<crate::model::Node> {
    let n = store.resolve_node(key, Some(NodeType::Intent))?;
    if !store.vocab_has(term)? {
        bail!("'{term}' is not a registered vocab term; add it with `loom vocab add`");
    }
    store.set_tag(&n.id, TargetKind::Node, term)?;
    Ok(n)
}

fn intent_tag(graph: Option<&Path>, cmd: IntentTagCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        IntentTagCmd::Add { key, term } => {
            let n = tag_intent(&store, &key, &term)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "intent": node_json(&n),
                    "action": "add",
                    "term": term,
                }),
                "loom status",
                format!("tagged '{}' with '{term}'", n.name),
            )?;
        }
        IntentTagCmd::Remove { key, term } => {
            let n = store.resolve_node(&key, Some(NodeType::Intent))?;
            store.remove_tag(&n.id, TargetKind::Node, &term)?;
            pulse::emit_line(
                &store,
                json,
                serde_json::json!({
                    "intent": node_json(&n),
                    "action": "remove",
                    "term": term,
                }),
                "loom status",
                format!("untagged '{}' '{term}'", n.name),
            )?;
        }
    }
    Ok(())
}

/// Render what stands on a behavior.
///
/// The unproven ones are the point: a dependent with no passing proof is where
/// a change to the queried behavior would break something silently, so they are
/// called out rather than left for the reader to spot in a list.
fn intent_dependents(graph: Option<&Path>, key: &str, depth: usize, json: bool) -> Result<()> {
    let store = crate::commands::open_read(graph)?;
    let target = store.resolve_node(key, Some(NodeType::Intent))?;
    let found = store.dependents(&target.id, depth)?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "intent": { "id": target.id, "name": target.name },
                "depth": depth,
                "dependents": found,
                "unproven": found.iter().filter(|d| !d.proven).count(),
            }))?
        );
        return Ok(());
    }

    if found.is_empty() {
        println!(
            "nothing stands on '{}' within {depth} hop(s) — changing it reaches no other behavior",
            target.name
        );
        return Ok(());
    }
    println!("{} behavior(s) stand on '{}':", found.len(), target.name);
    for d in &found {
        println!(
            "  {:>2} hop{}  {:<9} {}",
            d.hops,
            if d.hops == 1 { " " } else { "s" },
            if d.proven { "proven" } else { "UNPROVEN" },
            d.intent.name
        );
    }
    let unproven = found.iter().filter(|d| !d.proven).count();
    if unproven > 0 {
        println!(
            "\n{unproven} of them have no passing proof — a change here would not be caught there."
        );
    }
    Ok(())
}
