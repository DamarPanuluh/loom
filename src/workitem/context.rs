//! Work-packet context — the read set handed to the LLM with each item.
//!
//! Plane: judgment-plane routing (pure reads over the store). Builds the
//! `TraversalContext` for a node or edge: linked entities, suggested reads,
//! and the grounded-file read set, deterministically ordered and bounded.
//! Context is evidence to read, never a conclusion — this module writes
//! nothing and asserts nothing.

use super::{FileRead, LinkedEntity, SuggestedRead, TraversalContext};
use crate::model::{Edge, EdgeKind, GroundingRole, Node, NodeType};
use crate::store::Store;
use crate::Result;

pub(super) fn node_context(store: &Store, node: &Node, purpose: &str) -> Result<TraversalContext> {
    let mut ctx = TraversalContext {
        purpose: purpose.into(),
        linked_entities: Vec::new(),
        suggested_reads: Vec::new(),
        read_set: Vec::new(),
    };
    let mut seen_entities = std::collections::BTreeSet::new();
    let mut seen_reads = std::collections::BTreeSet::new();
    push_node_entity(&mut ctx, &mut seen_entities, "target", node);
    push_node_read(&mut ctx, &mut seen_reads, "show target entity", node);

    let mut edges = store.edges_with(None, Some(&node.id), None)?;
    edges.extend(store.edges_with(None, None, Some(&node.id))?);
    edges.sort_by(|a, b| {
        a.kind
            .as_str()
            .cmp(b.kind.as_str())
            .then(a.from_id.cmp(&b.from_id))
            .then(a.to_id.cmp(&b.to_id))
    });

    for edge in edges.into_iter().take(12) {
        push_edge_entity(store, &mut ctx, &mut seen_entities, "linked_edge", &edge)?;
        push_edge_read(&mut ctx, &mut seen_reads, "inspect linked edge", &edge);
        let other_id = if edge.from_id == node.id {
            &edge.to_id
        } else {
            &edge.from_id
        };
        if let Some(other) = store.get_node(other_id)? {
            let role = if edge.from_id == node.id {
                format!("outgoing_{}", edge.kind)
            } else {
                format!("incoming_{}", edge.kind)
            };
            push_node_entity(&mut ctx, &mut seen_entities, &role, &other);
            push_node_read(
                &mut ctx,
                &mut seen_reads,
                &format!("inspect {role}"),
                &other,
            );
        }
    }
    push_notes(
        store,
        &mut ctx,
        &mut seen_entities,
        &mut seen_reads,
        &node.id,
    )?;

    match node.node_type {
        // Build-lane packets start from the intent's grounded files.
        NodeType::Intent => {
            push_intent_read_set(store, &mut ctx, node)?;
            if !ctx
                .linked_entities
                .iter()
                .any(|e| e.kind == NodeType::CodeFile.as_str())
            {
                push_raw_read(
                    &mut ctx,
                    &mut seen_reads,
                    "survey registered codefiles when no grounding exists yet",
                    "loom codefile list",
                );
            }
        }
        // Coverage-lane packets are about one concrete file: name it directly —
        // unless it is gone from disk, in which case there is nothing to read.
        NodeType::CodeFile if store.root().join(&node.name).exists() => {
            ctx.read_set.push(FileRead {
                path: node.name.clone(),
                locator: None,
                why: "the file this work item is about — read it before deciding".into(),
            });
        }
        _ => {}
    }
    Ok(ctx)
}

pub(super) fn edge_context(store: &Store, edge: &Edge, purpose: &str) -> Result<TraversalContext> {
    let mut ctx = TraversalContext {
        purpose: purpose.into(),
        linked_entities: Vec::new(),
        suggested_reads: Vec::new(),
        read_set: Vec::new(),
    };
    let mut seen_entities = std::collections::BTreeSet::new();
    let mut seen_reads = std::collections::BTreeSet::new();
    push_edge_entity(store, &mut ctx, &mut seen_entities, "target_edge", edge)?;
    push_edge_read(&mut ctx, &mut seen_reads, "inspect target edge", edge);
    for (role, id) in [("from", &edge.from_id), ("to", &edge.to_id)] {
        if let Some(node) = store.get_node(id)? {
            push_node_entity(&mut ctx, &mut seen_entities, role, &node);
            push_node_read(
                &mut ctx,
                &mut seen_reads,
                &format!("inspect {role} endpoint"),
                &node,
            );
            if matches!(node.node_type, NodeType::Intent) {
                push_grounded_codefiles(
                    store,
                    &mut ctx,
                    &mut seen_entities,
                    &mut seen_reads,
                    &node,
                )?;
            }
        }
    }
    push_notes(
        store,
        &mut ctx,
        &mut seen_entities,
        &mut seen_reads,
        &edge.id,
    )?;
    for id in [&edge.from_id, &edge.to_id] {
        push_notes(store, &mut ctx, &mut seen_entities, &mut seen_reads, id)?;
    }
    // An Exemplar verdict is earned against the located source itself. Unlike
    // Intent groundings, neither endpoint causes a CodeFile read set to be
    // expanded above, so name the exemplar file and locator directly.
    if edge.kind == EdgeKind::Exemplar {
        if let Some(file) = store.get_node(&edge.to_id)? {
            ctx.read_set.push(FileRead {
                path: file.name,
                locator: store.get_facet(&edge.id, crate::model::TargetKind::Edge, "locator")?,
                why: "live symbol proposed as the reviewed Pattern exemplar".into(),
            });
        }
    }
    Ok(ctx)
}

