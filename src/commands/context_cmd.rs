//! Context command — read-only, one-screen packets for Journey, code, and Intent work.
//!
//! Plane: CLI read assembly. Resolves a target and composes facts already in
//! the graph into `TraversalContext`; it never writes, infers, or certifies
//! truth beyond plainly reporting stored state.

use super::open_read;
use crate::model::{Edge, EdgeKind, InspectionStatus, Node, NodeType, TargetKind};
use crate::store::Store;
use crate::workitem::{FileRead, LinkedEntity, SuggestedRead, TraversalContext};
use crate::Result;
use anyhow::bail;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize)]
pub(crate) struct ContextPacket {
    /// Identifies this serving of the packet. Minted at the boundary where the
    /// packet leaves the process (CLI or MCP), journaled as `packet_served`, so
    /// a later verified write can be traced back to the context that informed
    /// it. Absent when the packet is assembled but not served.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) packet_id: Option<String>,
    target: LinkedEntity,
    context: TraversalContext,
    #[serde(skip_serializing_if = "Option::is_none")]
    completeness: Option<crate::completeness::Scorecard>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    staleness_flags: Vec<String>,
}

impl ContextPacket {
    pub(crate) fn target_id(&self) -> &str {
        &self.target.id
    }
}

/// Resolve an Intent first, then a Journey, then an exact registered codefile
/// path, then the same keyword score used by `loom door`. A query has no
/// fabricated target: its closest Journey/Intent candidates are the packet's
/// targets.
pub(crate) fn context_cmd(graph: Option<&std::path::Path>, input: &str, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    render(served_context(&store, input)?, json)
}

/// Assemble a packet AND record that it was served. Both the CLI and the
/// `loom_context` MCP tool go through here, so the efficacy denominator counts
/// every real serving exactly once.
pub(crate) fn served_context(store: &Store, input: &str) -> Result<ContextPacket> {
    let mut packet = context_packet(store, input)?;
    packet.packet_id = Some(crate::packet::serve_one(
        store,
        "context",
        packet.target_id(),
    )?);
    Ok(packet)
}

/// Assemble the packet without serving it. The one implementation behind both
/// `loom context` and the `loom_context` MCP tool — a second assembler is how
/// the text and `--json` surfaces drifted apart in the first place.
pub(crate) fn context_packet(store: &Store, input: &str) -> Result<ContextPacket> {
    let input = input.trim();
    if input.is_empty() {
        bail!("context needs a Journey, Intent, registered codefile path, or query");
    }

    if let Ok(intent) = store.resolve_node(input, Some(NodeType::Intent)) {
        return node_packet(store, intent, "intent");
    }
    if let Ok(journey) = store.resolve_node(input, Some(NodeType::Journey)) {
        return node_packet(store, journey, "Journey");
    }
    if let Some(file) = store
        .codefiles()?
        .into_iter()
        .find(|file| file.name == input)
    {
        return node_packet(store, file, "codefile");
    }

    let hits =
        super::discover_cmd::keyword_hits(store, input, &[NodeType::Journey, NodeType::Intent], 5)?;
    if hits.is_empty() {
        bail!(
            "could not resolve '{input}' as a Journey, Intent, registered codefile path, or related query"
        );
    }
    let mut context = empty_context(format!(
        "Closest Journey/Intent context for query '{input}'"
    ));
    let mut seen_edges = BTreeSet::new();
    let mut staleness_flags = Vec::new();
    for (_, _, _, id) in hits {
        let Some(node) = store.get_node(&id)? else {
            continue;
        };
        match node.node_type {
            NodeType::Journey => append_journey(
                store,
                &node,
                "query_match",
                &mut context,
                &mut seen_edges,
                &mut staleness_flags,
            )?,
            NodeType::Intent => append_intent(
                store,
                &node,
                "query_match",
                &mut context,
                &mut seen_edges,
                &mut staleness_flags,
            )?,
            _ => unreachable!("query is restricted to Journey and Intent nodes"),
        }
    }
    let target = LinkedEntity {
        role: "query".into(),
        kind: "query".into(),
        id: format!("query:{input}"),
        name: input.into(),
        description: None,
        status: None,
        edge_kind: None,
        edge_status: None,
        locator: None,
        facets: None,
    };
    Ok(ContextPacket {
        packet_id: None,
        target,
        context,
        completeness: None,
        staleness_flags,
    })
}

