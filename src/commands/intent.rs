//! `loom intent` command family.

use super::{node_json, open, pulse, require_lane};
use crate::cli::IntentCmd;
use crate::model::{NodeType, TargetKind, TruthClass};
use crate::Result;
use anyhow::bail;
use std::path::Path;

pub fn dispatch(graph: Option<&Path>, cmd: IntentCmd, json: bool) -> Result<()> {
    match cmd {
        IntentCmd::Add {
            name,
            description,
            level,
            lifecycle,
            visibility,
            layer,
            aspect,
            allow_symbol_name,
        } => intent_add(
            graph,
            IntentAddArgs {
                name,
                description,
                level,
                lifecycle,
                visibility,
                layer,
                aspect,
                allow_symbol_name,
            },
            json,
        ),
        IntentCmd::Show { key } => intent_show(graph, key),
        IntentCmd::Set {
            key,
            level,
            visibility,
            aspect,
        } => intent_set(graph, key, level, visibility, aspect, json),
        IntentCmd::Waive { key, axis, reason } => intent_waive(graph, key, axis, reason, json),
        IntentCmd::Reactivate { key, reason } => intent_reactivate(graph, key, reason, json),
        IntentCmd::List { limit } => intent_list(graph, limit, json),
        IntentCmd::Mark {
            key,
            lifecycle,
            reason,
        } => intent_mark(graph, key, lifecycle, reason, json),
        IntentCmd::Update {
            key,
            description,
            reason,
            reword,
        } => intent_update(graph, key, description, reason, reword, json),
        IntentCmd::Retire {
            key,
            reason,
            replaced_by,
        } => intent_retire(graph, key, reason, replaced_by, json),
        IntentCmd::Confirm { key } => intent_confirm(graph, key, json),
        IntentCmd::Tag { action, key, term } => intent_tag(graph, action, key, term, json),
    }
}

struct IntentAddArgs {
    name: String,
    description: String,
    level: String,
    lifecycle: String,
    visibility: Option<String>,
    layer: Option<String>,
    aspect: Option<String>,
    allow_symbol_name: bool,
}

/// Validate a scenario aspect label.
fn check_aspect(aspect: &str) -> Result<()> {
    const ASPECTS: &[&str] = &["happy", "sad", "fallback", "edge_case"];
    if !ASPECTS.contains(&aspect) {
        bail!("unknown aspect '{aspect}' (use {})", ASPECTS.join("|"));
    }
    Ok(())
}

fn intent_add(graph: Option<&Path>, args: IntentAddArgs, json: bool) -> Result<()> {
    let IntentAddArgs {
        name,
        description,
        level,
        lifecycle,
        visibility,
        layer,
        aspect,
        allow_symbol_name,
    } = args;
    // INV-ATOM: symbols are locators, not intents.
    if looks_like_symbol(&name) {
        if !allow_symbol_name {
            bail!(
                "intent name '{name}' looks like a code symbol. Intents are behaviors, \
                 not functions. Use a behavioral name, or pass --allow-symbol-name with \
                 a behavioral --description if this is a deliberate symbol-level intent."
            );
        }
        if description.trim().is_empty() {
            bail!(
                "--allow-symbol-name requires a non-empty --description carrying a \
                 behavioral criterion"
            );
        }
    }
    let store = open(graph)?;
    let node = store.add_node(
        NodeType::Intent,
        &name,
        &description,
        &lifecycle,
        serde_json::json!({ "level": level }),
    )?;
    if allow_symbol_name && looks_like_symbol(&name) {
        store.set_facet(
            &node.id,
            TargetKind::Node,
            "symbol_name_override",
            "true",
            TruthClass::Asserted,
        )?;
    }
    store.set_facet(
        &node.id,
        TargetKind::Node,
        "level",
        &level,
        TruthClass::Asserted,
    )?;
    if let Some(v) = &visibility {
        store.set_facet(
            &node.id,
            TargetKind::Node,
            "visibility",
            v,
            TruthClass::Asserted,
        )?;
    }
    if let Some(l) = &layer {
        store.set_facet(&node.id, TargetKind::Node, "layer", l, TruthClass::Asserted)?;
    }
    if let Some(a) = &aspect {
        check_aspect(a)?;
        store.set_facet(
            &node.id,
            TargetKind::Node,
            "aspect",
            a,
            TruthClass::Asserted,
        )?;
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&node),
            "level": level,
            "visibility": visibility,
            "layer": layer,
            "aspect": aspect,
            "allow_symbol_name": allow_symbol_name,
        }),
        "loom status",
        format!("added intent '{}' [{}]", node.name, &node.id[..8]),
    )?;
    Ok(())
}

fn intent_show(graph: Option<&Path>, key: String) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    println!("{} [{}]", n.name, n.id);
    println!("  lifecycle: {}", n.status);
    if !n.description.is_empty() {
        println!("  description: {}", n.description);
    }
    if let Some(level) = store.get_facet(&n.id, TargetKind::Node, "level")? {
        println!("  level: {level}");
    }
    if let Some(vis) = store.get_facet(&n.id, TargetKind::Node, "visibility")? {
        println!("  visibility: {vis}");
    }
    if let Some(layer) = store.get_facet(&n.id, TargetKind::Node, "layer")? {
        println!("  layer: {layer}");
    }
    if let Some(aspect) = store.get_facet(&n.id, TargetKind::Node, "aspect")? {
        println!("  aspect: {aspect}");
    }
    let tags = store.tags_of(&n.id, TargetKind::Node)?;
    if !tags.is_empty() {
        println!("  tags: {}", tags.join(", "));
    }
    Ok(())
}

