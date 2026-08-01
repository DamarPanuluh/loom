//! Travel format — deterministic export / two-phase import.
//!
//! Plane: serialization only. The store produces a sorted `Snapshot`; this
//! module turns it into bytes and back. Determinism contract: the same graph
//! exports to byte-identical JSON, so `loom.graph.json` diffs cleanly in PRs and
//! `loom export --check` can gate CI.

use crate::model::{Edge, Facet, Node, NodeType, Tag};
use crate::store::{Identity, Snapshot, Store};
use crate::Result;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The on-disk export envelope. Field order here is the byte order in the file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Export {
    pub format: u32,
    pub graph_id: String,
    pub name: String,
    pub observed: bool,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// The asserted claims and their anchors. Empty on a graph with no verdicts,
    /// so a bare structural export keeps its exact byte shape.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<crate::evidence::Fact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<crate::evidence::EvidenceRow>,
    pub facets: Vec<Facet>,
    pub tags: Vec<Tag>,
    /// Portable repo config (allowlisted meta keys: layer order, coverage
    /// ignores, codefile globs, scan adapters). Absent when empty, so graphs
    /// without config keep their exact pre-config byte format.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub config: std::collections::BTreeMap<String, String>,
}

/// Current export format version.
pub const FORMAT: u32 = 2;

impl Export {
    pub fn from_snapshot(snap: Snapshot) -> Export {
        Export {
            format: FORMAT,
            graph_id: snap.identity.graph_id,
            name: snap.identity.name,
            observed: snap.identity.observed,
            nodes: snap.nodes,
            edges: snap.edges,
            facts: snap.facts,
            evidence: snap.evidence,
            facets: snap.facets,
            tags: snap.tags,
            config: snap.config,
        }
    }

    pub fn into_snapshot(self) -> Snapshot {
        Snapshot {
            facts: self.facts,
            evidence: self.evidence,
            identity: Identity {
                graph_id: self.graph_id,
                name: self.name,
                schema_version: crate::SCHEMA_VERSION,
                observed: self.observed,
            },
            nodes: self.nodes,
            edges: self.edges,
            facets: self.facets,
            tags: self.tags,
            config: self.config,
        }
    }

    /// Serialize to deterministic, pretty JSON with a trailing newline.
    pub fn to_json(&self) -> Result<String> {
        let mut s = serde_json::to_string_pretty(self).context("serializing export")?;
        s.push('\n');
        Ok(s)
    }

    pub fn from_json(text: &str) -> Result<Export> {
        // Two-phase import begins here: parse fully (and loudly) before the
        // store ever sees it. A malformed export never leaves a partial graph.
        let export: Export =
            serde_json::from_str(text).context("parsing export (malformed loom.graph.json)")?;
        // Reject a format this loom does not speak, rather than silently
        // restoring it as the current schema (M-7).
        if export.format != FORMAT {
            anyhow::bail!(
                "export format version {} is unsupported (this loom speaks format {FORMAT}) — upgrade loom or re-export",
                export.format
            );
        }
        Ok(export)
    }
}

/// Export a store's graph to the canonical `loom.graph.json` at the project root.
pub fn export_to_file(store: &Store) -> Result<std::path::PathBuf> {
    let proj = graph_projection()?;
    let json = proj.render(store.snapshot()?)?;
    let path = store.root().join(proj.artifact_path());
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Compute whether the committed export at `root` matches the live graph.
/// Returns Ok(true) when fresh, Ok(false) when drifted or missing.
pub fn export_is_fresh(store: &Store) -> Result<bool> {
    let proj = graph_projection()?;
    let path = store.root().join(proj.artifact_path());
    let committed = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        // A missing export is honest drift; a real IO error (permissions, etc.)
        // must surface, not masquerade as "not fresh" (L-1).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let live = proj.render(store.snapshot()?)?;
    Ok(committed == live)
}

/// Refresh the committed export ONLY if it already exists and has drifted from
/// the live graph. Returns whether it was rewritten. This makes a fresh
/// `loom.graph.json` a byproduct of `loom sync` (one fewer command in the loop)
/// without ever creating an artifact a repo chose not to track, and without
/// rewriting a fresh one (so determinism and clean diffs hold).
pub fn refresh_export_if_tracked(store: &Store) -> Result<bool> {
    let path = store.root().join(graph_projection()?.artifact_path());
    if !path.exists() || export_is_fresh(store)? {
        return Ok(false);
    }
    export_to_file(store)?;
    Ok(true)
}

/// Read an export file from disk and parse it (phase 1 of import).
pub fn read_export(path: &Path) -> Result<Export> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Export::from_json(&text)
}

