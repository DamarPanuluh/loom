//! Node persistence — the asserted-node write path of the store.
//!
//! Plane: engine (persistence). `add_node` accepts ONLY asserted node kinds —
//! derived nodes must take the deterministic-id path in `derived.rs`, so the
//! truth-class line is enforced at the insert (INV-5). Name resolution is
//! exact-or-unique-fragment; ambiguity is an error with candidates, never a
//! silent guess.

use super::*;

impl Store {
    // ---- nodes -----------------------------------------------------------

    /// Add a node. Generates id + timestamps. `body` defaults to `{}`.
    pub fn add_node(
        &self,
        node_type: NodeType,
        name: &str,
        description: &str,
        status: &str,
        body: serde_json::Value,
    ) -> Result<Node> {
        if name.trim().is_empty() {
            bail!("node name must not be empty");
        }
        if !registry::node_allows_truth_class(node_type, TruthClass::Asserted) {
            bail!(
                "'{node_type}' does not allow asserted nodes — use add_derived_node, not add_node"
            );
        }
        let tc = TruthClass::Asserted;
        let (id, now) = id_and_now(&self.conn)?;
        self.conn.execute(
            "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?8)",
            params![
                id,
                node_type.as_str(),
                name,
                description,
                status,
                tc.as_str(),
                body.to_string(),
                now
            ],
        )?;
        self.get_node(&id)?
            .ok_or_else(|| anyhow!("node vanished after insert"))
    }

