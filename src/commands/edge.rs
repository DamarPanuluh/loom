//! `loom edge` command family.
//!
//! Plane: CLI surface over the judgment plane — asserted relationships and
//! verdicts; every settled write goes through the store's evidence gates.

use super::{open, pulse, verdict_status};
use crate::cli::EdgeCmd;
use crate::model::{EdgeKind, GroundingRole, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::workitem;
use crate::Result;
use anyhow::{anyhow, bail};
use std::path::Path;

/// Refuse a locator that does not resolve under the shared symbol/anchor rules.
fn require_resolvable_locator(
    store: &Store,
    file: &crate::model::Node,
    locator: &str,
) -> Result<()> {
    crate::locator::validate_for_codefile(store, file, locator)
}

/// Symbol kinds the call graph can treat as callable — the only symbols a
/// proof can call its way to (S3+).
const CALLABLE_KINDS: &[&str] = &["function", "method"];

/// Proof-strength lints, computed at edge-write time: a grounding that makes
/// the top strength grades permanently unreachable says so NOW — not later as
/// the symptom "nothing reaches the grounded symbol". Each lint says what
/// WOULD be indexable. Warnings, never refusals: the resolution gate refuses
/// fabrications; these are honest-but-capped groundings.
fn grounding_lints(
    root: &Path,
    file: &str,
    locator: Option<&str>,
    role: GroundingRole,
) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(root.join(file)) else {
        return Vec::new(); // an unreadable file is the resolution gate's refusal
    };
    let ex = crate::extract::extract(file, &content);
    let callable_names: Vec<&str> = ex
        .symbols
        .iter()
        .filter(|s| CALLABLE_KINDS.contains(&s.kind.as_str()))
        .map(|s| s.name.as_str())
        .collect();
    let mut out = Vec::new();
    if locator.is_some_and(crate::locator::is_anchor_locator) {
        out.push(format!(
            "locator '{}' is navigation-only — it can stabilize tracing and blast radius, but cannot itself earn S3 or prove behavior",
            locator.unwrap_or_default()
        ));
        return out;
    }
    match role {
        GroundingRole::Realizes => {
            let Some(loc) = locator else { return out };
            if crate::locator::is_module_scope(loc) {
                return out;
            }
            for name in crate::locator::symbols(loc) {
                // An unresolved name is require_resolvable_locator's refusal,
                // not a lint.
                let Some(sym) = ex.symbols.iter().find(|s| s.name == name) else {
                    continue;
                };
                if !CALLABLE_KINDS.contains(&sym.kind.as_str()) {
                    let alternatives = if callable_names.is_empty() {
                        "this file declares no callable symbol — ground through a different file"
                            .to_string()
                    } else {
                        format!(
                            "callable symbols here: {}",
                            callable_names
                                .iter()
                                .take(5)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                    out.push(format!(
                        "locator '{name}' is a {} in '{file}', which the call graph cannot treat \
                         as callable — no proof can reach S3 (call-graph witness) through it; \
                         the verdict symptom would be \"nothing reaches the grounded symbol\". {alternatives}",
                        sym.kind
                    ));
                }
            }
        }
        GroundingRole::Verifies => {
            if ex.symbols.is_empty() {
                out.push(format!(
                    "witness file '{file}' exposes no indexable symbols (language: {}) — a \
                     call-graph witness needs a callable symbol that calls the grounded symbol, \
                     so a proof resting on this file caps below S3. Indexable languages: rust, \
                     python, go, javascript, typescript",
                    ex.language.as_str()
                ));
            } else if callable_names.is_empty() {
                let kinds: Vec<&str> = ex
                    .symbols
                    .iter()
                    .map(|s| s.kind.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                out.push(format!(
                    "witness file '{file}' declares no callable symbol (only: {}) — nothing here \
                     can call the grounded symbol, so a proof resting on this file caps below S3",
                    kinds.join(", ")
                ));
            }
        }
        _ => {}
    }
    out
}

pub fn dispatch(graph: Option<&Path>, cmd: EdgeCmd, json: bool) -> Result<()> {
    let store = open(graph)?;
    match cmd {
        EdgeCmd::Implement {
            intent,
            codefile,
            locator,
            role,
        } => edge_implement(&store, intent, codefile, locator, role, json),
        EdgeCmd::Exercises {
            validation,
            codefile,
            locator,
        } => edge_exercises(&store, validation, codefile, locator, json),
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
        EdgeCmd::Retarget {
            edge_id,
            to,
            reason,
        } => edge_retarget(&store, edge_id, to, reason, json),
        EdgeCmd::Show { edge_id } => edge_show(&store, edge_id, json),
        EdgeCmd::List { limit, offset } => edge_list(&store, limit, offset, json),
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
            crate::model::short(&e.id)
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
    let existing = store
        .edges_with(Some(EdgeKind::Implements), Some(&i.id), Some(&cf.id))?
        .into_iter()
        .next();
    let mut update_existing_locator = false;
    if let Some(edge) = &existing {
        let current_role = store.grounding_role(&edge.id)?;
        let current_locator = store.get_facet(&edge.id, TargetKind::Edge, "locator")?;
        let locator_changed = locator
            .as_deref()
            .is_some_and(|new| current_locator.as_deref() != Some(new));
        if current_role != role
            || (locator_changed && edge.status != crate::model::InspectionStatus::Uninspected)
        {
            bail!(
                "edge exists for intent '{}' and codefile '{}' as {} [{}] — \
                 use `loom edge set-role {}` / `loom edge set-locator {}` or remove it first",
                i.name,
                cf.name,
                current_role,
                crate::model::short(&edge.id),
                crate::model::short(&edge.id),
                crate::model::short(&edge.id),
            );
        }
        // Preserve the long-standing pre-verdict re-grounding convenience.
        // Once inspected, locator changes cross the explicit set-locator gate.
        update_existing_locator = locator_changed;
    }
    // Ordinary symbol resolution is required only for realizing groundings.
    // Anchor cardinality/attachment is strict for every role because a graph
    // write may never persist an ambiguous navigation identity.
    if let Some(loc) = &locator {
        if role == GroundingRole::Realizes || crate::locator::is_anchor_locator(loc) {
            require_resolvable_locator(store, &cf, loc)?;
        }
    }
    let created = existing.is_none();
    let e = match existing {
        Some(edge) => edge,
        None => store.add_edge(EdgeKind::Implements, &i.id, &cf.id, TruthClass::Asserted)?,
    };
    if created || update_existing_locator {
        if let Some(loc) = &locator {
            store.set_facet(
                &e.id,
                TargetKind::Edge,
                "locator",
                loc,
                TruthClass::Asserted,
            )?;
        }
    }
    // Persist the role facet only for a newly-created non-default grounding.
    if created && role != GroundingRole::Realizes {
        store.set_grounding_role(&e.id, role)?;
    }
    let lints = grounding_lints(store.root(), &cf.name, locator.as_deref(), role);
    for l in &lints {
        eprintln!("lint: {l}");
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
            "lints": lints,
        }),
        "loom sync",
        format!(
            "grounded '{}' in '{}' as {} [{}]",
            i.name,
            cf.name,
            role,
            crate::model::short(&e.id)
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
            crate::model::short(&edge.id),
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
            crate::model::short(&old.id),
            successor.name,
            crate::model::short(&new.id)
        ),
    )?;
    Ok(())
}

/// Re-point an asserted edge's target at a successor node, in place (P10):
/// the recorded operation of a file rename/split. The edge keeps its id,
/// locator/role, and verdict facts — sync's reverification decides what
/// still holds (content that moved intact re-anchors; the move itself never
/// forces a re-verdict).
fn edge_retarget(
    store: &Store,
    edge_id: String,
    to: String,
    reason: String,
    json: bool,
) -> Result<()> {
    let e = store.resolve_edge(&edge_id)?;
    let successor = store.resolve_node(&to, None)?;
    let old_to = e.to_id.clone();
    let old_name = store
        .get_node(&old_to)?
        .map(|n| n.name)
        .unwrap_or_else(|| old_to.clone());
    let updated = store.retarget_edge(&e.id, &successor.id, &reason)?;
    store.append_journal(
        "edge_retargeted",
        &e.id,
        serde_json::json!({
            "kind": updated.kind,
            "from": { "id": updated.from_id },
            "old_to": { "id": old_to, "name": old_name },
            "new_to": { "id": successor.id, "name": successor.name },
            "reason": reason,
        }),
    )?;
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": updated,
            "old_to": { "id": old_to, "name": old_name },
            "new_to": { "id": successor.id, "name": successor.name },
            "reason": reason,
            "facets": store.facets_of(&updated.id, TargetKind::Edge)?,
        }),
        "loom sync",
        format!(
            "retargeted edge [{}] {} → '{}' (id/verdicts kept; `loom sync` re-verifies evidence at the new location)",
            crate::model::short(&e.id),
            old_name,
            successor.name
        ),
    )?;
    Ok(())
}

