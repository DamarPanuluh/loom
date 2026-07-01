//! `loom intent` command family.

use super::{open, require_lane};
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
            allow_symbol_name,
        } => intent_add(
            graph,
            name,
            description,
            level,
            lifecycle,
            visibility,
            allow_symbol_name,
        ),
        IntentCmd::Show { key } => intent_show(graph, key),
        IntentCmd::Set {
            key,
            level,
            visibility,
        } => intent_set(graph, key, level, visibility),
        IntentCmd::Reactivate { key, reason } => intent_reactivate(graph, key, reason),
        IntentCmd::List { limit } => intent_list(graph, limit, json),
        IntentCmd::Mark {
            key,
            lifecycle,
            reason,
        } => intent_mark(graph, key, lifecycle, reason),
        IntentCmd::Update {
            key,
            description,
            reason,
            reword,
        } => intent_update(graph, key, description, reason, reword),
        IntentCmd::Retire {
            key,
            reason,
            replaced_by,
        } => intent_retire(graph, key, reason, replaced_by),
        IntentCmd::Confirm { key } => intent_confirm(graph, key),
        IntentCmd::Tag { action, key, term } => intent_tag(graph, action, key, term),
    }
}

fn intent_add(
    graph: Option<&Path>,
    name: String,
    description: String,
    level: String,
    lifecycle: String,
    visibility: Option<String>,
    allow_symbol_name: bool,
) -> Result<()> {
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
    if let Some(v) = visibility {
        store.set_facet(
            &node.id,
            TargetKind::Node,
            "visibility",
            &v,
            TruthClass::Asserted,
        )?;
    }
    println!("added intent '{}' [{}]", node.name, &node.id[..8]);
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
) -> Result<()> {
    if level.is_none() && visibility.is_none() {
        bail!("nothing to set — pass --level and/or --visibility");
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
    println!("updated intent '{}'", n.name);
    Ok(())
}

fn intent_reactivate(graph: Option<&Path>, key: String, reason: String) -> Result<()> {
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
    println!("reactivated intent '{}' → planned", n.name);
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
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    // builder lane (lifecycle is builder-owned); solo allowed
    require_lane(&store, crate::registry::OwnerRole::Builder)?;
    store.update_node(&n.id, None, None, Some(&lifecycle))?;
    if let Some(r) = reason {
        store.add_note(&n.id, "decision", &format!("lifecycle {lifecycle}: {r}"))?;
    }
    println!("marked '{}' lifecycle={lifecycle}", n.name);
    Ok(())
}

fn intent_update(
    graph: Option<&Path>,
    key: String,
    description: String,
    reason: String,
    reword: bool,
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    if reword {
        // clearer words, same concept: no ripple
        require_lane(&store, crate::registry::OwnerRole::Builder)?;
        store.update_node(&n.id, None, Some(&description), None)?;
        store.add_note(&n.id, "decision", &format!("reworded: {reason}"))?;
        println!("reworded '{}' (no ripple)", n.name);
    } else {
        let reopened = store.redefine_intent(&n.id, &description)?;
        store.add_note(&n.id, "decision", &format!("redefined: {reason}"))?;
        println!("redefined '{}' — {reopened} edge(s) re-opened", n.name);
    }
    Ok(())
}

fn intent_retire(
    graph: Option<&Path>,
    key: String,
    reason: String,
    replaced_by: Option<String>,
) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    let rb = match replaced_by.as_deref() {
        Some(r) => Some(store.resolve_node(r, Some(NodeType::Intent))?.id),
        None => None,
    };
    store.retire_intent(&n.id, &reason, rb.as_deref())?;
    println!("retired '{}'", n.name);
    Ok(())
}

fn intent_confirm(graph: Option<&Path>, key: String) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    store.add_note(&n.id, "confirm", "meaning re-affirmed")?;
    println!("confirmed '{}'", n.name);
    Ok(())
}

fn intent_tag(graph: Option<&Path>, action: String, key: String, term: String) -> Result<()> {
    let store = open(graph)?;
    let n = store.resolve_node(&key, Some(NodeType::Intent))?;
    match action.as_str() {
        "add" => {
            if !store.vocab_has(&term)? {
                bail!("'{term}' is not a registered vocab term; add it with `loom vocab add`");
            }
            store.set_tag(&n.id, TargetKind::Node, &term)?;
            println!("tagged '{}' with '{term}'", n.name);
        }
        "remove" => {
            store.remove_tag(&n.id, TargetKind::Node, &term)?;
            println!("untagged '{}' '{term}'", n.name);
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
