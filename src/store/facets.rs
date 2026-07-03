use super::*;

impl Store {
    // ---- facets / tags ---------------------------------------------------

    pub fn set_facet(
        &self,
        target_id: &str,
        target_kind: TargetKind,
        key: &str,
        value: &str,
        truth_class: TruthClass,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO facet(target_id,target_kind,key,value,truth_class)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(target_id,target_kind,key)
             DO UPDATE SET value=?4, truth_class=?5",
            params![
                target_id,
                target_kind.as_str(),
                key,
                value,
                truth_class.as_str()
            ],
        )?;
        Ok(())
    }

    /// Remove a single facet. Used when a registered file disappears: its
    /// derived `content_hash` must go so the incremental path and a clean
    /// wipe+rebuild converge to the same state (INV-2).
    pub fn clear_facet(&self, target_id: &str, target_kind: TargetKind, key: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind=?2 AND key=?3",
            params![target_id, target_kind.as_str(), key],
        )?;
        Ok(())
    }

    pub fn set_tag(&self, target_id: &str, target_kind: TargetKind, term: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO tag(target_id,target_kind,term) VALUES (?1,?2,?3)",
            params![target_id, target_kind.as_str(), term],
        )?;
        Ok(())
    }

    pub fn remove_tag(&self, target_id: &str, target_kind: TargetKind, term: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM tag WHERE target_id=?1 AND target_kind=?2 AND term=?3",
            params![target_id, target_kind.as_str(), term],
        )?;
        Ok(())
    }

    /// Tags on a target.
    pub fn tags_of(&self, target_id: &str, target_kind: TargetKind) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT term FROM tag WHERE target_id=?1 AND target_kind=?2 ORDER BY term")?;
        let rows = stmt.query_map(params![target_id, target_kind.as_str()], |r| {
            r.get::<_, String>(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- snapshot / restore (export / import) ----------------------------

    /// Read the whole graph as a deterministic snapshot (sorted collections).
    pub fn snapshot(&self) -> Result<Snapshot> {
        let identity = self.identity()?;
        let nodes = self.list_all_nodes()?;
        let edges = self.list_edges(None, usize::MAX)?;
        let facets = self.list_all_facets()?;
        let tags = self.list_all_tags()?;
        let mut config = std::collections::BTreeMap::new();
        for key in PORTABLE_META_KEYS {
            if let Some(v) = self.get_meta(key)? {
                config.insert((*key).to_string(), v);
            }
        }
        Ok(Snapshot {
            identity,
            nodes,
            edges,
            facets,
            tags,
            config,
        })
    }

    fn list_all_nodes(&self) -> Result<Vec<Node>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,node_type,name,description,status,truth_class,body,created_at,updated_at
             FROM node ORDER BY id",
        )?;
        let rows = stmt.query_map([], row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn list_all_facets(&self) -> Result<Vec<Facet>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_id,target_kind,key,value,truth_class
             FROM facet ORDER BY target_id,target_kind,key",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Facet {
                target_id: r.get(0)?,
                target_kind: parse_col(r, 1)?,
                key: r.get(2)?,
                value: r.get(3)?,
                truth_class: parse_col(r, 4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn list_all_tags(&self) -> Result<Vec<Tag>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_id,target_kind,term FROM tag ORDER BY target_id,target_kind,term",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Tag {
                target_id: r.get(0)?,
                target_kind: parse_col(r, 1)?,
                term: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Restore a snapshot into a freshly-initialized store. Refuses to overwrite
    /// a non-empty graph. Two-phase: validate fully, then write in one txn.
    pub fn restore(&mut self, snap: &Snapshot) -> Result<()> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM node", [], |r| r.get(0))?;
        if count > 0 {
            bail!("refusing to import into a non-empty graph ({count} nodes present)");
        }
        // Phase 1: validate every edge against the registry before any write.
        let node_types: std::collections::HashMap<&str, NodeType> = snap
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.node_type))
            .collect();
        for e in &snap.edges {
            let spec = registry::spec(e.kind);
            if !spec.allows_truth_class(e.truth_class) {
                bail!(
                    "import: edge '{}' kind '{}' disallows truth_class '{}'",
                    e.id,
                    e.kind,
                    e.truth_class
                );
            }
            let ft = node_types.get(e.from_id.as_str()).ok_or_else(|| {
                anyhow!("import: edge '{}' from-node '{}' missing", e.id, e.from_id)
            })?;
            let tt = node_types
                .get(e.to_id.as_str())
                .ok_or_else(|| anyhow!("import: edge '{}' to-node '{}' missing", e.id, e.to_id))?;
            if *ft != spec.from || *tt != spec.to {
                bail!(
                    "import: edge '{}' has endpoints violating kind '{}'",
                    e.id,
                    e.kind
                );
            }
        }
        // Facets/tags reference a node or edge by (target_id, target_kind), but
        // the schema has no FK on target_id — an orphaned facet/tag would import
        // silently (M-14). Validate every target against the imported nodes/edges.
        let edge_ids: std::collections::HashSet<&str> =
            snap.edges.iter().map(|e| e.id.as_str()).collect();
        let has_target = |id: &str, kind: TargetKind| match kind {
            TargetKind::Node => node_types.contains_key(id),
            TargetKind::Edge => edge_ids.contains(id),
        };
        for f in &snap.facets {
            if !has_target(&f.target_id, f.target_kind) {
                bail!(
                    "import: facet '{}' on {} '{}' references a missing target",
                    f.key,
                    f.target_kind,
                    f.target_id
                );
            }
        }
        for t in &snap.tags {
            if !has_target(&t.target_id, t.target_kind) {
                bail!(
                    "import: tag '{}' on {} '{}' references a missing target",
                    t.term,
                    t.target_kind,
                    t.target_id
                );
            }
        }
        // Phase 2: write everything in one transaction.
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES ('graph_id',?1)
             ON CONFLICT(key) DO UPDATE SET value=?1",
            params![snap.identity.graph_id],
        )?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES ('name',?1)
             ON CONFLICT(key) DO UPDATE SET value=?1",
            params![snap.identity.name],
        )?;
        tx.execute(
            "INSERT INTO meta(key,value) VALUES ('observed',?1)
             ON CONFLICT(key) DO UPDATE SET value=?1",
            params![if snap.identity.observed { "1" } else { "0" }],
        )?;
        for n in &snap.nodes {
            tx.execute(
                "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    n.id, n.node_type.as_str(), n.name, n.description, n.status,
                    n.truth_class.as_str(), n.body.to_string(), n.created_at, n.updated_at
                ],
            )?;
        }
        for e in &snap.edges {
            tx.execute(
                "INSERT INTO edge(id,from_id,to_id,kind,truth_class,status,criterion,evidence,
                        confidence,depends_on,inspected_by,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    e.id,
                    e.from_id,
                    e.to_id,
                    e.kind.as_str(),
                    e.truth_class.as_str(),
                    e.status.as_str(),
                    e.criterion,
                    e.evidence,
                    e.confidence,
                    e.depends_on.to_string(),
                    e.inspected_by,
                    e.created_at,
                    e.updated_at
                ],
            )?;
        }
        for f in &snap.facets {
            tx.execute(
                "INSERT INTO facet(target_id,target_kind,key,value,truth_class)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    f.target_id,
                    f.target_kind.as_str(),
                    f.key,
                    f.value,
                    f.truth_class.as_str()
                ],
            )?;
        }
        for t in &snap.tags {
            tx.execute(
                "INSERT INTO tag(target_id,target_kind,term) VALUES (?1,?2,?3)",
                params![t.target_id, t.target_kind.as_str(), t.term],
            )?;
        }
        for (key, value) in &snap.config {
            if !PORTABLE_META_KEYS.contains(&key.as_str()) {
                bail!("import: config key '{key}' is not portable");
            }
            tx.execute(
                "INSERT INTO meta(key,value) VALUES (?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=?2",
                params![key, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

}