fn intent_set(
    graph: Option<&Path>,
    key: String,
    level: Option<String>,
    visibility: Option<String>,
    aspect: Option<String>,
    json: bool,
) -> Result<()> {
    if level.is_none() && visibility.is_none() && aspect.is_none() {
        bail!("nothing to set — pass --level, --visibility and/or --aspect");
    }
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    if let Some(l) = &level {
        store.set_facet(&n.id, TargetKind::Node, "level", l, TruthClass::Asserted)?;
    }
    if let Some(v) = &visibility {
        store.set_facet(
            &n.id,
            TargetKind::Node,
            "visibility",
            v,
            TruthClass::Asserted,
        )?;
    }
    if let Some(a) = &aspect {
        check_aspect(a)?;
        store.set_facet(&n.id, TargetKind::Node, "aspect", a, TruthClass::Asserted)?;
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "intent": node_json(&n),
            "level": level,
            "visibility": visibility,
            "aspect": aspect,
        }),
        "loom status",
        format!("updated intent '{}'", n.name),
    )?;
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

fn intent_list(graph: Option<&Path>, limit: usize, json: bool) -> Result<()> {
    let store = open(graph)?;
    let intents = store.list_nodes(Some(NodeType::Intent), limit)?;
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
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if intents.is_empty() {
        println!("no intents");
    }
    for n in &intents {
        println!("{:<12} {} [{}]", n.status, n.name, &n.id[..8]);
    }
    Ok(())
}

fn intent_mark(
    graph: Option<&Path>,
    key: String,
    lifecycle: String,
    reason: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    // builder lane (lifecycle is builder-owned); solo allowed
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    store.update_node(&n.id, None, None, Some(&lifecycle))?;
    if let Some(r) = &reason {
        store.add_note(&n.id, "decision", &format!("lifecycle {lifecycle}: {r}"))?;
    }
    let next_step = if lifecycle == "implemented" {
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
                "name": n.name,
                "lifecycle": lifecycle,
            },
            "reason": reason,
        }),
        next_step,
        format!("marked '{}' lifecycle={lifecycle}", n.name),
    )?;
    Ok(())
}

fn intent_update(
    graph: Option<&Path>,
    key: String,
    description: String,
    reason: String,
    reword: bool,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    if reword {
        // clearer words, same concept: no ripple
        require_lane(&store, crate::registry::OwnerRole::Builder)?;
        store.update_node(&n.id, None, Some(&description), None)?;
        store.add_note(&n.id, "decision", &format!("reworded: {reason}"))?;
        pulse::emit_line(
            &store,
            json,
            serde_json::json!({
                "intent": {
                    "id": n.id,
                    "name": n.name,
                    "description": description,
                    "status": n.status,
                },
                "reword": true,
                "reason": reason,
            }),
            "loom status",
            format!("reworded '{}' (no ripple)", n.name),
        )?;
    } else {
        let reopened = store.redefine_intent(&n.id, &description)?;
        store.add_note(&n.id, "decision", &format!("redefined: {reason}"))?;
        pulse::emit_line(
            &store,
            json,
            serde_json::json!({
                "intent": {
                    "id": n.id,
                    "name": n.name,
                    "description": description,
                    "status": n.status,
                },
                "reword": false,
                "reopened_edges": reopened,
                "reason": reason,
            }),
            "loom status",
            format!("redefined '{}' — {reopened} edge(s) re-opened", n.name),
        )?;
    }
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

fn intent_tag(
    graph: Option<&Path>,
    action: String,
    key: String,
    term: String,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    match action.as_str() {
        "add" => {
            if !store.vocab_has(&term)? {
                bail!("'{term}' is not a registered vocab term; add it with `loom vocab add`");
            }
            store.set_tag(&n.id, TargetKind::Node, &term)?;
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
        "remove" => {
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
        other => bail!("unknown tag action '{other}' (use add|remove)"),
    }
    Ok(())
}

/// Heuristic: does this name look like a code symbol rather than a behavior?
/// Behaviors read as phrases ("payment can be captured"); symbols are single
/// tokens (`capture_payment`, `runWithSqlite`, `Store::open`, `handle()`).
pub fn looks_like_symbol(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() || n.contains(' ') {
        return false;
    }
    n.contains('_') || n.contains("::") || n.contains('(') || has_internal_caps(n)
}

/// camelCase / PascalCase detection: a lowercase letter immediately followed by
/// an uppercase one, e.g. `runWithSqlite`.
fn has_internal_caps(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars
        .windows(2)
        .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::looks_like_symbol;

    #[test]
    fn symbol_names_detected() {
        assert!(looks_like_symbol("capture_payment"));
        assert!(looks_like_symbol("runWithSqlite"));
        assert!(looks_like_symbol("Store::open"));
        assert!(looks_like_symbol("handle()"));
    }

    #[test]
    fn behavioral_names_pass() {
        assert!(!looks_like_symbol("payment can be captured"));
        assert!(!looks_like_symbol("user can log in"));
        assert!(!looks_like_symbol("sync"));
        assert!(!looks_like_symbol("checkout"));
    }
}
