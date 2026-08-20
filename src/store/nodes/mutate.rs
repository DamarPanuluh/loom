use super::super::*;

impl Store {
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
        let quality_description_changed = node.node_type == NodeType::QualityRule
            && description.is_some_and(|value| value != node.description);
        if let Some(n) = name {
            node.name = n.to_string();
        }
        if let Some(d) = description {
            node.description = d.to_string();
        }
        if let Some(s) = status {
            node.status = s.to_string();
        }
        crate::research::validate_record(
            self.get_node(id)?.as_ref(),
            &node,
            chrono::Utc::now(),
            false,
        )?;
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE node SET name=?2,description=?3,status=?4,updated_at=?5 WHERE id=?1",
            params![id, node.name, node.description, node.status, now],
        )?;
        if quality_description_changed {
            let cause = format!("quality rule '{}' description updated", node.name);
            for edge in self.edges_with(Some(EdgeKind::Governs), Some(id), None)? {
                self.stale_edge(&edge.id, &cause)?;
            }
        }
        node.updated_at = now;
        Ok(node)
    }

    /// Replace a node's JSON body (e.g. a surface's kind/identity or a
    /// validation's type/command). Asserted-node attribute edits live here.
    pub fn set_node_body(&self, id: &str, body: &serde_json::Value) -> Result<()> {
        let node = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no node '{id}'"))?;
        crate::pattern::validate_node_body(node.node_type, body)?;
        let mut prospective = node.clone();
        prospective.body = body.clone();
        crate::research::validate_record(Some(&node), &prospective, chrono::Utc::now(), false)?;
        // Pattern authority covers the exact normative body. Keep this at the
        // persistence boundary so imports, future commands, and direct Store
        // callers cannot let rewritten guidance borrow an earlier approval.
        // Validation happens first; after that we deliberately fail closed if
        // the final UPDATE fails, leaving the old text unratified.
        if node.node_type == NodeType::Pattern && node.body != *body {
            self.invalidate_pattern(id)?;
        }
        let body_changed = node.body != *body;
        let now = now(&self.conn)?;
        let n = self.conn.execute(
            "UPDATE node SET body=?2, updated_at=?3 WHERE id=?1",
            params![id, body.to_string(), now],
        )?;
        if n == 0 {
            bail!("no node '{id}'");
        }
        // A quality verdict measures the rule body that existed when it was
        // recorded. Rewriting patterns or guidance changes that criterion, so
        // every settled governs edge must fail closed instead of borrowing the
        // old verdict. The next quality packet remeasures it under the new rule.
        if body_changed && node.node_type == NodeType::QualityRule {
            let cause = format!("quality rule '{}' body updated", node.name);
            for edge in self.edges_with(Some(EdgeKind::Governs), Some(id), None)? {
                self.stale_edge(&edge.id, &cause)?;
            }
        }
        Ok(())
    }

    /// Atomically route a captured intake item to one exact typed destination.
    /// Names and fragments are deliberately refused: the durable reference is
    /// a node id whose node type (and task subtype where applicable) agrees
    /// with the selected landing.
    pub fn route_inbox_item(
        &self,
        id: &str,
        destination: &crate::model::IntakeDestination,
    ) -> Result<Node> {
        let mut item = self
            .get_node(id)?
            .ok_or_else(|| anyhow!("no node '{id}'"))?;
        if item.node_type != NodeType::InboxItem {
            bail!("'{id}' is not an inbox item");
        }
        let target = self.get_node(&destination.reference)?.ok_or_else(|| {
            anyhow!(
                "no destination node with exact stable id '{}'",
                destination.reference
            )
        })?;
        let expected_type = destination.destination_type.node_type();
        if target.node_type != expected_type {
            bail!(
                "destination type '{}' requires a {} node, but '{}' is {}",
                destination.destination_type,
                expected_type,
                destination.reference,
                target.node_type
            );
        }
        if let Some(expected_kind) = destination.destination_type.task_kind() {
            let actual_kind = target.body.get("kind").and_then(|value| value.as_str());
            if actual_kind != Some(expected_kind) {
                bail!(
                    "destination type '{}' requires task kind '{}', but '{}' has kind '{}'",
                    destination.destination_type,
                    expected_kind,
                    destination.reference,
                    actual_kind.unwrap_or("missing")
                );
            }
        }
        let body = item
            .body
            .as_object_mut()
            .ok_or_else(|| anyhow!("inbox item '{id}' has a non-object body"))?;
        body.insert("destination".into(), serde_json::to_value(destination)?);
        item.status = "routed".into();
        crate::research::validate_record(
            self.get_node(id)?.as_ref(),
            &item,
            chrono::Utc::now(),
            false,
        )?;
        let now = now(&self.conn)?;
        let changed = self.conn.execute(
            "UPDATE node SET status='routed',body=?2,updated_at=?3 WHERE id=?1",
            params![id, item.body.to_string(), now],
        )?;
        if changed == 0 {
            bail!("no node '{id}'");
        }
        item.updated_at = now;
        Ok(item)
    }

    fn delete_edge_records(&self, id: &str) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM fact WHERE subject_id=?1 AND subject_kind='edge'",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind='edge'",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM tag WHERE target_id=?1 AND target_kind='edge'",
            params![id],
        )?;
        Ok(self
            .conn
            .execute("DELETE FROM edge WHERE id=?1", params![id])?)
    }

    /// Hard-delete an asserted node and everything keyed to it. Incident edges
    /// and body-linked Notes are deleted explicitly (not via FK cascade) so
    /// their facets and tags cannot orphan. Notes are followed recursively: a
    /// note about a deleted decision note is part of the same target-bound
    /// history. Refuses derived nodes (sync owns them). All in one transaction.
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
        // Notes link to their target through the body, and a note can itself be
        // the target of another note (a trail on a decision). Load every note
        // once and index by target so the transitive closure is a walk over an
        // in-memory map rather than a full Note scan per popped target.
        let mut notes_by_target: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for note in self.list_nodes(Some(NodeType::Note), usize::MAX)? {
            if let Some(t) = note.body.get("target_id").and_then(|v| v.as_str()) {
                notes_by_target
                    .entry(t.to_string())
                    .or_default()
                    .push(note.id);
            }
        }
        let mut note_targets = vec![id.to_string()];
        note_targets.extend(incident.iter().cloned());
        let mut dependent_notes = std::collections::BTreeSet::new();
        while let Some(target) = note_targets.pop() {
            if let Some(ids) = notes_by_target.get(&target) {
                for note_id in ids {
                    if dependent_notes.insert(note_id.clone()) {
                        note_targets.push(note_id.clone());
                    }
                }
            }
        }
        let tx = self.maybe_tx()?;
        for eid in &incident {
            self.delete_edge_records(eid)?;
        }
        self.conn.execute(
            "DELETE FROM fact WHERE subject_id=?1 AND subject_kind='node'",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind='node'",
            params![id],
        )?;
        self.conn.execute(
            "DELETE FROM tag WHERE target_id=?1 AND target_kind='node'",
            params![id],
        )?;
        for note_id in &dependent_notes {
            self.conn.execute(
                "DELETE FROM fact WHERE subject_id=?1 AND subject_kind='node'",
                params![note_id],
            )?;
            self.conn.execute(
                "DELETE FROM facet WHERE target_id=?1 AND target_kind='node'",
                params![note_id],
            )?;
            self.conn.execute(
                "DELETE FROM tag WHERE target_id=?1 AND target_kind='node'",
                params![note_id],
            )?;
            self.conn
                .execute("DELETE FROM node WHERE id=?1", params![note_id])?;
        }
        self.conn
            .execute("DELETE FROM node WHERE id=?1", params![id])?;
        if let Some(tx) = tx {
            tx.commit()?;
        }
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
    /// locator). Ownership is enforced here so no command or direct Store
    /// caller can erase another lane's asserted relationship.
    pub fn require_edge_owner(&self, id: &str) -> Result<()> {
        let edge = self
            .get_edge(id)?
            .ok_or_else(|| anyhow!("no edge '{id}'"))?;
        self.require_edge_kind_owner(edge.kind)
    }

    pub fn delete_edge(&self, id: &str) -> Result<()> {
        self.require_edge_owner(id)?;
        // Facets, tags, then the edge fall together; `maybe_tx` composes with an
        // outer batch when one is open, so a caller inside `begin()` keeps a
        // single unit and a lone call still gets its own. A `bail` on a missing
        // edge drops the tx (or bubbles to the outer batch) and rolls back.
        let tx = self.maybe_tx()?;
        let n = self.delete_edge_records(id)?;
        if n == 0 {
            bail!("no edge '{id}'");
        }
        if let Some(tx) = tx {
            tx.commit()?;
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
}