/// The intent's live groundings, realizing edges first — where the behavior
/// lives, so those are read first — then consumer/config/verify edges (they
/// only exercise it across a seam). Superseded (rehomed) edges are dropped.
fn ordered_groundings(store: &Store, intent_id: &str) -> Result<Vec<(Edge, GroundingRole)>> {
    let mut edges = Vec::new();
    for e in store.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
        if store.edge_superseded(&e.id)? {
            continue;
        }
        let role = store.grounding_role(&e.id)?;
        edges.push((e, role));
    }
    // Stable sort: realizing first, original (id) order preserved within a role.
    edges.sort_by_key(|(_, role)| u8::from(*role != GroundingRole::Realizes));
    Ok(edges)
}

fn push_grounded_codefiles(
    store: &Store,
    ctx: &mut TraversalContext,
    seen_entities: &mut std::collections::BTreeSet<String>,
    seen_reads: &mut std::collections::BTreeSet<String>,
    intent: &Node,
) -> Result<()> {
    for (edge, _role) in ordered_groundings(store, &intent.id)?.into_iter().take(8) {
        push_edge_entity(store, ctx, seen_entities, "grounding_edge", &edge)?;
        if let Some(cf) = store.get_node(&edge.to_id)? {
            push_node_entity(ctx, seen_entities, "grounded_codefile", &cf);
            push_node_read(ctx, seen_reads, "inspect grounded codefile", &cf);
        }
    }
    push_intent_read_set(store, ctx, intent)
}

/// Append the intent's grounded files (path + implements locator) to the
/// packet's read set — realizing files first, consumer surfaces after as
/// context. This is what makes a packet self-contained: the worker opens these
/// files directly instead of running follow-up show commands.
fn push_intent_read_set(store: &Store, ctx: &mut TraversalContext, intent: &Node) -> Result<()> {
    // The read set is authoritative, not decorative: never cap it silently.
    // Linked entities above stay bounded for packet size, but every grounded
    // file must remain visible to the worker that earns a verdict.
    for (edge, role) in ordered_groundings(store, &intent.id)? {
        let Some(cf) = store.get_node(&edge.to_id)? else {
            continue;
        };
        if ctx.read_set.iter().any(|r| r.path == cf.name) {
            continue;
        }
        let locator = store.get_facet(&edge.id, crate::model::TargetKind::Edge, "locator")?;
        // Never send a worker to read a ghost without saying so: a grounding
        // whose file vanished is itself the finding.
        let why = if !store.root().join(&cf.name).exists() {
            format!(
                "grounds intent '{}' — but the file is GONE from disk: re-ground the intent in its successor, then `loom codefile remove {}`",
                intent.name, cf.name
            )
        } else if role == GroundingRole::Realizes {
            format!(
                "grounds intent '{}' (realizes — behavior lives here)",
                intent.name
            )
        } else {
            format!(
                "exercises intent '{}' across a seam ({role}) — context, not where the behavior lives",
                intent.name
            )
        };
        ctx.read_set.push(FileRead {
            path: cf.name.clone(),
            locator,
            why,
        });
    }
    Ok(())
}

fn push_node_entity(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    role: &str,
    node: &Node,
) {
    let key = format!("node:{}:{role}", node.id);
    if !seen.insert(key) {
        return;
    }
    // The behavioral criterion is the core input for primary entities; keep
    // peripheral entities lean so the packet stays small.
    //
    // `research_advisory` is primary because its description is the ONLY thing
    // that distinguishes a current conclusion from an expired one: the branch
    // above rewrites it to either "Current advisory research conclusion: …" or
    // "STALE research conclusion (not current guidance; suppress
    // recommendation): …", plus the source list that makes either checkable.
    // Filtering it out computed a warning and then threw it away, leaving a
    // reader unable to tell guidance from history.
    let primary = matches!(
        role,
        "target" | "from" | "to" | "grounded_codefile" | "note" | "research_advisory"
    );
    ctx.linked_entities.push(LinkedEntity {
        role: role.into(),
        kind: node.node_type.as_str().into(),
        id: node.id.clone(),
        name: node.name.clone(),
        description: Some(node.description.clone()).filter(|d| primary && !d.is_empty()),
        status: Some(node.status.clone()).filter(|s| !s.is_empty()),
        edge_kind: None,
        edge_status: None,
        locator: None,
        facets: None,
    });
}