fn node_packet(store: &Store, node: Node, target_kind: &str) -> Result<ContextPacket> {
    let mut context = empty_context(format!(
        "Read-only context for {target_kind} '{}'",
        node.name
    ));
    let mut seen_edges = BTreeSet::new();
    let mut staleness_flags = Vec::new();
    let target = match node.node_type {
        NodeType::Intent => {
            append_intent(
                store,
                &node,
                "target",
                &mut context,
                &mut seen_edges,
                &mut staleness_flags,
            )?;
            intent_entity(store, "target", &node)?
        }
        NodeType::Journey => {
            append_journey(
                store,
                &node,
                "target",
                &mut context,
                &mut seen_edges,
                &mut staleness_flags,
            )?;
            node_entity("target", &node)
        }
        NodeType::CodeFile => {
            append_file(
                store,
                &node,
                "target",
                &mut context,
                &mut seen_edges,
                &mut staleness_flags,
            )?;
            node_entity("target", &node)
        }
        _ => unreachable!("context resolves only Journey/Intent/codefile nodes"),
    };
    let completeness = (node.node_type == NodeType::Intent)
        .then(|| crate::completeness::scorecard(store, &node))
        .transpose()?;
    Ok(ContextPacket {
        packet_id: None,
        target,
        context,
        completeness,
        staleness_flags,
    })
}

fn append_journey(
    store: &Store,
    journey: &Node,
    role: &str,
    context: &mut TraversalContext,
    seen_edges: &mut BTreeSet<String>,
    staleness_flags: &mut Vec<String>,
) -> Result<()> {
    push_entity(context, node_entity(role, journey));
    push_suggested_read(
        context,
        "inspect authored Journey root",
        format!("loom journey show {}", journey.id),
    );
    for edge in touching_edges(store, &journey.id)? {
        let edge_role = match edge.kind {
            EdgeKind::Derives if edge.from_id == journey.id => "derived_intent",
            EdgeKind::Surfaces if edge.from_id == journey.id => "surface",
            EdgeKind::Proves if edge.to_id == journey.id => "proof",
            EdgeKind::Questions if edge.to_id == journey.id => "open_question",
            _ => "related",
        };
        push_edge(
            store,
            context,
            edge_role,
            &edge,
            seen_edges,
            staleness_flags,
        )?;
        let other_id = if edge.from_id == journey.id {
            &edge.to_id
        } else {
            &edge.from_id
        };
        if let Some(other) = store.get_node(other_id)? {
            let role = match other.node_type {
                NodeType::Intent => "derived_intent",
                NodeType::InterfaceSurface => "surface",
                NodeType::Validation => "validation",
                NodeType::Question if other.status == "open" => "open_question",
                _ => "related",
            };
            push_entity(context, node_entity(role, &other));
        }
    }
    append_notes(store, &journey.id, context)?;
    Ok(())
}

fn empty_context(purpose: String) -> TraversalContext {
    TraversalContext {
        purpose,
        linked_entities: Vec::new(),
        suggested_reads: Vec::new(),
        read_set: Vec::new(),
    }
}

fn append_intent(
    store: &Store,
    intent: &Node,
    role: &str,
    context: &mut TraversalContext,
    seen_edges: &mut BTreeSet<String>,
    staleness_flags: &mut Vec<String>,
) -> Result<()> {
    push_entity(context, intent_entity(store, role, intent)?);
    push_suggested_read(
        context,
        "inspect intent criterion",
        format!("loom intent show {}", intent.id),
    );

    for edge in touching_edges(store, &intent.id)? {
        let edge_role = match edge.kind {
            EdgeKind::Implements if edge.from_id == intent.id => "grounding",
            EdgeKind::Validates if edge.to_id == intent.id => "proof",
            EdgeKind::Governs if edge.to_id == intent.id => "quality_rule",
            EdgeKind::Questions if edge.to_id == intent.id => "open_question",
            _ => "related",
        };
        push_edge(
            store,
            context,
            edge_role,
            &edge,
            seen_edges,
            staleness_flags,
        )?;
        let other_id = if edge.from_id == intent.id {
            &edge.to_id
        } else {
            &edge.from_id
        };
        if let Some(other) = store.get_node(other_id)? {
            match edge.kind {
                EdgeKind::Implements if other.node_type == NodeType::CodeFile => {
                    push_entity(context, node_entity("grounding_file", &other));
                    push_file_read(
                        store,
                        context,
                        &other,
                        edge_locator(store, &edge)?,
                        "grounded implementation",
                    );
                }
                EdgeKind::Validates if other.node_type == NodeType::Validation => {
                    push_entity(context, node_entity("validation", &other));
                    push_suggested_read(
                        context,
                        "inspect proof definition and last result",
                        format!("loom validation show {}", other.id),
                    );
                }
                EdgeKind::Governs if other.node_type == NodeType::QualityRule => {
                    push_entity(context, node_entity("quality_rule", &other));
                    push_suggested_read(
                        context,
                        "inspect applicable quality rule",
                        format!("loom rule show {}", other.id),
                    );
                }
                EdgeKind::Questions
                    if other.node_type == NodeType::Question && other.status == "open" =>
                {
                    push_entity(context, node_entity("open_question", &other));
                }
                _ if other.node_type == NodeType::Intent => {
                    push_entity(context, intent_entity(store, "related_intent", &other)?);
                }
                _ => {}
            }
        }
    }
    append_notes(store, &intent.id, context)?;
    Ok(())
}