    pub fn get_node(&self, id: &str) -> Result<Option<Node>> {
        self.conn
            .query_row(
                "SELECT id,node_type,name,description,status,truth_class,body,created_at,updated_at
                 FROM node WHERE id=?1",
                params![id],
                row_to_node,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Resolve a node by id, exact name, or unique name fragment. Ambiguity is
    /// an error with candidates — never a silent guess.
    pub fn resolve_node(&self, key: &str, node_type: Option<NodeType>) -> Result<Node> {
        if let Some(n) = self.get_node(key)? {
            if node_type.is_none_or(|t| t == n.node_type) {
                return Ok(n);
            }
        }
        let type_filter = node_type.map(|t| t.as_str());
        // exact name first
        let exact = self.find_nodes_by("name = ?1", params![key], type_filter)?;
        if exact.len() == 1 {
            return Ok(exact
                .into_iter()
                .next()
                .expect("exact.len() == 1 checked above"));
        }
        if exact.len() > 1 {
            // Actionable ambiguity: list every colliding node with its short id
            // so the caller can address one directly (show/remove it by id)
            // instead of being told only a count. A bare count is what forced a
            // blind, substring-based dedup during recovery.
            let list = exact
                .iter()
                .map(|n| format!("[{}] {}", &n.id[..8.min(n.id.len())], n.name))
                .collect::<Vec<_>>()
                .join("; ");
            bail!(
                "ambiguous name '{key}': {} nodes match exactly — address one by id: {list}",
                exact.len()
            );
        }
        // unique id prefix — the short id most commands print, e.g. "[eed4cdb2]".
        // Strictly additive: resolves only a lookup that would otherwise fail,
        // and on an ambiguous/empty prefix falls through to the name logic below
        // rather than bailing, so it can never break a currently-working lookup.
        if key.len() >= 4 && key.chars().all(|c| c.is_ascii_hexdigit()) {
            let by_id =
                self.find_nodes_by("id LIKE ?1", params![format!("{key}%")], type_filter)?;
            if by_id.len() == 1 {
                return Ok(by_id.into_iter().next().expect("len == 1 checked above"));
            }
        }
        // unique fragment
        let frag = format!("%{key}%");
        let matches = self.find_nodes_by("name LIKE ?1", params![frag], type_filter)?;
        match matches.len() {
            0 => bail!("no node matches '{key}'"),
            1 => Ok(matches.into_iter().next().expect("len == 1 by match arm")),
            n => {
                let names: Vec<_> = matches
                    .iter()
                    .take(8)
                    .map(|m| format!("[{}] {}", &m.id[..8.min(m.id.len())], m.name))
                    .collect();
                bail!(
                    "ambiguous fragment '{key}': {n} candidates: {}",
                    names.join("; ")
                )
            }
        }
    }

    pub(super) fn find_nodes_by(
        &self,
        where_clause: &str,
        params: &[&dyn rusqlite::ToSql],
        type_filter: Option<&str>,
    ) -> Result<Vec<Node>> {
        let sql = if let Some(t) = type_filter {
            format!(
                "SELECT {NODE_COLS}
                 FROM node WHERE {where_clause} AND node_type='{t}' ORDER BY id"
            )
        } else {
            format!(
                "SELECT {NODE_COLS}
                 FROM node WHERE {where_clause} ORDER BY id"
            )
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_nodes(&self, node_type: Option<NodeType>, limit: usize) -> Result<Vec<Node>> {
        self.list_nodes_page(node_type, limit, 0)
    }

    /// Page a node listing (ordered by name). `offset` skips that many rows
    /// before taking `limit`. Offset-0 is the full/first-page case every
    /// internal caller uses via [`list_nodes`]; a non-zero offset backs the
    /// `--offset` flag on the `list` commands so a caller can walk past the
    /// first page instead of being permanently capped at it. A negative i64
    /// `limit` (from `usize::MAX`) means "no bound" in SQLite, so the
    /// full-scan callers keep their unlimited behavior.
    pub fn list_nodes_page(
        &self,
        node_type: Option<NodeType>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Node>> {
        let mut stmt;
        let rows = if let Some(t) = node_type {
            stmt = self.conn.prepare(
                "SELECT id,node_type,name,description,status,truth_class,body,created_at,updated_at
                 FROM node WHERE node_type=?1 ORDER BY name LIMIT ?2 OFFSET ?3",
            )?;
            stmt.query_map(
                params![t.as_str(), limit as i64, offset as i64],
                row_to_node,
            )?
        } else {
            stmt = self.conn.prepare(
                "SELECT id,node_type,name,description,status,truth_class,body,created_at,updated_at
                 FROM node ORDER BY name LIMIT ?1 OFFSET ?2",
            )?;
            stmt.query_map(params![limit as i64, offset as i64], row_to_node)?
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Total node count of a type (or all types). Backs the "showing N–M of
    /// TOTAL" page footer so a `list` caller knows more rows exist beyond the
    /// current page — the signal whose absence hid duplicates during recovery.
    pub fn count_nodes(&self, node_type: Option<NodeType>) -> Result<usize> {
        let n: i64 = if let Some(t) = node_type {
            self.conn.query_row(
                "SELECT COUNT(*) FROM node WHERE node_type=?1",
                params![t.as_str()],
                |r| r.get(0),
            )?
        } else {
            self.conn
                .query_row("SELECT COUNT(*) FROM node", [], |r| r.get(0))?
        };
        Ok(n as usize)
    }

    /// Update a node's mutable fields. Touches `updated_at`.
    pub fn update_node(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        status: Option<&str>,
    ) -> Result<Node> {
        let mut node = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no node '{id}'"))?;
        if let Some(n) = name {
            node.name = n.to_string();
        }
        if let Some(d) = description {
            node.description = d.to_string();
        }
        if let Some(s) = status {
            node.status = s.to_string();
        }
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE node SET name=?2,description=?3,status=?4,updated_at=?5 WHERE id=?1",
            params![id, node.name, node.description, node.status, now],
        )?;
        node.updated_at = now;
        Ok(node)
    }

    /// Replace a node's JSON body (e.g. a surface's kind/identity or a
    /// validation's type/command). Asserted-node attribute edits live here.
    pub fn set_node_body(&self, id: &str, body: &serde_json::Value) -> Result<()> {
        let now = now(&self.conn)?;
        let n = self.conn.execute(
            "UPDATE node SET body=?2, updated_at=?3 WHERE id=?1",
            params![id, body.to_string(), now],
        )?;
        if n == 0 {
            bail!("no node '{id}'");
        }
        Ok(())
    }

    /// Hard-delete an asserted node and everything keyed to it. Incident edges
    /// are deleted explicitly (not via FK cascade) so their edge-scoped facets
    /// and tags — e.g. an `implements` locator — are cleaned too; those rows have
    /// no FK and would otherwise orphan. Refuses derived nodes (sync owns them).
    /// All in one transaction.
    pub fn delete_node(&self, id: &str) -> Result<()> {
        let node = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no node '{id}'"))?;
        if node.truth_class == TruthClass::Derived {
            bail!("'{id}' is a derived node — rebuilt by `loom sync`; do not hard-delete it");
        }
        let mut incident: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for e in self.edges_with(None, Some(id), None)? {
            incident.insert(e.id);
        }
        for e in self.edges_with(None, None, Some(id))? {
            incident.insert(e.id);
        }
        let tx = self.conn.unchecked_transaction()?;
        for eid in &incident {
            tx.execute(
                "DELETE FROM facet WHERE target_id=?1 AND target_kind='edge'",
                params![eid],
            )?;
            tx.execute(
                "DELETE FROM tag WHERE target_id=?1 AND target_kind='edge'",
                params![eid],
            )?;
            tx.execute("DELETE FROM edge WHERE id=?1", params![eid])?;
        }
        tx.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind='node'",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM tag WHERE target_id=?1 AND target_kind='node'",
            params![id],
        )?;
        tx.execute("DELETE FROM node WHERE id=?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Remove a vocabulary term and cascade-untag any nodes carrying it (the
    /// `tag` table references the term with no FK, so drop those rows too).
    pub fn remove_vocab_term(&self, term: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM tag WHERE term=?1", params![term])?;
        let n = tx.execute("DELETE FROM tag_vocabulary WHERE term=?1", params![term])?;
        if n == 0 {
            bail!("no vocab term '{term}'");
        }
        tx.commit()?;
        Ok(())
    }

    /// Delete one edge and its edge-scoped facets/tags (e.g. an `implements`
    /// locator). Asserted edges only at the command layer; this primitive is
    /// unconditional.
    pub fn delete_edge(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind='edge'",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM tag WHERE target_id=?1 AND target_kind='edge'",
            params![id],
        )?;
        let n = self
            .conn
            .execute("DELETE FROM edge WHERE id=?1", params![id])?;
        if n == 0 {
            bail!("no edge '{id}'");
        }
        Ok(())
    }

    /// Attach an audit Note to a target (stored as a Note node; the link lives
    /// in the note body). Used for decision/transition/redefinition trails.
    pub fn add_note(&self, target_id: &str, kind: &str, text: &str) -> Result<Node> {
        self.add_node(
            NodeType::Note,
            &format!("note:{kind}"),
            text,
            kind,
            serde_json::json!({ "target_id": target_id, "kind": kind }),
        )
    }

    /// Notes attached to a target, newest first — the adjudication trail.
    /// Single owner of the body `target_id` lookup (notes link through their
    /// body, not facets or edges); `note list` and packet assembly both go
    /// through here.
    pub fn notes_for(&self, target_id: &str) -> Result<Vec<Node>> {
        let mut notes: Vec<Node> = self
            .list_nodes(Some(NodeType::Note), usize::MAX)?
            .into_iter()
            .filter(|n| n.body.get("target_id").and_then(|v| v.as_str()) == Some(target_id))
            .collect();
        notes.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(notes)
    }

    /// Redefine an intent's description — the semantic twin of `sync`. Ripples
    /// one hop: every settled asserted verdict touching the intent re-opens to
    /// needs_reverification, linked validations reset to not_run, completeness
    /// waivers are cleared (a waiver granted against the OLD meaning must be
    /// re-earned against the new one), and the old wording is preserved in a
    /// decision note. A name-only change does not call this (no ripple).
    /// Builder lane.
    pub fn redefine_intent(&self, id: &str, new_description: &str) -> Result<usize> {
        self.check_lane(registry::OwnerRole::Builder)?;
        let intent = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no intent '{id}'"))?;
        if intent.node_type != NodeType::Intent {
            bail!("'{id}' is not an intent");
        }
        if intent.status == "deprecated" {
            bail!("cannot redefine a deprecated intent");
        }
        // preserve old wording
        self.add_note(
            id,
            "decision",
            &format!("redefined; previous description: {}", intent.description),
        )?;
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE node SET description=?2,updated_at=?3 WHERE id=?1",
            params![id, new_description, now],
        )?;
        // A redefinition invalidates every completeness waiver: the reasons
        // were given for the previous meaning.
        let cleared = self.conn.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind='node' AND key LIKE 'waiver:%'",
            params![id],
        )?;
        if cleared > 0 {
            self.add_note(
                id,
                "decision",
                &format!("{cleared} completeness waiver(s) re-opened by redefinition"),
            )?;
        }
        // ripple one hop: implements/targets/governs/validates/relationships touching it
        let cause = format!("intent '{}' description updated", intent.name);
        let mut reopened = 0usize;
        // Implements is Intent→CodeFile, so a grounding hangs off the FROM
        // side; the old to-side query never matched it, silently leaving
        // grounding verdicts settled across a redefinition (H-1). Targets/
        // governs/validates are X→Intent and hang off the TO side.
        for e in self.edges_with(Some(EdgeKind::Implements), Some(id), None)? {
            if self.edge_superseded(&e.id)? {
                continue; // a superseded grounding is history, not re-opened
            }
            if self.stale_edge(&e.id, &cause)? {
                reopened += 1;
            }
        }
        for k in [EdgeKind::Targets, EdgeKind::Governs, EdgeKind::Validates] {
            for e in self.edges_with(Some(k), None, Some(id))? {
                if self.stale_edge(&e.id, &cause)? {
                    reopened += 1;
                }
                if k == EdgeKind::Validates {
                    // A failed reset would leave the proof showing its old
                    // result while the command reports success (M-11) — surface it.
                    self.set_node_status(&e.from_id, "not_run")?;
                }
            }
        }
        for k in [
            EdgeKind::Relates,
            EdgeKind::Requires,
            EdgeKind::ScenarioOf,
            EdgeKind::VariantOf,
            EdgeKind::Triggers,
            EdgeKind::Sequence,
        ] {
            for e in self.edges_with(Some(k), Some(id), None)? {
                if self.stale_edge(&e.id, &cause)? {
                    reopened += 1;
                }
            }
            for e in self.edges_with(Some(k), None, Some(id))? {
                if self.stale_edge(&e.id, &cause)? {
                    reopened += 1;
                }
            }
        }
        Ok(reopened)
    }

    /// Retire an intent: status → deprecated. Invisible to computation, visible
    /// to history. Builder lane.
    pub fn retire_intent(&self, id: &str, reason: &str, replaced_by: Option<&str>) -> Result<()> {
        self.check_lane(registry::OwnerRole::Builder)?;
        let intent = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no intent '{id}'"))?;
        if intent.node_type != NodeType::Intent {
            bail!("'{id}' is not an intent");
        }
        let note = match replaced_by {
            Some(r) => format!("retired: {reason} (replaced by {r})"),
            None => format!("retired: {reason}"),
        };
        self.add_note(id, "decision", &note)?;
        self.set_node_status(id, "deprecated")?;
        Ok(())
    }
}
