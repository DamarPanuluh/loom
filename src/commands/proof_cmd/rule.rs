use super::*;

pub(crate) fn rule(graph: Option<&Path>, cmd: RuleCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        RuleCmd::Seed { pack } => rule_seed(&store, json, pack),
        RuleCmd::Verdict {
            rule,
            intent,
            outcome,
            criterion,
            evidence,
            confidence,
        } => rule_verdict(
            &store,
            json,
            RuleVerdictArgs {
                rule,
                intent,
                outcome,
                criterion,
                evidence,
                confidence,
            },
        ),
        RuleCmd::List { limit, offset } => rule_list(&store, json, limit, offset),
        RuleCmd::Show { key } => rule_show(&store, json, key),
        RuleCmd::Add {
            name,
            category,
            description,
        } => rule_add(&store, json, name, category, description),
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
        } => rule_update(
            &store,
            json,
            RuleUpdateArgs {
                key,
                description,
                category,
                severity,
                effort,
                guide,
                hint,
                pattern,
                reason,
            },
        ),
        RuleCmd::Remove { key } => rule_remove(&store, json, key),
        RuleCmd::Unlink { rule, intent } => rule_unlink(&store, json, rule, intent),
        RuleCmd::Suppress {
            rule,
            excerpt,
            reason,
        } => rule_suppress(&store, json, rule, excerpt, reason),
        RuleCmd::Unsuppress { rule, key } => rule_unsuppress(&store, json, rule, key),
        RuleCmd::Suppressions { rule } => rule_suppressions(&store, json, rule),
    }
}

fn rule_seed(store: &Store, json: bool, pack: String) -> Result<()> {
    let n = crate::packs::seed(store, &pack)?;
    pulse::emit_line(
        store,
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

struct RuleVerdictArgs {
    rule: String,
    intent: String,
    outcome: String,
    criterion: String,
    evidence: String,
    confidence: f64,
}

fn rule_verdict(store: &Store, json: bool, args: RuleVerdictArgs) -> Result<()> {
    let r = store.resolve_node(&args.rule, Some(NodeType::QualityRule))?;
    let i = store.resolve_node(&args.intent, Some(NodeType::Intent))?;
    let edge = store.ensure_edge(EdgeKind::Governs, &r.id, &i.id)?;
    let st = verdict_status_quality(&args.outcome)?;
    let actor = store.execution_identity().actor();
    let verdict_edge = store.record_verdict(
        &edge.id,
        st,
        &args.criterion,
        &args.evidence,
        args.confidence,
        &actor,
    )?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "rule": node_json(&r),
            "intent": node_json(&i),
            "edge": verdict_edge,
            "outcome": &args.outcome,
            "criterion": args.criterion,
            "evidence": args.evidence,
            "confidence": args.confidence,
        }),
        "loom status",
        format!("rule '{}' {} on '{}'", r.name, st, i.name),
    )
}

fn rule_list(store: &Store, json: bool, limit: usize, offset: usize) -> Result<()> {
    let rules = store.list_nodes_page(Some(NodeType::QualityRule), limit, offset)?;
    let total = store.count_nodes(Some(NodeType::QualityRule))?;
    if json {
        let rows: Vec<_> = rules.iter().map(node_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&pagination_envelope(&rows, offset, limit, total))?
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
        if let Some(footer) = page_footer(shown, offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}

fn rule_show(store: &Store, json: bool, key: String) -> Result<()> {
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

fn rule_add(
    store: &Store,
    json: bool,
    name: String,
    category: String,
    description: String,
) -> Result<()> {
    let r = store.add_node(
        NodeType::QualityRule,
        &name,
        &description,
        "",
        serde_json::json!({ "category": category }),
    )?;
    pulse::emit_line(
        store,
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

struct RuleUpdateArgs {
    key: String,
    description: Option<String>,
    category: Option<String>,
    severity: Option<String>,
    effort: Option<String>,
    guide: Option<String>,
    hint: Vec<String>,
    pattern: Vec<String>,
    reason: String,
}

fn rule_update(store: &Store, json: bool, args: RuleUpdateArgs) -> Result<()> {
    if args.reason.trim().is_empty() {
        bail!("rule update needs substantive --reason");
    }
    if args.description.is_none()
        && args.category.is_none()
        && args.severity.is_none()
        && args.effort.is_none()
        && args.guide.is_none()
        && args.hint.is_empty()
        && args.pattern.is_empty()
    {
        bail!("nothing to update — pass a rule field to change");
    }
    let r = store.resolve_node(&args.key, Some(NodeType::QualityRule))?;
    let mut body = r.body.clone();
    if let Some(v) = &args.category {
        body["category"] = serde_json::json!(v);
    }
    if let Some(v) = &args.severity {
        body["severity"] = serde_json::json!(v);
    }
    if let Some(v) = &args.effort {
        body["effort"] = serde_json::json!(v);
    }
    if let Some(v) = &args.guide {
        body["inspection_guide"] = serde_json::json!(v);
    }
    if !args.hint.is_empty() {
        body["detection_hints"] = serde_json::json!(args.hint);
    }
    if !args.pattern.is_empty() {
        body["patterns"] = serde_json::json!(args.pattern);
    }
    let updated = if let Some(v) = &args.description {
        store.update_node(&r.id, None, Some(v), None)?
    } else {
        r.clone()
    };
    store.set_node_body(&r.id, &body)?;
    store.add_note(
        &r.id,
        "decision",
        &format!("updated quality rule: {}", args.reason),
    )?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "rule": {
                "id": r.id,
                "name": r.name,
                "description": updated.description,
                "body": body,
            },
            "reason": args.reason,
        }),
        "loom status",
        format!("updated quality rule '{}'", r.name),
    )
}

fn rule_remove(store: &Store, json: bool, key: String) -> Result<()> {
    let r = store.resolve_node(&key, Some(NodeType::QualityRule))?;
    store.delete_node(&r.id)?;
    pulse::emit_line(
        store,
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

fn rule_unlink(store: &Store, json: bool, rule: String, intent: String) -> Result<()> {
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
                store,
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

fn rule_suppress(
    store: &Store,
    json: bool,
    rule: String,
    excerpt: String,
    reason: String,
) -> Result<()> {
    let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
    let row = store.suppress_hit(&r.name, &excerpt, &reason)?;
    pulse::emit_line(
        store,
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

fn rule_unsuppress(store: &Store, json: bool, rule: String, key: String) -> Result<()> {
    let r = store.resolve_node(&rule, Some(NodeType::QualityRule))?;
    let row = store.unsuppress_hit(&r.name, &key)?;
    pulse::emit_line(
        store,
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

fn rule_suppressions(store: &Store, json: bool, rule: Option<String>) -> Result<()> {
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

fn verdict_status_quality(s: &str) -> Result<InspectionStatus> {
    match s {
        "passing" => Ok(InspectionStatus::Passing),
        "failing" => Ok(InspectionStatus::Failing),
        "independent" => Ok(InspectionStatus::Independent),
        other => bail!("unknown status '{other}' (use passing|failing|independent)"),
    }
}
