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

    /// Database-clock UTC timestamp for portable asserted configuration.
    pub fn current_timestamp(&self) -> Result<String> {
        now(&self.conn)
    }

    /// Refuse a direct authority-bearing write from every declared LLM lane.
    /// A mediated human decision takes a separate typed path; this method is
    /// intentionally unaware of it.
    pub fn require_human_authority(&self) -> Result<()> {
        if let Agent::Lane(r) = self.agent() {
            bail!(
                "INV-8: ratification authority is human-only — agent 'llm:{}' may not decide; ask the human and record their exact answer with --human-decision",
                r.as_str()
            );
        }
        Ok(())
    }

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
        crate::pattern::validate_node_body(node_type, &body)?;
        if !registry::node_allows_truth_class(node_type, TruthClass::Asserted) {
            bail!(
                "'{node_type}' does not allow asserted nodes — use add_derived_node, not add_node"
            );
        }
        let tc = TruthClass::Asserted;
        let (id, now) = id_and_now(&self.conn)?;
        let prospective = Node {
            id: id.clone(),
            node_type,
            name: name.into(),
            description: description.into(),
            status: status.into(),
            truth_class: tc,
            body: body.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        crate::research::validate_record(None, &prospective, chrono::Utc::now(), false)?;
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
        let mut exact = self.find_nodes_by("name = ?1", params![key], type_filter)?;
        if exact.len() == 1 {
            return exact
                .pop()
                .ok_or_else(|| anyhow::anyhow!("exact.len() == 1 but node vector empty"));
        }
        if exact.len() > 1 {
            // Actionable ambiguity: list every colliding node with its short id
            // so the caller can address one directly (show/remove it by id)
            // instead of being told only a count. A bare count is what forced a
            // blind, substring-based dedup during recovery.
            let list = exact
                .iter()
                .map(|n| format!("[{}] {}", crate::model::short(&n.id), n.name))
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
                    .map(|m| format!("[{}] {}", crate::model::short(&m.id), m.name))
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
                 FROM node WHERE node_type=?1 ORDER BY name, id LIMIT ?2 OFFSET ?3",
            )?;
            stmt.query_map(
                params![t.as_str(), limit as i64, offset as i64],
                row_to_node,
            )?
        } else {
            stmt = self.conn.prepare(
                "SELECT id,node_type,name,description,status,truth_class,body,created_at,updated_at
                 FROM node ORDER BY name, id LIMIT ?1 OFFSET ?2",
            )?;
            stmt.query_map(params![limit as i64, offset as i64], row_to_node)?
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Ids of the validations registering exactly `command`, minus `skip`.
    ///
    /// Asked in SQL rather than by listing every validation and comparing in
    /// Rust. The caller is a WRITE-TIME warning on `validation add`/`update`, so
    /// its cost is paid on every proof anyone registers; the listing form
    /// deserialized the body JSON of every validation in the graph and then ran
    /// an edge query per node, to discard nearly all of them. Here SQLite reads
    /// the command out of the body itself and returns only genuine collisions —
    /// which is almost always none, and never many.
    ///
    /// Still a scan of the validation rows (`idx_node_type` bounds it to those);
    /// what it no longer does is materialize them. An expression index would make
    /// it a lookup, and at the graph sizes loom sees the difference is not
    /// measurable — that is a schema migration to buy on evidence, not on
    /// principle.
    pub fn validations_with_command(
        &self,
        command: &str,
        skip: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM node
              WHERE node_type = ?1
                AND json_extract(body, '$.command') = ?2
                AND (?3 IS NULL OR id <> ?3)
              ORDER BY id",
        )?;
        let rows = stmt.query_map(params![NodeType::Validation.as_str(), command, skip], |r| {
            r.get::<_, String>(0)
        })?;
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
        let tx = self.conn.unchecked_transaction()?;
        for eid in &incident {
            tx.execute(
                "DELETE FROM fact WHERE subject_id=?1 AND subject_kind='edge'",
                params![eid],
            )?;
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
            "DELETE FROM fact WHERE subject_id=?1 AND subject_kind='node'",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM facet WHERE target_id=?1 AND target_kind='node'",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM tag WHERE target_id=?1 AND target_kind='node'",
            params![id],
        )?;
        for note_id in &dependent_notes {
            tx.execute(
                "DELETE FROM fact WHERE subject_id=?1 AND subject_kind='node'",
                params![note_id],
            )?;
            tx.execute(
                "DELETE FROM facet WHERE target_id=?1 AND target_kind='node'",
                params![note_id],
            )?;
            tx.execute(
                "DELETE FROM tag WHERE target_id=?1 AND target_kind='node'",
                params![note_id],
            )?;
            tx.execute("DELETE FROM node WHERE id=?1", params![note_id])?;
        }
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
    /// locator). Ownership is enforced here so no command or direct Store
    /// caller can erase another lane's asserted relationship.
    pub fn require_edge_owner(&self, id: &str) -> Result<()> {
        let edge = self
            .get_edge(id)?
            .ok_or_else(|| anyhow!("no edge '{id}'"))?;
        self.check_lane(registry::spec(edge.kind).owner)
    }

    pub fn delete_edge(&self, id: &str) -> Result<()> {
        self.require_edge_owner(id)?;
        // Facets, tags, then the edge fall together; `maybe_tx` composes with an
        // outer batch when one is open, so a caller inside `begin()` keeps a
        // single unit and a lone call still gets its own. A `bail` on a missing
        // edge drops the tx (or bubbles to the outer batch) and rolls back.
        let tx = self.maybe_tx()?;
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
        let n = self
            .conn
            .execute("DELETE FROM edge WHERE id=?1", params![id])?;
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
        // Wantedness rots with meaning: a ratified intent whose criterion
        // changed is no longer known-wanted. Stale the ratification exactly as
        // the loop below stales verdicts; the ratify queue re-serves it.
        if self.ratification(id)? == "ratified" {
            // Demotion, not authorization: no human is required to notice that
            // meaning drifted, and requiring one would mean stale wantedness
            // could only be spotted by the person it was hidden from.
            self.assert_fact(
                crate::store::Assertion::new(
                    crate::store::Subject::Node(id.to_string()),
                    crate::model::Claim::Ratification,
                    "needs_reconfirmation",
                    "sync",
                )
                .criterion("redefined after ratification")
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    "the criterion the authority approved was rewritten".into(),
                )]),
            )?;
            self.add_note(id, "ratify", "ratification staled by redefinition")?;
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
                    // loom-stability-exempt: resets a proof to not_run on ripple
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
    /// Ratify an intent: the human authority's evidence-bearing "yes, this is
    /// wanted". INV-8 is about who decides, not who types: an LLM lane may
    /// record an explicit mediated [`HumanDecision`], but may never supply the
    /// decision itself. The ordinary direct path remains denied to every lane.
    /// Record that a behavior is NOT wanted. Same boundary, same authority
    /// check, same journal — refusal is an act of the same kind as approval.
    pub fn reject_intent(&self, id: &str, reason: &str, presence: &str) -> Result<()> {
        let decision = crate::ratification::HumanDecision::direct(presence)?;
        self.apply_human_decision(id, "rejected", reason, &decision, None)
    }

    pub fn reject_intent_from_human(
        &self,
        id: &str,
        reason: &str,
        decision: &crate::ratification::HumanDecision,
    ) -> Result<()> {
        self.apply_human_decision(id, "rejected", reason, decision, None)
    }

    pub fn ratify_intent(&self, id: &str, evidence: &str, presence: &str) -> Result<()> {
        let decision = crate::ratification::HumanDecision::direct(presence)?;
        self.apply_human_decision(id, "ratified", evidence, &decision, None)
    }

    pub fn ratify_intent_from_human(
        &self,
        id: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
    ) -> Result<()> {
        self.apply_human_decision(id, "ratified", evidence, decision, None)
    }

    pub fn ratify_intent_from_human_batch(
        &self,
        id: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
        batch_id: &str,
    ) -> Result<()> {
        self.apply_human_decision(id, "ratified", evidence, decision, Some(batch_id))
    }

    pub fn ratify_pattern(&self, id: &str, evidence: &str, presence: &str) -> Result<()> {
        let decision = crate::ratification::HumanDecision::direct(presence)?;
        self.apply_human_decision(id, "ratified", evidence, &decision, None)
    }

    pub fn ratify_pattern_from_human(
        &self,
        id: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
    ) -> Result<()> {
        self.apply_human_decision(id, "ratified", evidence, decision, None)
    }

    pub fn invalidate_pattern(&self, id: &str) -> Result<usize> {
        if self.ratification(id)? == "ratified" {
            self.assert_fact(
                crate::store::Assertion::new(
                    crate::store::Subject::Node(id.to_string()),
                    crate::model::Claim::Ratification,
                    "needs_reconfirmation",
                    "sync",
                )
                .criterion("pattern guidance or applicability changed")
                .cited(vec![crate::evidence::CitedEvidence::Claim(
                    "the guidance the human approved was rewritten".into(),
                )]),
            )?;
        }
        let mut reopened = 0;
        for edge in self.edges_with(Some(EdgeKind::Exemplar), Some(id), None)? {
            if self.stale_edge(&edge.id, "pattern guidance or applicability changed")? {
                reopened += 1;
            }
        }
        Ok(reopened)
    }

    /// Both halves of the authority — approval and refusal — through one gate.
    fn apply_human_decision(
        &self,
        id: &str,
        state: &str,
        evidence: &str,
        decision: &crate::ratification::HumanDecision,
        batch_id: Option<&str>,
    ) -> Result<()> {
        let presence = decision.presence();
        // Fail before journaling. The assertion boundary repeats this check so
        // no alternate caller can bypass it, but doing it here avoids leaving
        // a journal event for a write that was refused.
        if !decision.permits_mediated_recording() {
            self.require_human_authority()?;
        }
        // The prose anchors the WANT; the journal entry below anchors the ACT.
        // Both are required: without this check the journal ref loom writes
        // itself would make every ratification self-anchoring, which is the
        // circularity the whole evidence spine exists to refuse.
        if crate::model::is_placeholder(evidence) {
            bail!(
                "ratification needs substantive evidence: why this behavior is wanted \
                 (an utterance, a source doc, a decision)"
            );
        }
        let event = if state == "rejected" {
            "rejection"
        } else {
            "ratification"
        };
        // The journal entry is written FIRST, so the ref the fact cites is real
        // by construction rather than by convention. This is also what makes
        // "every ratified intent has a journal entry behind it" a checkable
        // invariant — the predicate that identifies the 39 facet-only
        // ratifications this graph carried from before the spine.
        let entry = self.append_journal(event, id, {
            let mut payload = serde_json::json!({
            "evidence": evidence,
            "ratified_by": "human",
            "presence": presence,
            "human_decision": decision,
            });
            if let Some(batch_id) = batch_id {
                payload["batch_id"] = serde_json::json!(batch_id);
                payload["decision_mode"] = serde_json::json!("batch");
            }
            let node = self
                .get_node(id)?
                .ok_or_else(|| anyhow!("no node '{id}'"))?;
            if node.node_type == NodeType::Pattern {
                payload["pattern_body"] = node.body;
            }
            payload
        })?;
        let mut cited = crate::evidence::cite(self.root(), evidence)?;
        cited.push(crate::evidence::CitedEvidence::Journal(entry.id.clone()));
        // Authority (INV-8), the deprecated check, and the evidence floor all
        // live at the boundary now — this function only shapes the assertion.
        let mut assertion = crate::store::Assertion::new(
            crate::store::Subject::Node(id.to_string()),
            crate::model::Claim::Ratification,
            state,
            "human",
        )
        .criterion(presence)
        .confidence(1.0)
        .cited(cited);
        if decision.permits_mediated_recording() {
            assertion = assertion.mediated_human_decision();
        }
        if let Some(batch_id) = batch_id {
            assertion = assertion.batch(batch_id);
        }
        self.assert_fact(assertion)?;
        // A mint-time ratification writes no note: the fact and the journal
        // entry already record that the minting act WAS the ratification, and a
        // note on every solo mint is pure audit-trail bloat.
        if presence != "mint" {
            self.add_note(id, "ratify", &format!("{state}: {evidence}"))?;
        }
        Ok(())
    }

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
        // loom-stability-exempt: retires a node
        self.set_node_status(id, "deprecated")?;
        Ok(())
    }
}