fn append_file(
    store: &Store,
    file: &Node,
    role: &str,
    context: &mut TraversalContext,
    seen_edges: &mut BTreeSet<String>,
    staleness_flags: &mut Vec<String>,
) -> Result<()> {
    push_entity(context, node_entity(role, file));
    push_file_read(store, context, file, None, "registered codefile target");
    push_suggested_read(
        context,
        "inspect registered codefile",
        format!("loom codefile show {}", file.id),
    );
    for edge in touching_edges(store, &file.id)? {
        let edge_role = if edge.kind == EdgeKind::Implements {
            "grounding"
        } else if edge.kind == EdgeKind::Exercises {
            "validation_evidence"
        } else {
            "related"
        };
        push_edge(
            store,
            context,
            edge_role,
            &edge,
            seen_edges,
            staleness_flags,
        )?;
        let other_id = if edge.from_id == file.id {
            &edge.to_id
        } else {
            &edge.from_id
        };
        if let Some(other) = store.get_node(other_id)? {
            if other.node_type == NodeType::Intent {
                append_intent(
                    store,
                    &other,
                    "owning_intent",
                    context,
                    seen_edges,
                    staleness_flags,
                )?;
            } else if other.node_type == NodeType::Validation {
                push_entity(context, node_entity("validation", &other));
            }
        }
    }
    append_notes(store, &file.id, context)
}

fn append_notes(store: &Store, target_id: &str, context: &mut TraversalContext) -> Result<()> {
    for note in store.notes_for(target_id)?.into_iter().take(6) {
        let role = match note.status.as_str() {
            "warning" => "warning",
            "decision" => "decision",
            _ => "note",
        };
        let mut entity = node_entity(role, &note);
        // A decision's NAME is `note:decision`, which told a reader nothing —
        // the packet rendered a column of identical blank labels where the
        // reasoning was supposed to be. Show the reversal instead.
        if let (Some(chose), Some(instead)) = (
            note.body.get("chose").and_then(|v| v.as_str()),
            note.body.get("instead_of").and_then(|v| v.as_str()),
        ) {
            entity.name = format!("{chose} — instead of {instead}");
        }
        push_entity(context, entity);
    }
    Ok(())
}

fn touching_edges(store: &Store, id: &str) -> Result<Vec<Edge>> {
    let mut edges = store.edges_with(None, Some(id), None)?;
    edges.extend(store.edges_with(None, None, Some(id))?);
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    edges.dedup_by(|a, b| a.id == b.id);
    Ok(edges)
}

fn push_edge(
    store: &Store,
    context: &mut TraversalContext,
    role: &str,
    edge: &Edge,
    seen_edges: &mut BTreeSet<String>,
    staleness_flags: &mut Vec<String>,
) -> Result<()> {
    if seen_edges.insert(edge.id.clone()) {
        let locator = edge_locator(store, edge)?;
        context.linked_entities.push(LinkedEntity {
            role: role.into(),
            kind: "edge".into(),
            id: edge.id.clone(),
            name: format!("{} {} {}", edge.from_id, edge.kind.as_str(), edge.to_id),
            description: Some(edge.criterion.clone()).filter(|criterion| !criterion.is_empty()),
            status: None,
            edge_kind: Some(edge.kind.as_str().into()),
            edge_status: Some(edge.status.as_str().into()),
            locator,
            facets: None,
        });
        push_suggested_read(
            context,
            "inspect linked edge",
            format!("loom edge show {}", edge.id),
        );
    }
    if matches!(
        edge.status,
        InspectionStatus::NeedsReverification | InspectionStatus::Failing
    ) {
        let flag = format!(
            "{} edge {} is {}",
            edge.kind.as_str(),
            crate::model::short(&edge.id),
            edge.status.as_str()
        );
        if !staleness_flags.contains(&flag) {
            staleness_flags.push(flag);
        }
    }
    Ok(())
}