/// Bind a validation to an interface surface it exercises (a `calls` edge).
/// Idempotent: re-calling the same pair returns the existing edge.
fn invalidate_exercises_validation(store: &Store, validation_id: &str, reason: &str) -> Result<()> {
    store.reset_validation_status_for_sync(validation_id)?;
    for validates in store.edges_with(Some(EdgeKind::Validates), Some(validation_id), None)? {
        store.stale_edge(&validates.id, reason)?;
    }
    Ok(())
}

fn edge_exercises(
    store: &Store,
    validation: String,
    codefile: String,
    locator: Option<String>,
    json: bool,
) -> Result<()> {
    let validation = store.resolve_node(&validation, Some(NodeType::Validation))?;
    let codefile = store.resolve_node(&codefile, Some(NodeType::CodeFile))?;
    if let Some(locator) = &locator {
        require_resolvable_locator(store, &codefile, locator)?;
    }
    let existing = store
        .edges_with(
            Some(EdgeKind::Exercises),
            Some(&validation.id),
            Some(&codefile.id),
        )?
        .into_iter()
        .next();
    let is_new = existing.is_none();
    let edge = match existing {
        Some(edge) => edge,
        None => store.add_edge(
            EdgeKind::Exercises,
            &validation.id,
            &codefile.id,
            TruthClass::Asserted,
        )?,
    };
    let previous_locator = store.get_facet(&edge.id, TargetKind::Edge, "locator")?;
    if previous_locator != locator || is_new {
        invalidate_exercises_validation(
            store,
            &validation.id,
            "validation-specific S3 evidence changed",
        )?;
    }
    if let Some(locator) = &locator {
        store.set_facet(
            &edge.id,
            TargetKind::Edge,
            "locator",
            locator,
            TruthClass::Asserted,
        )?;
    } else if previous_locator.is_some() {
        store.clear_facet(&edge.id, TargetKind::Edge, "locator")?;
    }
    pulse::emit_line(
        store,
        json,
        serde_json::json!({
            "edge": edge,
            "validation": { "id": validation.id, "name": validation.name },
            "codefile": { "id": codefile.id, "path": codefile.name },
            "locator": locator,
        }),
        "loom sync",
        format!(
            "validation '{}' exercises '{}' [{}]",
            validation.name,
            codefile.name,
            crate::model::short(&edge.id)
        ),
    )
}

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
        format!(
            "'{}' calls surface '{}' [{}]",
            v.name,
            s.name,
            crate::model::short(&e.id)
        ),
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
            crate::model::short(&e.id),
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
    if e.kind == EdgeKind::Exercises {
        invalidate_exercises_validation(
            store,
            &e.from_id,
            "validation-specific S3 evidence removed",
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
                crate::model::short(&e.id),
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
            crate::model::short(&e.id)
        );
    }
    // Realizing implements edges carry the same probe as `edge implement`.
    let mut lints = Vec::new();
    if e.kind == EdgeKind::Implements {
        let role = store.grounding_role(&e.id)?;
        let file = store.get_node(&e.to_id)?.ok_or_else(|| {
            anyhow!(
                "implements edge [{}] has no codefile",
                crate::model::short(&e.id)
            )
        })?;
        if role == GroundingRole::Realizes || crate::locator::is_anchor_locator(&locator) {
            require_resolvable_locator(store, &file, &locator)?;
        }
        lints = grounding_lints(store.root(), &file.name, Some(&locator), role);
        for l in &lints {
            eprintln!("lint: {l}");
        }
    } else if e.kind == EdgeKind::Exercises {
        let file = store.get_node(&e.to_id)?.ok_or_else(|| {
            anyhow!(
                "exercises edge [{}] has no codefile",
                crate::model::short(&e.id)
            )
        })?;
        require_resolvable_locator(store, &file, &locator)?;
        let previous = store.get_facet(&e.id, TargetKind::Edge, "locator")?;
        if previous.as_deref() != Some(locator.as_str()) {
            invalidate_exercises_validation(
                store,
                &e.from_id,
                "validation-specific S3 locator changed",
            )?;
        }
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
            "lints": lints,
        }),
        "loom sync",
        format!(
            "set locator on {} edge [{}] → {locator}",
            e.kind,
            crate::model::short(&e.id)
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
        format!(
            "{} '{}' → '{}' [{}]",
            kind,
            a.name,
            b.name,
            crate::model::short(&e.id)
        ),
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
    let actor = store.execution_identity().actor();
    let e = store.record_verdict(
        &target.id, status, &criterion, &evidence, confidence, &actor,
    )?;
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
        format!(
            "recorded {} on edge [{}]",
            e.status,
            crate::model::short(&e.id)
        ),
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
    let actor = store.execution_identity().actor();
    let verdict_edge =
        store.record_verdict(&edge.id, status, &criterion, &evidence, confidence, &actor)?;
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
        let prose = store.verdict_prose(&e.id)?;
        if !prose.is_empty() {
            println!("  evidence: {prose}");
        }
        // How strongly the verdict is anchored — the number that decides whether
        // it counts toward a rung, printed next to the claim it qualifies.
        println!("  anchored: {}", store.edge_verification(&e.id)?);
        for (k, val) in &facets {
            println!("  {k}: {val}");
        }
    }
    Ok(())
}

fn edge_list(store: &Store, limit: usize, offset: usize, json: bool) -> Result<()> {
    let edges = store.list_edges_page(None, limit, offset)?;
    let total = store.count_edges(None)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&super::pagination_envelope(
                &edges, offset, limit, total
            ))?
        );
    } else {
        if edges.is_empty() && offset == 0 {
            println!("no edges");
        }
        for e in &edges {
            println!(
                "{:<10} {:<18} {} [{}]",
                e.truth_class,
                e.kind,
                e.status,
                crate::model::short(&e.id)
            );
        }
        if let Some(footer) = super::page_footer(edges.len(), offset, total) {
            println!("{footer}");
        }
    }
    Ok(())
}
