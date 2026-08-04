//! Facet/tag persistence plus whole-graph snapshot and restore.
//!
//! Plane: engine (persistence). Facets are keyed key/value annotations that
//! carry their own truth class; tags are plain terms. Snapshot reads the whole
//! graph as deterministically sorted collections (stable export bytes), and
//! restore replays one atomically — neither invents, filters, or re-derives
//! truth on the way through (derived rebuild is `sync`'s job, INV-2).

use super::*;

impl Store {
    // ---- facets / tags ---------------------------------------------------

    /// Facets and tags use a polymorphic `(target_id, target_kind)` reference,
    /// which SQLite cannot express as one foreign key. Enforce that reference
    /// here so ordinary Store writes cannot create the orphan truth that import
    /// validation and `doctor` are designed to detect.
    fn require_annotation_target(&self, target_id: &str, target_kind: TargetKind) -> Result<()> {
        let exists = match target_kind {
            TargetKind::Node => self.get_node(target_id)?.is_some(),
            TargetKind::Edge => self.get_edge(target_id)?.is_some(),
        };
        if !exists {
            bail!("no {target_kind} target '{target_id}' for facet/tag write");
        }
        Ok(())
    }

    /// Facet keys that are no longer facets: they became `fact` rows, and a
    /// write here would be a second, ungated way to say the same thing.
    ///
    /// This is the door that mattered most. `adjudication` and `ratification`
    /// lived as facets, and `set_facet` is a public primitive every command
    /// reaches for — so "only a human may ratify" was enforced in
    /// `ratify_intent` and bypassed by one `set_facet` call. Naming the keys
    /// here makes the bypass impossible rather than merely discouraged.
    const RESERVED_FACET_KEYS: &'static [&'static str] = &[
        "adjudication",
        "ratification",
        "ratified_by",
        "ratified_at",
        "ratified_presence",
    ];

    pub fn set_facet(
        &self,
        target_id: &str,
        target_kind: TargetKind,
        key: &str,
        value: &str,
        truth_class: TruthClass,
    ) -> Result<()> {
        if Self::RESERVED_FACET_KEYS.contains(&key) {
            bail!(
                "'{key}' is an asserted fact, not a facet — record it through the write \
                 boundary (loom finding verdict / loom intent ratify) so it carries evidence \
                 loom can re-check"
            );
        }
        // Derived-only keys: loom computes these from the graph, so an asserted
        // write is a caller claiming an answer loom is supposed to work out.
        // `de_facto` especially — the whole point is that wantedness is EARNED
        // from evidence, and a writable `de_facto` would be a second, unchecked
        // way to declare a behavior wanted.
        if matches!(key, "de_facto" | "proof_strength" | "call_targets")
            && truth_class != TruthClass::Derived
        {
            bail!(
                "'{key}' is derived — loom computes it from the graph on sync; \
                 it cannot be asserted"
            );
        }
        self.require_annotation_target(target_id, target_kind)?;
        if target_kind == TargetKind::Edge && key == "locator" {
            if let Some(edge) = self.get_edge(target_id)? {
                if edge.kind == EdgeKind::Exemplar {
                    let file = self
                        .get_node(&edge.to_id)?
                        .ok_or_else(|| anyhow!("Exemplar target file is missing"))?;
                    if value.trim().is_empty()
                        || crate::runner::unique_locator_probe(&self.root, &file.name, value)
                            .is_none()
                    {
                        bail!(
                            "Exemplar locator must resolve exactly one live symbol in '{}'",
                            file.name
                        );
                    }
                }
            }
        }
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
        self.require_annotation_target(target_id, target_kind)?;
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

    /// Node ids tagged with `term` (any target_kind=node).
    pub fn nodes_with_tag(&self, term: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_id FROM tag WHERE term=?1 AND target_kind='node' ORDER BY target_id",
        )?;
        let rows = stmt.query_map(params![term], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Node ids with facet `key=value` (target_kind=node).
    pub fn nodes_where_facet(&self, key: &str, value: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT target_id FROM facet WHERE key=?1 AND value=?2 AND target_kind='node' ORDER BY target_id",
        )?;
        let rows = stmt.query_map(params![key, value], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---- snapshot / restore (export / import) ----------------------------

    /// Read the whole graph as a deterministic snapshot (sorted collections).
    pub fn snapshot(&self) -> Result<Snapshot> {
        let identity = self.identity()?;
        let nodes = self.list_all_nodes()?;
        let edges = self.list_edges(None, usize::MAX)?;
        let facts = self.all_facts()?;
        let mut evidence = Vec::new();
        for f in &facts {
            evidence.extend(self.evidence_for(&f.id)?);
        }
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
            facts,
            evidence,
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
    ///
    /// Strict by default: a facet/tag whose target node/edge is absent from the
    /// snapshot is an error (M-14 — an orphan would otherwise import silently),
    /// with ONE deliberate exception — asserted `adjudication` verdicts on a
    /// derived Finding id. That target re-materializes (deterministic id) on the
    /// next `sync`, so the verdict is a valid soft reference, not corruption;
    /// keeping it is what lets an export round-trip through import. Rejecting it
    /// was the version-incompatibility that made committed exports unimportable.
    /// For genuinely dangling orphans use [`Store::restore_repairing`].
    pub fn restore(&mut self, snap: &Snapshot) -> Result<()> {
        self.restore_inner(snap, false).map(|_| ())
    }

    /// Like [`Store::restore`], but instead of refusing a true orphan facet/tag
    /// it drops the orphan and records it in the returned [`RestoreReport`].
    /// The recovery path (`loom import --repair-orphans`) for a legacy or
    /// cross-version export whose targets no longer resolve. Soft refs are still
    /// preserved — repair only removes references that can never re-attach.
    pub fn restore_repairing(&mut self, snap: &Snapshot) -> Result<RestoreReport> {
        self.restore_inner(snap, true)
    }

    fn restore_inner(&mut self, snap: &Snapshot, repair: bool) -> Result<RestoreReport> {
        /// A facet that may legitimately reference a not-yet-materialized target:
        /// an asserted `adjudication` verdict on a derived Finding id. Sync
        /// re-creates the finding and the verdict re-attaches, so importing it
        /// against an absent target is correct behavior, never corruption.
        fn is_soft_ref_facet(f: &Facet) -> bool {
            f.target_kind == TargetKind::Node
                && f.truth_class == TruthClass::Asserted
                && f.key == "adjudication"
                && super::is_derived_node_id(&f.target_id)
        }

        fn is_soft_ref_fact(f: &crate::evidence::Fact) -> bool {
            f.subject_kind == TargetKind::Node
                && f.claim == Claim::Adjudication
                && super::is_derived_node_id(&f.subject_id)
        }

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
        for n in &snap.nodes {
            crate::pattern::validate_node_body(n.node_type, &n.body)
                .with_context(|| format!("import: invalid body for node '{}'", n.id))?;
            crate::research::validate_record(None, n, chrono::Utc::now(), true)
                .with_context(|| format!("import: invalid governed research node '{}'", n.id))?;
            if !registry::node_allows_truth_class(n.node_type, n.truth_class) {
                bail!(
                    "import: node '{}' type '{}' disallows truth_class '{}'",
                    n.id,
                    n.node_type,
                    n.truth_class
                );
            }
        }
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
            if e.kind == EdgeKind::Exemplar {
                let locators: Vec<_> = snap
                    .facets
                    .iter()
                    .filter(|f| {
                        f.target_kind == TargetKind::Edge
                            && f.target_id == e.id
                            && f.key == "locator"
                            && !f.value.trim().is_empty()
                    })
                    .collect();
                if locators.len() != 1 {
                    bail!(
                        "import: Exemplar edge '{}' requires exactly one nonempty locator facet",
                        e.id
                    );
                }
                let file = snap
                    .nodes
                    .iter()
                    .find(|node| node.id == e.to_id && node.node_type == NodeType::CodeFile)
                    .ok_or_else(|| {
                        anyhow!("import: Exemplar '{}' has no CodeFile endpoint", e.id)
                    })?;
                if crate::runner::unique_locator_probe(&self.root, &file.name, &locators[0].value)
                    .is_none()
                {
                    bail!(
                        "import: Exemplar edge '{}' locator does not resolve exactly one live symbol in '{}'",
                        e.id,
                        file.name
                    );
                }
            }
        }
        // Facets/tags reference a node or edge by (target_id, target_kind), but
        // the schema has no FK on target_id — an orphaned facet/tag would import
        // silently (M-14). Partition against the imported nodes/edges: valid
        // targets and preserved soft refs are written; true orphans are an error
        // under the strict default and dropped-with-report under repair.
        let mut report = RestoreReport::default();
        let edge_ids: std::collections::HashSet<&str> =
            snap.edges.iter().map(|e| e.id.as_str()).collect();
        let has_target = |id: &str, kind: TargetKind| match kind {
            TargetKind::Node => node_types.contains_key(id),
            TargetKind::Edge => edge_ids.contains(id),
        };
        let mut facets: Vec<&Facet> = Vec::with_capacity(snap.facets.len());
        for f in &snap.facets {
            if has_target(&f.target_id, f.target_kind) {
                facets.push(f);
            } else if is_soft_ref_facet(f) {
                report.preserved_soft_refs += 1;
                facets.push(f);
            } else if repair {
                report.dropped_facets.push((
                    f.target_kind.as_str().to_string(),
                    f.target_id.clone(),
                    f.key.clone(),
                ));
            } else {
                bail!(
                    "import: facet '{}' on {} '{}' references a missing target \
                     (re-run `loom import --repair-orphans` to drop dangling facets/tags)",
                    f.key,
                    f.target_kind,
                    f.target_id
                );
            }
        }
        let mut tags: Vec<&Tag> = Vec::with_capacity(snap.tags.len());
        for t in &snap.tags {
            if has_target(&t.target_id, t.target_kind) {
                tags.push(t);
            } else if repair {
                report.dropped_tags.push((
                    t.target_kind.as_str().to_string(),
                    t.target_id.clone(),
                    t.term.clone(),
                ));
            } else {
                bail!(
                    "import: tag '{}' on {} '{}' references a missing target \
                     (re-run `loom import --repair-orphans` to drop dangling facets/tags)",
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
                "INSERT INTO edge(id,from_id,to_id,kind,truth_class,status,
                        depends_on,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    e.id,
                    e.from_id,
                    e.to_id,
                    e.kind.as_str(),
                    e.truth_class.as_str(),
                    e.status.as_str(),
                    e.depends_on.to_string(),
                    e.created_at,
                    e.updated_at
                ],
            )?;
        }
        // Facts and their anchors travel, but their STRENGTH does not: an
        // exported `verified` is a claim about another filesystem. Every fact
        // lands at whatever its evidence earns HERE, and `verify_imported`
        // (below) re-checks each anchor against this working tree before the
        // transaction closes. This is what stops an import smuggling in a
        // verified fact whose covered files do not exist locally.
        // Legacy exports may carry delegated approvals. They are deliberately
        // not restored: only an explicit human decision can establish wantedness.
        // Filtering evidence with its fact avoids leaving orphan anchors.
        let mut facts = Vec::with_capacity(snap.facts.len());
        for f in &snap.facts {
            if f.claim == Claim::Ratification && f.asserted_by.starts_with("policy:") {
                continue;
            }
            let incompatible_subject = f.subject_kind == TargetKind::Node
                && node_types
                    .get(f.subject_id.as_str())
                    .is_some_and(|t| matches!(t, NodeType::TaskRecord | NodeType::Note));
            if incompatible_subject && repair {
                report.dropped_facts.push((
                    f.subject_kind.as_str().to_string(),
                    f.subject_id.clone(),
                    f.claim.as_str().to_string(),
                ));
            } else if incompatible_subject {
                bail!(
                    "import: facts cannot attach to TaskRecord or Note subject '{}'",
                    f.subject_id
                );
            } else if has_target(&f.subject_id, f.subject_kind) {
                facts.push(f.clone());
            } else if is_soft_ref_fact(f) {
                report.preserved_soft_refs += 1;
                facts.push(f.clone());
            } else if repair {
                report.dropped_facts.push((
                    f.subject_kind.as_str().to_string(),
                    f.subject_id.clone(),
                    f.claim.as_str().to_string(),
                ));
            } else {
                bail!(
                    "import: fact '{}' on {} '{}' references a missing subject \
                     (re-run `loom import --repair-orphans` to drop dangling facts)",
                    f.claim,
                    f.subject_kind,
                    f.subject_id
                );
            }
        }
        let fact_ids: std::collections::HashSet<&str> =
            facts.iter().map(|f| f.id.as_str()).collect();
        let evidence: Vec<_> = snap
            .evidence
            .iter()
            .filter(|e| fact_ids.contains(e.fact_id.as_str()))
            .cloned()
            .collect();
        super::facts::insert_imported(&tx, &facts, &evidence)?;
        for f in &facets {
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
        for t in &tags {
            tx.execute(
                "INSERT INTO tag(target_id,target_kind,term) VALUES (?1,?2,?3)",
                params![t.target_id, t.target_kind.as_str(), t.term],
            )?;
        }
        for (key, value) in &snap.config {
            // Accepted only as inert legacy input. It is neither persisted nor
            // portable in new exports, so old config cannot restore delegation.
            if key == "ratify_policies" {
                continue;
            }
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
        // Phase 3, mandatory: every imported anchor is re-checked against THIS
        // working tree. Whatever the export claimed, a fact keeps only the
        // strength its evidence earns here. Without this, `import` is a door
        // straight past the write boundary — which is exactly what it used to be.
        self.reverify_all(&std::collections::BTreeSet::new())?;
        Ok(report)
    }
}