/// Quarantine executable text crossing the import trust boundary. The graph
/// structure still restores atomically, but imported validation and scan
/// commands cannot execute until an operator re-enters each exact command via
/// the corresponding local update command.
///
/// The upstream-graph registry is quarantined the same way: its paths point at
/// the EXPORTER's filesystem, so restoring it would aim the next sync's
/// federation reader at foreign paths the importer never chose. Dropping it
/// leaves the shadow nodes as orphans until an operator deliberately
/// re-`graph link`s each upstream locally.
pub fn quarantine_imported_execution(snap: &mut Snapshot) -> Result<usize> {
    let mut quarantined = 0;
    for node in &mut snap.nodes {
        if node.node_type != NodeType::Validation {
            continue;
        }
        let has_command = node
            .body
            .get("command")
            .and_then(|value| value.as_str())
            .is_some_and(|command| !command.trim().is_empty());
        if has_command {
            node.body["command_trusted"] = serde_json::Value::Bool(false);
            quarantined += 1;
        }
    }
    if let Some(raw) = snap.config.get("scan_adapters").cloned() {
        let mut adapters: Vec<serde_json::Value> = serde_json::from_str(&raw)
            .context("parsing imported scan_adapters before command quarantine")?;
        for adapter in &mut adapters {
            let has_command = adapter
                .get("command")
                .and_then(|value| value.as_str())
                .is_some_and(|command| !command.trim().is_empty());
            if has_command {
                adapter["trusted"] = serde_json::Value::Bool(false);
                quarantined += 1;
            }
        }
        snap.config.insert(
            "scan_adapters".into(),
            serde_json::to_string(&adapters).context("serializing quarantined scan_adapters")?,
        );
    }
    if snap.config.remove("upstream_graphs").is_some() {
        quarantined += 1;
    }
    Ok(quarantined)
}

// ---- projection seam -------------------------------------------------------

/// The registry key of the canonical deterministic graph export.
pub const GRAPH_JSON_PROJECTION: &str = "graph_json";

/// A projection renders the graph snapshot into a world-facing artifact. The
/// engine's export path knows only this seam and the registry below: adding a
/// new export format (an OKF bundle, a wiki page) means registering another
/// `Projection`, never editing the export command or sync's freshness check.
pub trait Projection {
    /// The projection's stable registry key.
    fn name(&self) -> &str;
    /// The committed artifact's path, relative to the project root.
    fn artifact_path(&self) -> &str;
    /// Render a snapshot to deterministic bytes.
    fn render(&self, snap: Snapshot) -> Result<String>;
}

/// The canonical deterministic JSON projection. Per the engine/seed boundary
/// decision, `loom.graph.json` is loom's reference (and only) projection; other
/// projections (OKF bundles) belong to the separate canonical engine.
pub struct GraphJsonProjection;

impl Projection for GraphJsonProjection {
    fn name(&self) -> &str {
        GRAPH_JSON_PROJECTION
    }
    fn artifact_path(&self) -> &str {
        crate::GRAPH_EXPORT
    }
    fn render(&self, snap: Snapshot) -> Result<String> {
        Export::from_snapshot(snap).to_json()
    }
}

/// Every registered projection. The engine looks projections up here by key; a
/// seed adds one by adding an entry, with no change to the export path.
pub fn projections() -> Vec<Box<dyn Projection>> {
    vec![Box::new(GraphJsonProjection)]
}

/// Look up a projection by its registry key.
pub fn projection(name: &str) -> Option<Box<dyn Projection>> {
    projections().into_iter().find(|p| p.name() == name)
}

