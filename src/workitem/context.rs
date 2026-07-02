use super::{FileRead, LinkedEntity, SuggestedRead, TraversalContext};
use crate::model::{Edge, EdgeKind, Node, NodeType};
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
        // Coverage-lane packets are about one concrete file: name it directly.
        NodeType::CodeFile => ctx.read_set.push(FileRead {
            path: node.name.clone(),
            locator: None,
            why: "the file this work item is about — read it before deciding".into(),
        }),
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
    Ok(ctx)
}

fn push_grounded_codefiles(
    store: &Store,
    ctx: &mut TraversalContext,
    seen_entities: &mut std::collections::BTreeSet<String>,
    seen_reads: &mut std::collections::BTreeSet<String>,
    intent: &Node,
) -> Result<()> {
    for edge in store
        .edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)?
        .into_iter()
        .take(8)
    {
        push_edge_entity(store, ctx, seen_entities, "grounding_edge", &edge)?;
        if let Some(cf) = store.get_node(&edge.to_id)? {
            push_node_entity(ctx, seen_entities, "grounded_codefile", &cf);
            push_node_read(ctx, seen_reads, "inspect grounded codefile", &cf);
        }
    }
    push_intent_read_set(store, ctx, intent)
}

/// Append the intent's grounded files (path + implements locator) to the
/// packet's read set. This is what makes a packet self-contained: the worker
/// opens these files directly instead of running follow-up show commands.
fn push_intent_read_set(store: &Store, ctx: &mut TraversalContext, intent: &Node) -> Result<()> {
    for edge in store
        .edges_with(Some(EdgeKind::Implements), Some(&intent.id), None)?
        .into_iter()
        .take(8)
    {
        let Some(cf) = store.get_node(&edge.to_id)? else {
            continue;
        };
        if ctx.read_set.iter().any(|r| r.path == cf.name) {
            continue;
        }
        let locator = store.get_facet(&edge.id, crate::model::TargetKind::Edge, "locator")?;
        ctx.read_set.push(FileRead {
            path: cf.name.clone(),
            locator,
            why: format!("grounds intent '{}'", intent.name),
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
    let primary = matches!(role, "target" | "from" | "to" | "grounded_codefile");
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
    });
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
