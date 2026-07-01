//! Travel format — deterministic export / two-phase import.
//!
//! Plane: serialization only. The store produces a sorted `Snapshot`; this
//! module turns it into bytes and back. Determinism contract: the same graph
//! exports to byte-identical JSON, so `loom.graph.json` diffs cleanly in PRs and
//! `loom export --check` can gate CI.

use crate::model::{Edge, Facet, Node, Tag};
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
    pub facets: Vec<Facet>,
    pub tags: Vec<Tag>,
}

/// Current export format version.
pub const FORMAT: u32 = 1;

impl Export {
    pub fn from_snapshot(snap: Snapshot) -> Export {
        Export {
            format: FORMAT,
            graph_id: snap.identity.graph_id,
            name: snap.identity.name,
            observed: snap.identity.observed,
            nodes: snap.nodes,
            edges: snap.edges,
            facets: snap.facets,
            tags: snap.tags,
        }
    }

    pub fn into_snapshot(self) -> Snapshot {
        Snapshot {
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
        serde_json::from_str(text).context("parsing export (malformed loom.graph.json)")
    }
}

/// Export a store's graph to the canonical `loom.graph.json` at the project root.
pub fn export_to_file(store: &Store) -> Result<std::path::PathBuf> {
    let snap = store.snapshot()?;
    let export = Export::from_snapshot(snap);
    let json = export.to_json()?;
    let path = store.root().join(crate::GRAPH_EXPORT);
    std::fs::write(&path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Compute whether the committed export at `root` matches the live graph.
/// Returns Ok(true) when fresh, Ok(false) when drifted or missing.
pub fn export_is_fresh(store: &Store) -> Result<bool> {
    let path = store.root().join(crate::GRAPH_EXPORT);
    let committed = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };
    let live = Export::from_snapshot(store.snapshot()?).to_json()?;
    Ok(committed == live)
}

/// Read an export file from disk and parse it (phase 1 of import).
pub fn read_export(path: &Path) -> Result<Export> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Export::from_json(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_export_is_deterministic() {
        let e = Export {
            format: FORMAT,
            graph_id: "g1".into(),
            name: "demo".into(),
            observed: false,
            nodes: vec![],
            edges: vec![],
            facets: vec![],
            tags: vec![],
        };
        let a = e.to_json().unwrap();
        let b = e.to_json().unwrap();
        assert_eq!(a, b);
        let parsed = Export::from_json(&a).unwrap();
        assert_eq!(parsed, e);
    }
}