/// The canonical graph projection, resolved through the registry — so the
/// engine's export path dispatches by key rather than hardcoding the format.
fn graph_projection() -> Result<Box<dyn Projection>> {
    projection(GRAPH_JSON_PROJECTION)
        .ok_or_else(|| anyhow::anyhow!("graph_json projection is registered"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn empty_export_is_deterministic() {
        let e = Export {
            facts: Vec::new(),
            evidence: Vec::new(),
            format: FORMAT,
            graph_id: "g1".into(),
            name: "demo".into(),
            observed: false,
            nodes: vec![],
            edges: vec![],
            facets: vec![],
            tags: vec![],
            config: Default::default(),
        };
        let json = e.to_json().unwrap();
        insta::assert_snapshot!(json, @r###"
{
  "format": 2,
  "graph_id": "g1",
  "name": "demo",
  "observed": false,
  "nodes": [],
  "edges": [],
  "facets": [],
  "tags": []
}
"###);
        let parsed = Export::from_json(&json).unwrap();
        assert_eq!(parsed, e);
    }

    #[test]
    fn config_keys_travel_and_absent_config_parses() {
        // A populated allowlisted config round-trips…
        let mut config = std::collections::BTreeMap::new();
        config.insert("layer_order".to_string(), r#"["api","domain"]"#.to_string());
        let e = Export {
            facts: Vec::new(),
            evidence: Vec::new(),
            format: FORMAT,
            graph_id: "g1".into(),
            name: "demo".into(),
            observed: false,
            nodes: vec![],
            edges: vec![],
            facets: vec![],
            tags: vec![],
            config,
        };
        let json = e.to_json().unwrap();
        assert!(json.contains("\"layer_order\""));
        assert_eq!(Export::from_json(&json).unwrap(), e);
        // …and an export without the optional sections still parses: `config`,
        // `facts` and `evidence` are all absent-when-empty, so a structural
        // export keeps its exact byte shape.
        let minimal = r#"{"format":2,"graph_id":"g","name":"n","observed":false,
                          "nodes":[],"edges":[],"facets":[],"tags":[]}"#;
        let parsed = Export::from_json(minimal).unwrap();
        assert!(parsed.config.is_empty());
        assert!(parsed.facts.is_empty());
    }

    #[test]
    fn import_quarantines_validation_and_scan_commands() {
        let mut config = std::collections::BTreeMap::new();
        config.insert(
            "scan_adapters".into(),
            r#"[{"name":"lint","command":"cargo lint"}]"#.into(),
        );
        // An imported upstream registry points at the EXPORTER's filesystem, so
        // it must be dropped on import — the next sync's federation reader must
        // only ever see locally-linked paths.
        config.insert(
            "upstream_graphs".into(),
            r#"[{"path":"/exporter/only/loom.graph.json","alias":"x","graph_id":"g2"}]"#.into(),
        );
        let mut snapshot = Snapshot {
            facts: Vec::new(),
            evidence: Vec::new(),
            identity: Identity {
                graph_id: "g".into(),
                name: "n".into(),
                schema_version: crate::SCHEMA_VERSION,
                observed: false,
            },
            nodes: vec![Node {
                id: "validation".into(),
                node_type: NodeType::Validation,
                name: "imported proof".into(),
                description: String::new(),
                status: "not_run".into(),
                truth_class: crate::model::TruthClass::Asserted,
                body: serde_json::json!({"type":"test", "command":"cargo test", "command_trusted":true}),
                created_at: String::new(),
                updated_at: String::new(),
            }],
            edges: Vec::new(),
            facets: Vec::new(),
            tags: Vec::new(),
            config,
        };

        // 1 validation + 1 scan adapter + 1 upstream registry = 3 quarantined.
        assert_eq!(quarantine_imported_execution(&mut snapshot).unwrap(), 3);
        assert_eq!(
            snapshot.nodes[0].body["command_trusted"],
            serde_json::Value::Bool(false)
        );
        let adapters: serde_json::Value =
            serde_json::from_str(&snapshot.config["scan_adapters"]).unwrap();
        assert_eq!(adapters[0]["trusted"], serde_json::Value::Bool(false));
        assert!(
            !snapshot.config.contains_key("upstream_graphs"),
            "imported upstream registry must be dropped, not restored"
        );
    }

    #[test]
    fn export_dispatches_through_the_projection_registry() {
        // The canonical projection is registered and keys are honest.
        let proj = projection(GRAPH_JSON_PROJECTION).expect("graph_json registered");
        assert_eq!(proj.name(), "graph_json");
        assert_eq!(proj.artifact_path(), crate::GRAPH_EXPORT);
        assert!(
            projection("okf_bundle").is_none(),
            "no unregistered projection"
        );

        // Rendering through the seam is byte-identical to the direct export
        // path, so routing the engine through the registry changed no bytes.
        let snap = Snapshot {
            facts: Vec::new(),
            evidence: Vec::new(),
            identity: Identity {
                graph_id: "g1".into(),
                name: "demo".into(),
                schema_version: crate::SCHEMA_VERSION,
                observed: false,
            },
            nodes: vec![],
            edges: vec![],
            facets: vec![],
            tags: vec![],
            config: Default::default(),
        };
        let via_seam = proj.render(snap.clone()).unwrap();
        let direct = Export::from_snapshot(snap).to_json().unwrap();
        assert_eq!(via_seam, direct);
    }

    proptest! {
        #[test]
        fn empty_export_roundtrips_for_generated_identity(
            graph_id in "[a-z0-9]{1,16}",
            name in "[a-z][a-z0-9 -]{0,20}",
            observed in any::<bool>(),
        ) {
            let export = Export {
                facts: Vec::new(),
                evidence: Vec::new(),
                format: FORMAT,
                graph_id,
                name,
                observed,
                nodes: vec![],
                edges: vec![],
                facets: vec![],
                tags: vec![],
                config: Default::default(),
            };

            let first = export.to_json().unwrap();
            let second = export.to_json().unwrap();
            prop_assert_eq!(&first, &second);
            prop_assert_eq!(Export::from_json(&first).unwrap(), export);
        }
    }
}
