fn intent_update(graph: Option<&Path>, args: IntentUpdateArgs, json: bool) -> Result<()> {
    preflight_intent_update(&args)?;
    let store = open(graph)?;
    let n = store.resolve_node(&args.key, Some(NodeType::Intent))?;
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    let mut parts: Vec<String> = Vec::new();
    apply_intent_identity_edits(&store, &n, &args.new_name, &args.reason, &mut parts)?;
    apply_intent_attribute_edits(&store, &n.id, &args, &mut parts)?;
    apply_intent_rectify(&store, &n.id, &args.rectify, &args.reason, &mut parts)?;
    let reopened = apply_intent_description(
        &store,
        &n.id,
        &args.description,
        args.reword,
        &args.reason,
        &mut parts,
    )?;
    let display_name = args.new_name.as_deref().unwrap_or(n.name.as_str());
    let next_step = if args.lifecycle.as_deref() == Some("implemented") {
        "loom sync"
    } else {
        "loom status"
    };
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": {
                "id": n.id,
                "name": display_name,
                "previous_name": n.name,
                "description": args.description,
                "level": args.level,
                "visibility": args.visibility,
                "aspect": args.aspect,
                "lifecycle": args.lifecycle,
                "status": args.lifecycle.as_deref().unwrap_or(&n.status),
            },
            "reword": args.reword,
            "reopened_edges": reopened,
            "reason": args.reason,
        }),
        next_step,
        format!("updated '{display_name}': {}", parts.join(", ")),
    )
}

fn preflight_intent_update(args: &IntentUpdateArgs) -> Result<()> {
    if args.description.is_none()
        && args.new_name.is_none()
        && args.level.is_none()
        && args.visibility.is_none()
        && args.aspect.is_none()
        && args.lifecycle.is_none()
        && args.rectify.is_none()
    {
        bail!(
            "nothing to update — pass --description, --name, --level, --visibility, \
             --aspect, --lifecycle and/or --rectify"
        );
    }
    if args.reason.trim().is_empty() {
        bail!("intent update needs substantive --reason");
    }
    if let Some(l) = &args.level {
        check_level(l)?;
    }
    if let Some(v) = &args.visibility {
        check_visibility(v)?;
    }
    if let Some(a) = &args.aspect {
        check_aspect(a)?;
    }
    if let Some(lc) = &args.lifecycle {
        check_lifecycle(lc, false)?;
    }
    if let Some(r) = &args.rectify {
        if r != "escalated" && r != "clear" {
            bail!("unknown --rectify '{r}' (use escalated|clear)");
        }
    }
    Ok(())
}

fn apply_intent_identity_edits(
    store: &Store,
    n: &Node,
    new_name: &Option<String>,
    reason: &str,
    parts: &mut Vec<String>,
) -> Result<()> {
    let Some(name) = new_name else {
        return Ok(());
    };
    if crate::commands::looks_like_symbol(name) {
        bail!(
            "new name '{name}' looks like a code symbol — intents are behaviors; \
             symbols belong on implements-edge locators"
        );
    }
    store.update_node(&n.id, Some(name), None, None)?;
    store.add_note(
        &n.id,
        "decision",
        &format!("renamed from '{}': {reason}", n.name),
    )?;
    parts.push(format!("renamed from '{}'", n.name));
    Ok(())
}

fn apply_intent_attribute_edits(
    store: &Store,
    id: &str,
    args: &IntentUpdateArgs,
    parts: &mut Vec<String>,
) -> Result<()> {
    if let Some(l) = &args.level {
        store.set_facet(id, TargetKind::Node, "level", l, TruthClass::Asserted)?;
        parts.push(format!("level={l}"));
    }
    if let Some(v) = &args.visibility {
        store.set_facet(id, TargetKind::Node, "visibility", v, TruthClass::Asserted)?;
        parts.push(format!("visibility={v}"));
    }
    if let Some(a) = &args.aspect {
        store.set_facet(id, TargetKind::Node, "aspect", a, TruthClass::Asserted)?;
        parts.push(format!("aspect={a}"));
    }
    if let Some(lc) = &args.lifecycle {
        store.update_node(id, None, None, Some(lc))?;
        store.add_note(id, "decision", &format!("lifecycle {lc}: {}", args.reason))?;
        parts.push(format!("lifecycle={lc}"));
    }
    Ok(())
}

fn apply_intent_rectify(
    store: &Store,
    id: &str,
    rectify: &Option<String>,
    reason: &str,
    parts: &mut Vec<String>,
) -> Result<()> {
    let Some(r) = rectify else {
        return Ok(());
    };
    match r.as_str() {
        "escalated" => {
            store.set_facet(
                id,
                TargetKind::Node,
                crate::divergence::RECTIFY_FACET,
                crate::divergence::RECTIFY_ESCALATED,
                TruthClass::Asserted,
            )?;
            store.add_note(
                id,
                "decision",
                &format!("rectify escalated to human ratify: {reason}"),
            )?;
            parts.push("rectify=escalated".into());
        }
        "clear" => {
            let duplicate_pairs = crate::divergence::clear_duplicate_pairs(store, id, reason)?;
            if duplicate_pairs > 0 {
                parts.push(format!(
                    "rectify=clear ({duplicate_pairs} duplicate pair decision{})",
                    if duplicate_pairs == 1 { "" } else { "s" }
                ));
            } else {
                store.clear_facet(id, TargetKind::Node, crate::divergence::RECTIFY_FACET)?;
                store.add_note(
                    id,
                    "decision",
                    &format!("rectify escalation cleared: {reason}"),
                )?;
                parts.push("rectify=clear".into());
            }
        }
        _ => unreachable!("validated above"),
    }
    Ok(())
}

fn apply_intent_description(
    store: &Store,
    id: &str,
    description: &Option<String>,
    reword: bool,
    reason: &str,
    parts: &mut Vec<String>,
) -> Result<usize> {
    let Some(description) = description else {
        return Ok(0);
    };
    if reword {
        store.update_node(id, None, Some(description), None)?;
        store.add_note(id, "decision", &format!("reworded: {reason}"))?;
        parts.push("reworded (no ripple)".into());
        return Ok(0);
    }
    // redefine_intent also stales a ratified intent's ratification to
    // needs_reconfirmation — wantedness rots with meaning.
    let reopened = store.redefine_intent(id, description)?;
    store.clear_facet(id, TargetKind::Node, "journey_exemption")?;
    store.add_note(id, "decision", &format!("redefined: {reason}"))?;
    parts.push(format!("redefined — {reopened} edge(s) re-opened"));
    Ok(reopened)
}

struct IntentUpdateArgs {
    key: String,
    description: Option<String>,
    new_name: Option<String>,
    level: Option<String>,
    visibility: Option<String>,
    aspect: Option<String>,
    lifecycle: Option<String>,
    rectify: Option<String>,
    reason: String,
    reword: bool,
}
