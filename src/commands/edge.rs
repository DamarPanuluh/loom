//! `loom edge` command family.

use super::{open, pulse, verdict_status};
use crate::cli::EdgeCmd;
use crate::model::{EdgeKind, GroundingRole, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::workitem;
use crate::Result;
use anyhow::anyhow;
use std::path::Path;

pub fn dispatch(graph: Option<&Path>, cmd: EdgeCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        EdgeCmd::Implement {
            intent,
            codefile,
            locator,
            role,
        } => edge_implement(&store, intent, codefile, locator, role, json),
        EdgeCmd::Call {
            validation,
            surface,
        } => edge_call(&store, validation, surface, json),
        EdgeCmd::Remove { edge_id, reason } => edge_remove(&store, edge_id, reason, json),
        EdgeCmd::SetLocator { edge_id, locator } => {
            edge_set_locator(&store, edge_id, locator, json)
        }
        EdgeCmd::Relate { kind, from, to } => edge_relate(&store, kind, from, to, json),
        EdgeCmd::Verdict {
            edge_id,
            verdict,
            criterion,
            evidence,
            confidence,
        } => edge_verdict(
            &store, edge_id, verdict, criterion, evidence, confidence, json,
        ),
        EdgeCmd::Explore {
            a,
            b,
            verdict,
            criterion,
            evidence,
            confidence,
        } => edge_explore(&store, a, b, verdict, criterion, evidence, confidence, json),
        EdgeCmd::SetRole {
            edge_id,
            role,
            reason,
        } => edge_set_role(&store, edge_id, role, reason, json),
        EdgeCmd::Rehome {
            edge_id,
            to,
            reason,
        } => edge_rehome(&store, edge_id, to, reason, json),
        EdgeCmd::Show { edge_id } => edge_show(&store, edge_id, json),
        EdgeCmd::List { limit } => edge_list(&store, limit, json),
        EdgeCmd::DependsOn { intent, upstream } => edge_depends_on(&store, intent, upstream, json),
    }
}

fn parse_grounding_role(s: &str) -> Result<GroundingRole> {
    s.parse::<GroundingRole>()
        .map_err(|_| anyhow!("unknown role '{s}' (use realizes|consumes|configures|verifies)"))
}

fn edge_depends_on(store: &Store, intent: String, upstream: String, json: bool) -> Result<()> {
    let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
    let u = store.resolve_node(&upstream, Some(NodeType::UpstreamIntent))?;
    let e = store.add_edge(EdgeKind::DependsOn, &i.id, &u.id, TruthClass::Asserted)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge_id": e.id,
            "kind": "depends_on",
            "from": { "id": i.id, "name": i.name },
            "to": { "id": u.id, "name": u.name },
        }),
        "loom sync",
        format!(
            "depends_on: '{}' → upstream '{}' [{}]",
            i.name,
            u.name,
            &e.id[..8.min(e.id.len())]
        ),
    )
}

fn edge_implement(
    store: &Store,
    intent: String,
    codefile: String,
    locator: Option<String>,
    role: Option<String>,
    json: bool,
) -> Result<()> {
    let i = store.resolve_node(&intent, Some(NodeType::Intent))?;
    let cf = store.resolve_node(&codefile, Some(NodeType::CodeFile))?;
    let role = match role.as_deref() {
        Some(r) => parse_grounding_role(r)?,
        None => GroundingRole::Realizes,
    };
    let e = store.add_edge(EdgeKind::Implements, &i.id, &cf.id, TruthClass::Asserted)?;
    if let Some(loc) = &locator {
        store.set_facet(
            &e.id,
            TargetKind::Edge,
            "locator",
            loc,
            TruthClass::Asserted,
        )?;
    }
    // Persist the role facet only when non-default: a `realizes` grounding needs
    // no facet (it is the default), so pre-role graphs keep their byte-format.
    if role != GroundingRole::Realizes {
        store.set_grounding_role(&e.id, role)?;
    }
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "intent": { "id": i.id, "name": i.name },
            "codefile": { "id": cf.id, "path": cf.name },
            "locator": locator,
            "role": role.as_str(),
        }),
        "loom sync",
        format!(
            "grounded '{}' in '{}' as {} [{}]",
            i.name,
            cf.name,
            role,
            &e.id[..8]
        ),
    )?;
    Ok(())
}

