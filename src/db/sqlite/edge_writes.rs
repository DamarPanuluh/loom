use super::SqliteGraphStore;
use super::*;

impl SqliteGraphStore {
    pub fn insert_implements(
        &self,
        intent_id: &str,
        codefile_id: &str,
        locator: &str,
        notes: &str,
        now: &str,
    ) -> Result<()> {
        let changed = self.write_one(
            "INSERT INTO implements(
                intent_id, codefile_id, inspection_status, criterion, confidence, evidence,
                last_inspected, inspected_by, locator, notes, created_at
             )
             SELECT ?1, ?2, 'passing', '', 0, '', '', '', ?3, ?4, ?5
             WHERE EXISTS(SELECT 1 FROM intent WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM codefile WHERE id = ?2)
             ON CONFLICT(intent_id, codefile_id) DO UPDATE SET
                inspection_status = 'passing',
                criterion = '',
                confidence = 0,
                evidence = '',
                last_inspected = '',
                inspected_by = '',
                locator = excluded.locator,
                notes = excluded.notes",
            params![intent_id, codefile_id, locator, notes, now],
        )?;
        if changed == 0 {
            let intent_exists = self
                .conn
                .query_row(
                    "SELECT 1 FROM intent WHERE id = ?1",
                    params![intent_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !intent_exists {
                anyhow::bail!("Intent '{}' not found — `loom intent list`.", intent_id);
            }
            let codefile_exists = self
                .conn
                .query_row(
                    "SELECT 1 FROM codefile WHERE id = ?1",
                    params![codefile_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !codefile_exists {
                anyhow::bail!(
                    "CodeFile '{}' not found. Add it with `loom codefile add` first.",
                    codefile_id
                );
            }
        }
        Ok(())
    }
    pub fn delete_implements(&self, intent_id: &str, codefile_id: &str) -> Result<bool> {
        let changed = self.write_one(
            "DELETE FROM implements WHERE intent_id = ?1 AND codefile_id = ?2",
            params![intent_id, codefile_id],
        )?;
        Ok(changed > 0)
    }
    pub fn insert_hierarchy(
        &self,
        parent_id: &str,
        child_id: &str,
        notes: &str,
        now: &str,
    ) -> Result<()> {
        let endpoint_count: i64 = self.conn.query_row(
            "SELECT count(*) FROM intent WHERE id IN (?1, ?2)",
            params![parent_id, child_id],
            |row| row.get(0),
        )?;
        if endpoint_count < 2 {
            anyhow::bail!(
                "Cannot create HIERARCHY: one or both intents not found.\n\
                 parent id: {}\nchild id: {} — `loom intent list` to verify; \
                 `loom intent add` if missing.",
                parent_id,
                child_id
            );
        }

        let existing_parent: Option<String> = self
            .conn
            .query_row(
                "SELECT parent_id FROM hierarchy WHERE child_id = ?1",
                params![child_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing_parent) = existing_parent {
            if existing_parent == parent_id {
                anyhow::bail!(
                    "HIERARCHY {} -> {} already exists. Already recorded — \
                     `loom intent show {}` displays the tree; cross-cutting \
                     relationships belong in `loom edge explore`.",
                    parent_id,
                    child_id,
                    child_id
                );
            }
            anyhow::bail!(
                "Cannot add parent: intent '{}' already has parent '{}'.\n\
                 HIERARCHY is a tree — each intent has exactly one parent. Use \
                 `loom edge explore` (RELATES_TO) for cross-cutting links.",
                child_id,
                existing_parent
            );
        }

        let existing = self.hierarchy_pairs()?;
        if hierarchy_reaches(&existing, child_id, parent_id) {
            anyhow::bail!(
                "Cannot add HIERARCHY {} -> {}: it would create a cycle (the child is \
                 already an ancestor of the parent). Choose a different parent; if the \
                 relationship is cross-cutting rather than structural, record it with \
                 `loom edge explore` instead.",
                parent_id,
                child_id
            );
        }

        self.write_one(
            "INSERT INTO hierarchy(parent_id, child_id, notes, created_at)
             VALUES(?1, ?2, ?3, ?4)",
            params![parent_id, child_id, notes, now],
        )?;
        Ok(())
    }
    /// Set the relationship-kind multiset on a RELATES_TO edge (the taxonomy
    /// program's `populate kinds` backfill + judgment assignment write here).
    pub fn update_relates_to_kinds(
        &self,
        from_id: &str,
        to_id: &str,
        kinds: &[String],
    ) -> Result<()> {
        self.write_one(
            "UPDATE relates_to SET kinds = ?1 WHERE from_id = ?2 AND to_id = ?3",
            params![serde_json::to_string(kinds)?, from_id, to_id],
        )?;
        Ok(())
    }
    pub fn get_or_create_relates_to(
        &self,
        from_id: &str,
        to_id: &str,
        now: &str,
    ) -> Result<RelatesTo> {
        super::get_or_create_relates_to_conn(&self.conn, from_id, to_id, now)
    }
    /// Set (or clear) the `stable` low-churn flag on a RELATES_TO edge. Returns
    /// false when no such edge exists. A stable edge is exempt from `loom sync`
    /// code-change reverification (see sync.rs).
    pub fn set_relates_to_stable(
        &mut self,
        from_id: &str,
        to_id: &str,
        stable: bool,
    ) -> Result<bool> {
        let value = if stable { "true" } else { "" };
        let tx = self.write_tx()?;
        let changed = tx.execute(
            "UPDATE relates_to SET stable = ?1 WHERE from_id = ?2 AND to_id = ?3",
            params![value, from_id, to_id],
        )?;
        tx.commit()?;
        Ok(changed > 0)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_relates_to_ground(
        &mut self,
        from_id: &str,
        to_id: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
    ) -> Result<RelatesTo> {
        let tx = self.write_tx()?;
        let edge = super::get_or_create_relates_to_conn(&tx, from_id, to_id, now)?;
        tx.execute(
            "UPDATE relates_to
             SET inspection_status = 'passing',
                 criterion = ?1,
                 evidence = ?2,
                 confidence = ?3,
                 inspected_by = ?4,
                 last_inspected = ?5
             WHERE from_id = ?6 AND to_id = ?7",
            params![
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                from_id,
                to_id
            ],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &edge.id,
            &edge.inspection_status,
            "passing",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(edge)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_relates_to_issue(
        &mut self,
        from_id: &str,
        to_id: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
    ) -> Result<RelatesTo> {
        let tx = self.write_tx()?;
        let edge = super::get_or_create_relates_to_conn(&tx, from_id, to_id, now)?;
        tx.execute(
            "UPDATE relates_to
             SET inspection_status = 'failing',
                 criterion = ?1,
                 evidence = ?2,
                 confidence = ?3,
                 inspected_by = ?4,
                 last_inspected = ?5
             WHERE from_id = ?6 AND to_id = ?7",
            params![
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                from_id,
                to_id
            ],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &edge.id,
            &edge.inspection_status,
            "failing",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(edge)
    }
    pub fn upsert_relates_to_independent(
        &mut self,
        from_id: &str,
        to_id: &str,
        notes: &str,
        inspected_by: &str,
        now: &str,
    ) -> Result<RelatesTo> {
        let tx = self.write_tx()?;
        let edge = super::get_or_create_relates_to_conn(&tx, from_id, to_id, now)?;
        tx.execute(
            "UPDATE relates_to
             SET inspection_status = 'independent',
                 notes = ?1,
                 inspected_by = ?2,
                 last_inspected = ?3
             WHERE from_id = ?4 AND to_id = ?5",
            params![notes, inspected_by, now, from_id, to_id],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &edge.id,
            &edge.inspection_status,
            "independent",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(edge)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update_relates_to_ground(
        &mut self,
        from_id: &str,
        to_id: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_relates_to_between(from_id, to_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE relates_to
             SET inspection_status = 'passing',
                 criterion = ?1,
                 evidence = ?2,
                 confidence = ?3,
                 inspected_by = ?4,
                 last_inspected = ?5
             WHERE from_id = ?6 AND to_id = ?7",
            params![
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                from_id,
                to_id
            ],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &prev.id,
            &prev.inspection_status,
            "passing",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update_relates_to_issue(
        &mut self,
        from_id: &str,
        to_id: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_relates_to_between(from_id, to_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE relates_to
             SET inspection_status = 'failing',
                 criterion = ?1,
                 evidence = ?2,
                 confidence = ?3,
                 inspected_by = ?4,
                 last_inspected = ?5
             WHERE from_id = ?6 AND to_id = ?7",
            params![
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                from_id,
                to_id
            ],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &prev.id,
            &prev.inspection_status,
            "failing",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn update_relates_to_independent(
        &mut self,
        from_id: &str,
        to_id: &str,
        notes: &str,
        inspected_by: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_relates_to_between(from_id, to_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE relates_to
             SET inspection_status = 'independent',
                 notes = ?1,
                 inspected_by = ?2,
                 last_inspected = ?3
             WHERE from_id = ?4 AND to_id = ?5",
            params![notes, inspected_by, now, from_id, to_id],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &prev.id,
            &prev.inspection_status,
            "independent",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn flag_relates_to_needs_reverification(
        &mut self,
        edge: &RelatesTo,
        cause: &str,
        now: &str,
    ) -> Result<bool> {
        if edge.inspection_status != "passing" && edge.inspection_status != "independent" {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        super::stale_relates_to(&tx, &edge.from_id, &edge.to_id)?;
        insert_sync_flip_note_tx(
            &tx,
            "edge",
            &edge.id,
            &edge.inspection_status,
            "needs_reverification",
            cause,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn insert_validates(
        &self,
        validation_id: &str,
        intent_id: &str,
        notes: &str,
        now: &str,
    ) -> Result<()> {
        let changed = self.write_one(
            "INSERT OR IGNORE INTO validates(
                validation_id, intent_id, inspection_status, notes, created_at
             )
             SELECT ?1, ?2, 'uninspected', ?3, ?4
             WHERE EXISTS(SELECT 1 FROM validation WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)",
            params![validation_id, intent_id, notes, now],
        )?;
        if changed == 0 {
            let validation_exists = self
                .conn
                .query_row(
                    "SELECT 1 FROM validation WHERE id = ?1",
                    params![validation_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !validation_exists {
                anyhow::bail!(
                    "Validation '{}' not found — `loom validation list`.",
                    validation_id
                );
            }
            let intent_exists = self
                .conn
                .query_row(
                    "SELECT 1 FROM intent WHERE id = ?1",
                    params![intent_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !intent_exists {
                anyhow::bail!("Intent '{}' not found — `loom intent list`.", intent_id);
            }
        }
        Ok(())
    }
    pub fn insert_call(
        &self,
        validation_id: &str,
        interface_id: &str,
        step_index: usize,
        step_name: &str,
        intent_id: &str,
        now: &str,
    ) -> Result<()> {
        self.write_one(
            "INSERT OR REPLACE INTO calls(
                validation_id, interface_id, step_index, step_name, intent_id, notes, created_at
             )
             SELECT ?1, ?2, ?3, ?4, ?5, '', ?6
             WHERE EXISTS(SELECT 1 FROM validation WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM interface_surface WHERE id = ?2)
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?5)",
            params![
                validation_id,
                interface_id,
                step_index.to_string(),
                step_name,
                intent_id,
                now
            ],
        )?;
        Ok(())
    }
    pub fn delete_calls_for_validation(&self, validation_id: &str) -> Result<usize> {
        let deleted = self.write_one(
            "DELETE FROM calls WHERE validation_id = ?1",
            params![validation_id],
        )?;
        Ok(deleted)
    }
    pub fn get_or_create_serves(
        &self,
        persona_id: &str,
        intent_id: &str,
        now: &str,
    ) -> Result<ServesEdge> {
        self.write_one(
            "INSERT OR IGNORE INTO serves(
                persona_id, intent_id, inspection_status, criterion, confidence, evidence,
                last_inspected, inspected_by, notes, created_at
             )
             SELECT ?1, ?2, 'uninspected', '', 0, '', '', '', '', ?3
             WHERE EXISTS(SELECT 1 FROM persona WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)",
            params![persona_id, intent_id, now],
        )?;
        match self.get_serves_between(persona_id, intent_id)? {
            Some(edge) => Ok(edge),
            None => anyhow::bail!(
                "Cannot create SERVES edge: persona or intent not found.\n\
                 persona id: {}\n\
                 intent id: {}\n\
                 Run `loom persona list` and `loom intent list` to see available nodes.",
                persona_id,
                intent_id
            ),
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update_serves_ground(
        &mut self,
        persona_id: &str,
        intent_id: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_serves_between(persona_id, intent_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE serves
             SET inspection_status = 'passing',
                 criterion = ?1,
                 evidence = ?2,
                 confidence = ?3,
                 inspected_by = ?4,
                 last_inspected = ?5
             WHERE persona_id = ?6 AND intent_id = ?7",
            params![
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                persona_id,
                intent_id
            ],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &prev.id,
            &prev.inspection_status,
            "passing",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update_serves_issue(
        &mut self,
        persona_id: &str,
        intent_id: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_serves_between(persona_id, intent_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE serves
             SET inspection_status = 'failing',
                 criterion = ?1,
                 evidence = ?2,
                 confidence = ?3,
                 inspected_by = ?4,
                 last_inspected = ?5
             WHERE persona_id = ?6 AND intent_id = ?7",
            params![
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                persona_id,
                intent_id
            ],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &prev.id,
            &prev.inspection_status,
            "failing",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn update_serves_independent(
        &mut self,
        persona_id: &str,
        intent_id: &str,
        notes: &str,
        inspected_by: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_serves_between(persona_id, intent_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE serves
             SET inspection_status = 'independent',
                 notes = ?1,
                 inspected_by = ?2,
                 last_inspected = ?3
             WHERE persona_id = ?4 AND intent_id = ?5",
            params![notes, inspected_by, now, persona_id, intent_id],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &prev.id,
            &prev.inspection_status,
            "independent",
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn get_or_create_journeys(
        &self,
        persona_id: &str,
        validation_id: &str,
        now: &str,
    ) -> Result<JourneysEdge> {
        self.write_one(
            "INSERT OR IGNORE INTO journeys(persona_id, validation_id, notes, created_at)
             SELECT ?1, ?2, '', ?3
             WHERE EXISTS(SELECT 1 FROM persona WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM validation WHERE id = ?2)",
            params![persona_id, validation_id, now],
        )?;
        match self.get_journeys_between(persona_id, validation_id)? {
            Some(edge) => Ok(edge),
            None => anyhow::bail!(
                "Cannot create JOURNEYS edge: persona or validation not found.\n\
                 persona id: {}\n\
                 validation id: {}\n\
                 Run `loom persona list` and `loom validation list` to see available nodes.",
                persona_id,
                validation_id
            ),
        }
    }
    pub fn insert_targets(&self, hypothesis_id: &str, intent_id: &str, now: &str) -> Result<()> {
        let changed = self.write_one(
            "INSERT OR IGNORE INTO targets(
                hypothesis_id, intent_id, inspection_status, criterion, confidence, evidence,
                last_inspected, inspected_by, notes, created_at
             )
             SELECT ?1, ?2, 'uninspected', '', 0, '', '', '', '', ?3
             WHERE EXISTS(SELECT 1 FROM hypothesis WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)",
            params![hypothesis_id, intent_id, now],
        )?;
        if changed == 0
            && self
                .get_targets_between(hypothesis_id, intent_id)?
                .is_none()
        {
            anyhow::bail!(
                "Cannot create TARGETS edge: hypothesis or intent not found.\n\
                 hypothesis id: {}\nintent id: {}",
                hypothesis_id,
                intent_id
            );
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn set_targets_status_for_hypothesis(
        &mut self,
        hypothesis_id: &str,
        status: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
    ) -> Result<usize> {
        let previous = self.list_targets_for_hypothesis(hypothesis_id)?;
        let tx = self.write_tx()?;
        let changed = tx.execute(
            "UPDATE targets
             SET inspection_status = ?1,
                 criterion = ?2,
                 evidence = ?3,
                 confidence = ?4,
                 inspected_by = ?5,
                 last_inspected = ?6
             WHERE hypothesis_id = ?7",
            params![
                status,
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                hypothesis_id
            ],
        )?;
        for edge in previous {
            insert_transition_note_tx(
                &tx,
                "edge",
                &edge.id,
                &edge.inspection_status,
                status,
                inspected_by,
                now,
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }
    pub fn flag_targets_needs_reverification(
        &mut self,
        edge: &TargetsEdge,
        cause: &str,
        now: &str,
    ) -> Result<bool> {
        if edge.inspection_status != "passing" {
            return Ok(false);
        }
        // A confirmed hypothesis is settled: its TARGETS lineage is historical and
        // `prove` (the only re-stamper) is closed, so staling here would strand the
        // edge in needs_reverification forever. The live proof is the spawned intents'
        // validations, not this evidence edge — so leave it passing.
        let on_confirmed = self
            .conn
            .query_row(
                "SELECT 1 FROM hypothesis WHERE id = ?1 AND status = 'confirmed'",
                params![edge.hypothesis_id],
                |_| Ok(()),
            )
            .is_ok();
        if on_confirmed {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        super::stale_targets(&tx, &edge.hypothesis_id, &edge.intent_id)?;
        insert_sync_flip_note_tx(
            &tx,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            cause,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    /// Reconcile settled hypothesis lineage: a confirmed hypothesis's TARGETS are
    /// historical (prove is closed, the live proof is the spawned intents'
    /// validations), so any left `needs_reverification` — e.g. staled before sync
    /// learned to skip them — are returned to `passing`. Returns the count cleared.
    pub fn settle_confirmed_hypothesis_targets(&mut self) -> Result<usize> {
        let tx = self.write_tx()?;
        let n = tx.execute(
            "UPDATE targets SET inspection_status = 'passing'
             WHERE inspection_status = 'needs_reverification'
               AND hypothesis_id IN (SELECT id FROM hypothesis WHERE status = 'confirmed')",
            [],
        )?;
        tx.commit()?;
        Ok(n)
    }
    pub fn flag_serves_needs_reverification(
        &mut self,
        edge: &ServesEdge,
        cause: &str,
        now: &str,
    ) -> Result<bool> {
        if edge.inspection_status != "passing" {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        super::stale_serves(&tx, &edge.persona_id, &edge.intent_id)?;
        insert_sync_flip_note_tx(
            &tx,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            cause,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    pub fn flag_implements_needs_reverification(
        &self,
        intent_id: &str,
        codefile_id: &str,
    ) -> Result<bool> {
        let changed = self.write_one(
            "UPDATE implements
             SET inspection_status = 'needs_reverification'
             WHERE intent_id = ?1 AND codefile_id = ?2 AND inspection_status = 'passing'",
            params![intent_id, codefile_id],
        )?;
        Ok(changed > 0)
    }
    pub fn insert_governs(
        &self,
        rule_id: &str,
        intent_id: &str,
        criterion: &str,
        now: &str,
    ) -> Result<()> {
        let changed = self.write_one(
            "INSERT OR IGNORE INTO governs(
                rule_id, intent_id, inspection_status, criterion, confidence, evidence,
                last_inspected, inspected_by, notes, created_at
             )
             SELECT ?1, ?2, 'uninspected', ?3, 0, '', '', '', '', ?4
             WHERE EXISTS(SELECT 1 FROM quality_rule WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)",
            params![rule_id, intent_id, criterion, now],
        )?;
        if changed == 0 {
            let rule_exists = self
                .conn
                .query_row(
                    "SELECT 1 FROM quality_rule WHERE id = ?1",
                    params![rule_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !rule_exists {
                anyhow::bail!(
                    "QualityRule '{}' not found — `loom rule list` shows registered rules.",
                    rule_id
                );
            }
            let intent_exists = self
                .conn
                .query_row(
                    "SELECT 1 FROM intent WHERE id = ?1",
                    params![intent_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !intent_exists {
                anyhow::bail!("Intent '{}' not found — `loom intent list`.", intent_id);
            }
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update_governs_verdict(
        &mut self,
        rule_id: &str,
        intent_id: &str,
        status: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
        covers_descendants: bool,
    ) -> Result<bool> {
        let previous = self
            .list_governs_for_intent(intent_id)?
            .into_iter()
            .find(|edge| edge.rule_id == rule_id);
        let Some(previous) = previous else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE governs
             SET inspection_status = ?1,
                 criterion = ?2,
                 evidence = ?3,
                 confidence = ?4,
                 inspected_by = ?5,
                 last_inspected = ?6,
                 covers_descendants = ?7
             WHERE rule_id = ?8 AND intent_id = ?9",
            params![
                status,
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                if covers_descendants { "true" } else { "" },
                rule_id,
                intent_id
            ],
        )?;
        insert_transition_note_tx(
            &tx,
            "edge",
            &previous.id,
            &previous.inspection_status,
            status,
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_governs_verdict(
        &mut self,
        rule_id: &str,
        intent_id: &str,
        status: &str,
        criterion: &str,
        evidence: &str,
        confidence: f64,
        inspected_by: &str,
        now: &str,
        covers_descendants: bool,
    ) -> Result<()> {
        let tx = self.write_tx()?;
        let previous_status = tx
            .query_row(
                "SELECT inspection_status FROM governs WHERE rule_id = ?1 AND intent_id = ?2",
                params![rule_id, intent_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let previous_status = if let Some(previous_status) = previous_status {
            previous_status
        } else {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO governs(
                    rule_id, intent_id, inspection_status, criterion, confidence, evidence,
                    last_inspected, inspected_by, notes, created_at
                 )
                 SELECT ?1, ?2, 'uninspected', ?3, 0, '', '', '', '', ?4
                 WHERE EXISTS(SELECT 1 FROM quality_rule WHERE id = ?1)
                   AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)",
                params![rule_id, intent_id, criterion, now],
            )?;
            if changed == 0 {
                let rule_exists = tx
                    .query_row(
                        "SELECT 1 FROM quality_rule WHERE id = ?1",
                        params![rule_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !rule_exists {
                    anyhow::bail!(
                        "QualityRule '{}' not found — `loom rule list` shows registered rules.",
                        rule_id
                    );
                }
                let intent_exists = tx
                    .query_row(
                        "SELECT 1 FROM intent WHERE id = ?1",
                        params![intent_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !intent_exists {
                    anyhow::bail!("Intent '{}' not found — `loom intent list`.", intent_id);
                }
            }
            "uninspected".to_string()
        };
        tx.execute(
            "UPDATE governs
             SET inspection_status = ?1,
                 criterion = ?2,
                 evidence = ?3,
                 confidence = ?4,
                 inspected_by = ?5,
                 last_inspected = ?6,
                 covers_descendants = ?7
             WHERE rule_id = ?8 AND intent_id = ?9",
            params![
                status,
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
                if covers_descendants { "true" } else { "" },
                rule_id,
                intent_id
            ],
        )?;
        let edge_id = crate::db::schema::edge_key(edge::GOVERNS, rule_id, intent_id);
        insert_transition_note_tx(
            &tx,
            "edge",
            &edge_id,
            &previous_status,
            status,
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn flag_governs_needs_reverification(
        &mut self,
        edge: &Governs,
        cause: &str,
        now: &str,
    ) -> Result<bool> {
        if edge.inspection_status != "passing" {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        super::stale_governs(&tx, &edge.rule_id, &edge.intent_id)?;
        insert_sync_flip_note_tx(
            &tx,
            "edge",
            &edge.id,
            "passing",
            "needs_reverification",
            cause,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
}
