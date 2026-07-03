use super::*;

impl Store {
    // ---- edges -----------------------------------------------------------

    /// Add an edge, validated against the edge-kind registry. New edges are
    /// created uninspected (asserted) or current (derived) with empty evidence.
    pub fn add_edge(
        &self,
        kind: EdgeKind,
        from_id: &str,
        to_id: &str,
        truth_class: TruthClass,
    ) -> Result<Edge> {
        self.check_lane(registry::spec(kind).owner)?;
        self.validate_edge_endpoints(kind, from_id, to_id, truth_class)?;
        // Asserted-only path: derived edges MUST go through `add_derived_edge`
        // (a deterministic content-addressed id, so wipe+rebuild is byte-
        // identical — INV-5). A random-id derived edge would break that (M-12).
        if truth_class != TruthClass::Asserted {
            bail!("add_edge is for asserted edges; use add_derived_edge for derived");
        }
        let status = InspectionStatus::Uninspected;
        let (id, now) = id_and_now(&self.conn)?;
        self.conn.execute(
            "INSERT INTO edge(id,from_id,to_id,kind,truth_class,status,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?7)",
            params![
                id,
                from_id,
                to_id,
                kind.as_str(),
                truth_class.as_str(),
                status.as_str(),
                now
            ],
        )?;
        self.get_edge(&id)?
            .ok_or_else(|| anyhow!("edge vanished after insert"))
    }

    /// Find an existing asserted edge of `kind` between `from`/`to`, or create
    /// it uninspected. Used by verdict commands that name endpoints rather than
    /// an edge id (e.g. `loom rule verdict <rule> <intent>`).
    pub fn ensure_edge(&self, kind: EdgeKind, from_id: &str, to_id: &str) -> Result<Edge> {
        if let Some(e) = self
            .edges_with(Some(kind), Some(from_id), Some(to_id))?
            .into_iter()
            .next()
        {
            return Ok(e);
        }
        self.add_edge(kind, from_id, to_id, TruthClass::Asserted)
    }