fn intent_entity(store: &Store, role: &str, intent: &Node) -> Result<LinkedEntity> {
    let mut facets = BTreeMap::new();
    for key in [
        "origin",
        "level",
        "visibility",
        "aspect",
        "ratified_by",
        "ratified_at",
    ] {
        if let Some(value) = store.get_facet(&intent.id, TargetKind::Node, key)? {
            facets.insert(key.into(), value);
        }
    }
    facets.insert(
        "ratification".into(),
        store
            .ratification(&intent.id)
            .map(Some)?
            .unwrap_or_else(|| "unratified".into()),
    );
    Ok(LinkedEntity {
        facets: Some(facets),
        ..node_entity(role, intent)
    })
}

fn node_entity(role: &str, node: &Node) -> LinkedEntity {
    LinkedEntity {
        role: role.into(),
        kind: node.node_type.as_str().into(),
        id: node.id.clone(),
        name: node.name.clone(),
        description: Some(node.description.clone()).filter(|description| !description.is_empty()),
        status: Some(node.status.clone()).filter(|status| !status.is_empty()),
        edge_kind: None,
        edge_status: None,
        locator: None,
        facets: None,
    }
}

fn edge_locator(store: &Store, edge: &Edge) -> Result<Option<String>> {
    store.edge_locator(&edge.id)
}

fn push_entity(context: &mut TraversalContext, entity: LinkedEntity) {
    if !context
        .linked_entities
        .iter()
        .any(|existing| existing.id == entity.id && existing.role == entity.role)
    {
        context.linked_entities.push(entity);
    }
}

fn push_suggested_read(context: &mut TraversalContext, reason: &str, command: String) {
    if !context
        .suggested_reads
        .iter()
        .any(|read| read.command == command)
    {
        context.suggested_reads.push(SuggestedRead {
            reason: reason.into(),
            command,
        });
    }
}

fn push_file_read(
    store: &Store,
    context: &mut TraversalContext,
    file: &Node,
    locator: Option<String>,
    why: &str,
) {
    if !store.root().join(&file.name).exists()
        || context
            .read_set
            .iter()
            .any(|read| read.path == file.name && read.locator == locator)
    {
        return;
    }
    context.read_set.push(FileRead {
        path: file.name.clone(),
        locator,
        why: why.into(),
    });
}

fn render(packet: ContextPacket, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&packet)?);
        return Ok(());
    }
    println!(
        "{} [{}]",
        packet.target.name,
        crate::model::short(&packet.target.id)
    );
    if let Some(description) = &packet.target.description {
        println!("  criterion: {description}");
    }
    if let Some(status) = &packet.target.status {
        println!("  lifecycle: {status}");
    }
    if let Some(facets) = &packet.target.facets {
        println!(
            "  ratification: {}",
            facets
                .get("ratification")
                .map(String::as_str)
                .unwrap_or("unratified")
        );
    }
    for (heading, roles) in [
        (
            "intents",
            &["owning_intent", "related_intent", "query_match"] as &[&str],
        ),
        ("groundings", &["grounding", "grounding_file"]),
        ("proofs", &["proof", "validation"]),
        ("quality", &["quality_rule"]),
        ("decisions", &["decision", "warning", "note"]),
        ("open questions", &["open_question"]),
    ] {
        let rows: Vec<_> = packet
            .context
            .linked_entities
            .iter()
            .filter(|entity| roles.contains(&entity.role.as_str()))
            .take(6)
            .collect();
        if rows.is_empty() {
            continue;
        }
        println!("  {heading}:");
        for row in rows {
            let suffix = row
                .edge_status
                .as_deref()
                .or(row.status.as_deref())
                .map(|status| format!(" [{status}]"))
                .unwrap_or_default();
            let locator = row
                .locator
                .as_deref()
                .map(|locator| format!(" @ {locator}"))
                .unwrap_or_default();
            println!("    {}{}{}", row.name, locator, suffix);
        }
    }
    if let Some(scorecard) = &packet.completeness {
        println!("  completeness: {} open axis/axes", scorecard.open);
    }
    if !packet.staleness_flags.is_empty() {
        println!("  staleness:");
        for flag in &packet.staleness_flags {
            println!("    {flag}");
        }
    }
    Ok(())
}