/// Inline the target's adjudication trail. Notes carry evidence no edge
/// traversal can reach — proof evidence copied at hypothesis adoption,
/// experiment task outcomes, up-dependency adjudications — so a packet
/// without them silently drops recorded history.
fn push_notes(
    store: &Store,
    ctx: &mut TraversalContext,
    seen_entities: &mut std::collections::BTreeSet<String>,
    seen_reads: &mut std::collections::BTreeSet<String>,
    target_id: &str,
) -> Result<()> {
    const NOTE_CAP: usize = 6;
    let notes = store.notes_for(target_id)?;
    let overflow = notes.len() > NOTE_CAP;
    for n in notes.iter().take(NOTE_CAP) {
        if let Some(task_id) = n.body.get("research_task_id").and_then(|v| v.as_str()) {
            if let Some(task) = store.get_node(task_id)? {
                let mut advisory = task.clone();
                let body = crate::research::ResearchBody::parse(&task.body)?;
                let now = chrono::Utc::now();
                let stale = body
                    .conclusion_fresh_until
                    .as_ref()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                    .is_some_and(|v| v.with_timezone(&chrono::Utc) < now);
                let sources = body
                    .sources
                    .iter()
                    .map(|s| {
                        format!(
                            "{} published={} retrieved={} fresh_until={}",
                            s.url,
                            s.published_at.as_deref().unwrap_or("unspecified"),
                            s.retrieved_at,
                            s.fresh_until.as_deref().unwrap_or("non-expiring")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                advisory.description = if stale {
                    format!("STALE research conclusion (not current guidance; suppress recommendation): {}. Sources: {}. Create a successor research task for current guidance; never reopen this history.", task.description, sources)
                } else {
                    format!(
                        "Current advisory research conclusion: {}. Sources: {}",
                        task.description, sources
                    )
                };
                push_node_entity(ctx, seen_entities, "research_advisory", &advisory);
                continue;
            }
        }
        push_node_entity(ctx, seen_entities, "note", n);
    }
    if overflow {
        push_raw_read(
            ctx,
            seen_reads,
            "full adjudication trail (older notes elided from this packet)",
            &format!("loom note list {target_id}"),
        );
    }
    Ok(())
}

fn push_edge_entity(
    store: &Store,
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    role: &str,
    edge: &Edge,
) -> Result<()> {
    let key = format!("edge:{}:{role}", edge.id);
    if !seen.insert(key) {
        return Ok(());
    }
    let locator = store.get_facet(&edge.id, crate::model::TargetKind::Edge, "locator")?;
    ctx.linked_entities.push(LinkedEntity {
        role: role.into(),
        kind: "edge".into(),
        id: edge.id.clone(),
        name: format!("{} {} {}", edge.from_id, edge.kind, edge.to_id),
        description: None,
        status: None,
        edge_kind: Some(edge.kind.as_str().into()),
        edge_status: Some(edge.status.as_str().into()),
        locator,
        facets: None,
    });
    Ok(())
}

fn push_node_read(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    reason: &str,
    node: &Node,
) {
    let Some(command) = node_read_command(node) else {
        return;
    };
    push_raw_read(ctx, seen, reason, &command);
}

fn push_edge_read(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    reason: &str,
    edge: &Edge,
) {
    push_raw_read(ctx, seen, reason, &format!("loom edge show {}", edge.id));
}

fn push_raw_read(
    ctx: &mut TraversalContext,
    seen: &mut std::collections::BTreeSet<String>,
    reason: &str,
    command: &str,
) {
    if seen.insert(command.into()) {
        ctx.suggested_reads.push(SuggestedRead {
            reason: reason.into(),
            command: command.into(),
        });
    }
}

fn node_read_command(node: &Node) -> Option<String> {
    let cmd = match node.node_type {
        NodeType::Intent => format!("loom intent show {}", node.id),
        NodeType::CodeFile => format!("loom codefile show {}", node.id),
        NodeType::QualityRule => format!("loom rule show {}", node.id),
        NodeType::Validation => format!("loom validation show {}", node.id),
        NodeType::Hypothesis => format!("loom hypothesis show {}", node.id),
        NodeType::Finding => "loom finding list".into(),
        NodeType::InboxItem => format!("loom inbox show {}", node.id),
        _ => return None,
    };
    Some(cmd)
}