/// Reclassify a grounding edge's role. Delegates the re-open policy to the
/// store (a changed role stales the settled claim as `role_changed`).
fn edge_set_role(
    store: &Store,
    edge_id: String,
    role: String,
    reason: String,
    json: bool,
) -> Result<()> {
    let role = parse_grounding_role(&role)?;
    let e = store.resolve_edge(&edge_id)?;
    let (edge, old, reopened) = store.reclassify_grounding(&e.id, role, &reason)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": edge,
            "old_role": old.as_str(),
            "new_role": role.as_str(),
            "reopened": reopened,
            "reason": reason,
        }),
        if reopened {
            "loom next --mode analyze"
        } else {
            "loom status"
        },
        format!(
            "role {} → {} on [{}]{}",
            old,
            role,
            &edge.id[..8],
            if reopened {
                " (claim re-opened: role_changed)"
            } else {
                ""
            }
        ),
    )?;
    Ok(())
}

/// Rehome a grounding edge to a successor intent (supersede-not-delete).
fn edge_rehome(
    store: &Store,
    edge_id: String,
    to: String,
    reason: String,
    json: bool,
) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    let successor = store.resolve_node(&to, Some(NodeType::Intent))?;
    let (old, new) = store.rehome_grounding(&e.id, &successor.id, &reason)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "superseded_edge": old,
            "new_edge": new,
            "successor_intent": { "id": successor.id, "name": successor.name },
            "reason": reason,
        }),
        "loom next --mode analyze",
        format!(
            "rehomed grounding [{}] → '{}' [{}] (old superseded, new unverified)",
            &old.id[..8],
            successor.name,
            &new.id[..8]
        ),
    )?;
    Ok(())
}

/// Bind a validation to an interface surface it exercises (a `calls` edge).
/// Idempotent: re-calling the same pair returns the existing edge.
fn edge_call(store: &Store, validation: String, surface: String, json: bool) -> Result<()> {
    let v = store.resolve_node(&validation, Some(NodeType::Validation))?;
    let s = store.resolve_node(&surface, Some(NodeType::InterfaceSurface))?;
    let existing = store.edges_with(Some(EdgeKind::Calls), Some(&v.id), Some(&s.id))?;
    let e = match existing.into_iter().next() {
        Some(e) => e,
        None => store.add_edge(EdgeKind::Calls, &v.id, &s.id, TruthClass::Asserted)?,
    };
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "validation": {
                "id": v.id,
                "name": v.name,
            },
            "surface": {
                "id": s.id,
                "name": s.name,
            },
        }),
        "loom status",
        format!("'{}' calls surface '{}' [{}]", v.name, s.name, &e.id[..8]),
    )?;
    Ok(())
}

/// Prune an edge. Refuses derived edges — those are recomputed by `loom sync`,
/// so deleting one is pointless (it returns on the next sync); the fix is to
/// remove its source. Asserted edges (a redundant grounding/relationship) go.
fn edge_remove(store: &Store, edge_id: String, reason: Option<String>, json: bool) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    if e.truth_class == TruthClass::Derived {
        anyhow::bail!(
            "edge [{}] is a derived {} edge — it is rebuilt by `loom sync`; remove its source, not the edge",
            &e.id[..8],
            e.kind
        );
    }
    let ungrounded_intent = if e.kind == EdgeKind::Implements {
        Some(
            store
                .get_node(&e.from_id)?
                .map(|n| n.name)
                .unwrap_or_else(|| e.from_id.clone()),
        )
    } else {
        None
    };
    // Journal the prune on the source node before the edge id is gone.
    if let Some(r) = &reason {
        store.add_note(
            &e.from_id,
            "decision",
            &format!("removed {} edge: {r}", e.kind),
        )?;
    }
    store.delete_edge(&e.id)?;
    let warning = if let Some(intent_name) = &ungrounded_intent {
        if store.realizing_groundings(&e.from_id)?.is_empty() {
            Some(format!(
                "warning: intent '{intent_name}' now has no realizing grounding; run `loom status` or re-ground it"
            ))
        } else {
            None
        }
    } else {
        None
    };
    pulse::emit(
        store,
        json,
        serde_json::json!({
            "removed": true,
            "edge": e,
            "reason": reason,
            "warning": warning,
        }),
        "loom status",
        || {
            println!(
                "removed {} edge [{}]  ({} → {})",
                e.kind,
                &e.id[..8],
                e.from_id,
                e.to_id
            );
            if let Some(w) = &warning {
                println!("{w}");
            }
            Ok(())
        },
    )?;
    Ok(())
}