    pub fn get_edge(&self, id: &str) -> Result<Option<Edge>> {
        self.conn
            .query_row(
                "SELECT id,from_id,to_id,kind,truth_class,status,criterion,evidence,
                        confidence,depends_on,inspected_by,created_at,updated_at
                 FROM edge WHERE id=?1",
                params![id],
                row_to_edge,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Resolve an edge by exact id or unique id-prefix. Mirrors `resolve_node`
    /// so an operator can act on the 8-char ids that `find`/`next`/`edge list`
    /// print — the full id is never displayed. Ambiguity errors with the count;
    /// never a silent guess.
    pub fn resolve_edge(&self, key: &str) -> Result<Edge> {
        if let Some(e) = self.get_edge(key)? {
            return Ok(e);
        }
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EDGE_COLS} FROM edge WHERE id LIKE ?1 ORDER BY id"
        ))?;
        let matches = stmt
            .query_map(params![format!("{key}%")], row_to_edge)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match matches.len() {
            0 => bail!("no edge matches '{key}'"),
            1 => Ok(matches.into_iter().next().expect("len == 1 by match arm")),
            n => bail!("ambiguous edge prefix '{key}': {n} edges match"),
        }
    }

    /// Record an asserted verdict on an edge. This is the ONLY path that writes
    /// asserted statuses (INV-5). Enforces the evidence gate (INV-4, INV-6).
    pub fn record_verdict(
        &self,
        edge_id: &str,
        status: InspectionStatus,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
    ) -> Result<Edge> {
        let edge = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("no edge '{edge_id}'"))?;
        if edge.truth_class != TruthClass::Asserted {
            bail!("record_verdict is for asserted edges; '{edge_id}' is derived");
        }
        self.check_lane(registry::spec(edge.kind).owner)?;
        match status {
            InspectionStatus::Passing | InspectionStatus::Failing => {
                if crate::model::is_placeholder(criterion) || crate::model::is_placeholder(evidence)
                {
                    bail!("{status} verdict requires substantive criterion and evidence (not a placeholder like '…' or '<reason>')");
                }
            }
            InspectionStatus::Independent => {
                // INV-6: a measured outcome bears a criterion; INV-4: and
                // evidence of non-applicability. Both are required (H-2).
                if crate::model::is_placeholder(criterion) || crate::model::is_placeholder(evidence)
                {
                    bail!("independent verdict requires substantive criterion and evidence (not a placeholder like '…' or '<reason>')");
                }
            }
            InspectionStatus::Blocked => {
                if crate::model::is_placeholder(evidence) {
                    bail!("blocked requires a substantive reason (evidence), not a placeholder");
                }
            }
            InspectionStatus::Uninspected | InspectionStatus::NeedsReverification => {}
            InspectionStatus::Current => {
                bail!("'current' is a derived status; not valid for a verdict");
            }
        }
        if !(0.0..=1.0).contains(&confidence) {
            bail!("confidence must be in [0,1], got {confidence}");
        }
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE edge SET status=?2,criterion=?3,evidence=?4,confidence=?5,
                    inspected_by=?6,updated_at=?7 WHERE id=?1",
            params![
                edge_id,
                status.as_str(),
                criterion,
                evidence,
                confidence,
                inspected_by,
                now
            ],
        )?;
        self.clear_facet(edge_id, TargetKind::Edge, "stale_cause")?;
        self.get_edge(edge_id)?
            .ok_or_else(|| anyhow!("edge vanished after verdict"))
    }

    /// Set the status of a derived edge. This is the ONLY path that writes
    /// derived statuses (INV-5) — conceptually owned by `sync`.
    pub fn set_derived_status(&self, edge_id: &str, status: InspectionStatus) -> Result<()> {
        let edge = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("no edge '{edge_id}'"))?;
        if edge.truth_class != TruthClass::Derived {
            bail!("set_derived_status is for derived edges; '{edge_id}' is asserted");
        }
        // A derived edge's only resting state is `current`; every other status
        // is an asserted-verdict transition owned by `record_verdict`/`stale_edge`
        // (INV-5). Refuse anything else here (M-13).
        if status != InspectionStatus::Current {
            bail!("a derived edge may only be set to 'current', not '{status}' (INV-5)");
        }
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE edge SET status=?2,updated_at=?3 WHERE id=?1",
            params![edge_id, status.as_str(), now],
        )?;
        Ok(())
    }

    // ---- grounding roles (realizes / consumes / configures / verifies) ---

    /// The grounding role of an `implements` edge. Read from the `role` edge
    /// facet; a missing facet means `Realizes` (the pre-role default, so old
    /// graphs keep their exact semantics). Only meaningful for `Implements`
    /// edges.
    pub fn grounding_role(&self, edge_id: &str) -> Result<GroundingRole> {
        match self.get_facet(edge_id, TargetKind::Edge, "role")? {
            Some(v) => v
                .parse()
                .map_err(|_| anyhow!("edge '{edge_id}' has unrecognized role facet '{v}'")),
            None => Ok(GroundingRole::Realizes),
        }
    }

    /// Set (or refresh) the `role` facet on a grounding edge. A pure facet
    /// write — the re-open-on-change policy lives in `reclassify_grounding`.
    pub fn set_grounding_role(&self, edge_id: &str, role: GroundingRole) -> Result<()> {
        self.set_facet(
            edge_id,
            TargetKind::Edge,
            "role",
            role.as_str(),
            TruthClass::Asserted,
        )
    }

    /// Whether a grounding edge has been superseded by a `rehome` (its
    /// `superseded_by` facet is set). Superseded edges are history: they keep
    /// their verdict but no longer count for coverage, staleness, or navigation.
    pub fn edge_superseded(&self, edge_id: &str) -> Result<bool> {
        Ok(self
            .get_facet(edge_id, TargetKind::Edge, "superseded_by")?
            .is_some())
    }

    /// Live (non-superseded) `implements` edges into `codefile_id` whose role
    /// is `realizes` — the only role that confers ownership. A file grounded
    /// solely by `consumes`/`configures`/`verifies` edges is still unowned.
    /// Every ownership query (coverage, maturity, finding owners, layer/smell
    /// clustering) MUST route through this, never a raw `edges_with`.
    pub fn realizing_implementers(&self, codefile_id: &str) -> Result<Vec<Edge>> {
        let mut out = Vec::new();
        for e in self.edges_with(Some(EdgeKind::Implements), None, Some(codefile_id))? {
            if self.edge_superseded(&e.id)? {
                continue;
            }
            if self.grounding_role(&e.id)? == GroundingRole::Realizes {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// Live (non-superseded) `implements` edges out of `intent_id` whose role
    /// is `realizes` — the groundings where the intent's behavior actually
    /// lives. Intent-grounding checks (maturity, `coverage` ungrounded) use it.
    pub fn realizing_groundings(&self, intent_id: &str) -> Result<Vec<Edge>> {
        let mut out = Vec::new();
        for e in self.edges_with(Some(EdgeKind::Implements), Some(intent_id), None)? {
            if self.edge_superseded(&e.id)? {
                continue;
            }
            if self.grounding_role(&e.id)? == GroundingRole::Realizes {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// Reclassify a grounding edge's role (`loom edge set-role`). Keeps the
    /// edge, its verdict history, and its notes; when the role actually
    /// changes, a settled claim re-opens (→ needs_reverification, with
    /// `stale_cause` leading `role_changed`) so the owning lane re-verdicts
    /// under the new role's criterion. Reclassification is never deletion.
    pub fn reclassify_grounding(
        &self,
        edge_id: &str,
        role: GroundingRole,
        reason: &str,
    ) -> Result<(Edge, GroundingRole, bool)> {
        let edge = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("no edge '{edge_id}'"))?;
        if edge.kind != EdgeKind::Implements {
            bail!(
                "set-role is for grounding (implements) edges; '{edge_id}' is a {} edge",
                edge.kind
            );
        }
        if edge.truth_class != TruthClass::Asserted {
            bail!("cannot set the role of a derived edge");
        }
        self.check_lane(registry::spec(EdgeKind::Implements).owner)?;
        let reason = reason.trim();
        if reason.is_empty() {
            bail!("set-role requires a reason");
        }
        let old = self.grounding_role(edge_id)?;
        self.set_grounding_role(edge_id, role)?;
        self.add_note(
            &edge.from_id,
            "decision",
            &format!("grounding role {old} → {role}: {reason}"),
        )?;
        // Re-open a settled claim under the new criterion. An uninspected or
        // blocked edge has nothing settled to re-open; the note above records
        // the change either way.
        let reopened = if old != role {
            self.stale_edge(edge_id, &format!("role_changed ({old} → {role}): {reason}"))?
        } else {
            false
        };
        let edge = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("edge vanished after set-role"))?;
        Ok((edge, old, reopened))
    }

    /// Rehome a grounding edge to a successor intent (`loom edge rehome`), for a
    /// true mis-attachment (wrong intent, not just wrong role). Supersede, not
    /// delete: the old edge keeps its verdict + notes but is marked
    /// `superseded_by` the new edge and stops counting for coverage/staleness;
    /// a fresh uninspected edge from the successor carries the old locator/role
    /// and a `stale_cause: rehomed` so the analyze queue re-earns the claim.
    pub fn rehome_grounding(
        &self,
        edge_id: &str,
        successor_intent_id: &str,
        reason: &str,
    ) -> Result<(Edge, Edge)> {
        let old = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("no edge '{edge_id}'"))?;
        if old.kind != EdgeKind::Implements {
            bail!(
                "rehome is for grounding (implements) edges; '{edge_id}' is a {} edge",
                old.kind
            );
        }
        if old.truth_class != TruthClass::Asserted {
            bail!("cannot rehome a derived edge");
        }
        self.check_lane(registry::spec(EdgeKind::Implements).owner)?;
        let reason = reason.trim();
        if reason.is_empty() {
            bail!("rehome requires a reason");
        }
        if old.from_id == successor_intent_id {
            bail!("rehome successor is the current owner — use set-role for a role change");
        }
        // The new home carries the same file, locator, and role, and starts
        // unverified: `ensure_edge` may return an already-settled successor
        // grounding, so re-open it explicitly. `stale_cause: rehomed` routes it
        // through the analyze queue to re-earn the claim on the new intent.
        let new = self.ensure_edge(EdgeKind::Implements, successor_intent_id, &old.to_id)?;
        if let Some(loc) = self.get_facet(edge_id, TargetKind::Edge, "locator")? {
            self.set_facet(
                &new.id,
                TargetKind::Edge,
                "locator",
                &loc,
                TruthClass::Asserted,
            )?;
        }
        let role = self.grounding_role(edge_id)?;
        self.set_grounding_role(&new.id, role)?;
        let cause = format!("rehomed from '{}': {reason}", old.from_id);
        // Re-open a settled successor; then stamp the cause so a freshly-created
        // (already-uninspected) successor also carries the rehome context.
        self.stale_edge(&new.id, &cause)?;
        self.set_facet(
            &new.id,
            TargetKind::Edge,
            "stale_cause",
            &cause,
            TruthClass::Derived,
        )?;
        // Supersede the old edge (history, not counted).
        self.set_facet(
            edge_id,
            TargetKind::Edge,
            "superseded_by",
            &new.id,
            TruthClass::Asserted,
        )?;
        self.add_note(
            &old.from_id,
            "decision",
            &format!("grounding rehomed to intent {successor_intent_id}: {reason}"),
        )?;
        let new = self
            .get_edge(&new.id)?
            .ok_or_else(|| anyhow!("edge vanished after rehome"))?;
        Ok((old, new))
    }

    /// Asserted edges of the given statuses, excluding any superseded by a
    /// `rehome`. Work queues and maturity counts read this so a superseded
    /// grounding (history) never re-enters a lane as live work.
    pub fn live_edges_by_status(
        &self,
        truth: TruthClass,
        statuses: &[InspectionStatus],
    ) -> Result<Vec<Edge>> {
        let mut out = Vec::new();
        for e in self.edges_by_status(truth, statuses)? {
            if !self.edge_superseded(&e.id)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    pub fn list_edges(&self, kind: Option<EdgeKind>, limit: usize) -> Result<Vec<Edge>> {
        let mut stmt;
        let rows = if let Some(k) = kind {
            stmt = self.conn.prepare(
                "SELECT id,from_id,to_id,kind,truth_class,status,criterion,evidence,
                        confidence,depends_on,inspected_by,created_at,updated_at
                 FROM edge WHERE kind=?1 ORDER BY id LIMIT ?2",
            )?;
            stmt.query_map(params![k.as_str(), limit as i64], row_to_edge)?
        } else {
            stmt = self.conn.prepare(
                "SELECT id,from_id,to_id,kind,truth_class,status,criterion,evidence,
                        confidence,depends_on,inspected_by,created_at,updated_at
                 FROM edge ORDER BY id LIMIT ?1",
            )?;
            stmt.query_map(params![limit as i64], row_to_edge)?
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Edges of a given truth class whose status is in `statuses`. The asserted
    /// residue query (`loom next`): truth_class='asserted', stale/uninspected/failing.
    pub fn edges_by_status(
        &self,
        truth: TruthClass,
        statuses: &[InspectionStatus],
    ) -> Result<Vec<Edge>> {
        if statuses.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> =
            (0..statuses.len()).map(|i| format!("?{}", i + 2)).collect();
        let sql = format!(
            "SELECT {EDGE_COLS} FROM edge WHERE truth_class=?1 AND status IN ({}) ORDER BY id",
            placeholders.join(",")
        );
        let mut args: Vec<String> = vec![truth.as_str().to_string()];
        for s in statuses {
            args.push(s.as_str().to_string());
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), row_to_edge)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Nodes of a type whose `status` (lifecycle) is in `statuses`.
    pub fn nodes_by_status(&self, node_type: NodeType, statuses: &[&str]) -> Result<Vec<Node>> {
        let all = self.list_nodes(Some(node_type), usize::MAX)?;
        Ok(all
            .into_iter()
            .filter(|n| statuses.contains(&n.status.as_str()))
            .collect())
    }
}