/// Correct the `locator` (symbol) facet on an asserted edge — e.g. a grounding
/// whose target symbol moved or was misnamed. Upserts; refuses derived edges.
fn edge_set_locator(store: &Store, edge_id: String, locator: String, json: bool) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    if e.truth_class == TruthClass::Derived {
        anyhow::bail!(
            "edge [{}] is derived — its facets are sync-owned",
            &e.id[..8]
        );
    }
    store.set_facet(
        &e.id,
        TargetKind::Edge,
        "locator",
        &locator,
        TruthClass::Asserted,
    )?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "locator": locator,
        }),
        "loom sync",
        format!(
            "set locator on {} edge [{}] → {locator}",
            e.kind,
            &e.id[..8]
        ),
    )?;
    Ok(())
}

fn edge_relate(store: &Store, kind: String, from: String, to: String, json: bool) -> Result<()> {
    let k = workitem::relationship_kind(&kind)
        .ok_or_else(|| anyhow!("unknown relationship kind '{kind}'"))?;
    let a = store.resolve_node(&from, Some(NodeType::Intent))?;
    let b = store.resolve_node(&to, Some(NodeType::Intent))?;
    let e = store.add_edge(k, &a.id, &b.id, TruthClass::Asserted)?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "kind": kind,
            "from": {
                "id": a.id,
                "name": a.name,
            },
            "to": {
                "id": b.id,
                "name": b.name,
            },
        }),
        "loom status",
        format!("{} '{}' → '{}' [{}]", kind, a.name, b.name, &e.id[..8]),
    )?;
    Ok(())
}

fn edge_verdict(
    store: &Store,
    edge_id: String,
    verdict: String,
    criterion: String,
    evidence: String,
    confidence: f64,
    json: bool,
) -> Result<()> {
    let status = verdict_status(&verdict)?;
    let target = store.resolve_edge(&edge_id)?;
    let e = store.record_verdict(&target.id, status, &criterion, &evidence, confidence, "llm")?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": e,
            "verdict": verdict,
            "criterion": criterion,
            "evidence": evidence,
            "confidence": confidence,
        }),
        "loom status",
        format!("recorded {} on edge [{}]", e.status, &e.id[..8]),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn edge_explore(
    store: &Store,
    a: String,
    b: String,
    verdict: String,
    criterion: String,
    evidence: String,
    confidence: f64,
    json: bool,
) -> Result<()> {
    let ia = store.resolve_node(&a, Some(NodeType::Intent))?;
    let ib = store.resolve_node(&b, Some(NodeType::Intent))?;
    let existing = store.edges_with(Some(EdgeKind::Relates), Some(&ia.id), Some(&ib.id))?;
    let edge = match existing.into_iter().next() {
        Some(e) => e,
        None => store.add_edge(EdgeKind::Relates, &ia.id, &ib.id, TruthClass::Asserted)?,
    };
    let status = verdict_status(&verdict)?;
    let verdict_edge =
        store.record_verdict(&edge.id, status, &criterion, &evidence, confidence, "llm")?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": verdict_edge,
            "a": {
                "id": ia.id,
                "name": ia.name,
            },
            "b": {
                "id": ib.id,
                "name": ib.name,
            },
            "verdict": verdict,
            "criterion": criterion,
            "evidence": evidence,
            "confidence": confidence,
        }),
        "loom status",
        format!("explored '{}' ~ '{}': {}", ia.name, ib.name, status),
    )?;
    Ok(())
}

fn edge_show(store: &Store, edge_id: String, json: bool) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    let facets = store.facets_of(&e.id, TargetKind::Edge)?;
    if json {
        let facet_obj: serde_json::Map<String, serde_json::Value> = facets
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let mut v = serde_json::to_value(&e)?;
        v["facets"] = serde_json::Value::Object(facet_obj);
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("{} [{}]", e.kind, e.id);
        println!("  {} → {}", e.from_id, e.to_id);
        println!("  truth_class: {}  status: {}", e.truth_class, e.status);
        if !e.criterion.is_empty() {
            println!("  criterion: {}", e.criterion);
        }
        if !e.evidence.is_empty() {
            println!("  evidence: {}", e.evidence);
        }
        for (k, val) in &facets {
            println!("  {k}: {val}");
        }
    }
    Ok(())
}

fn edge_list(store: &Store, limit: usize, json: bool) -> Result<()> {
    let edges = store.list_edges(None, limit)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&edges)?);
    } else {
        if edges.is_empty() {
            println!("no edges");
        }
        for e in &edges {
            println!(
                "{:<10} {:<18} {} [{}]",
                e.truth_class,
                e.kind,
                e.status,
                &e.id[..8]
            );
        }
    }
    Ok(())
}
