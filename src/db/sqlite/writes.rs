use super::SqliteGraphStore;
use super::*;

impl SqliteGraphStore {
    pub fn set_intent_tags(&self, id: &str, tags: Vec<String>, updated_at: &str) -> Result<bool> {
        let encoded = crate::db::queries::vocab::encode_tags(tags)?;
        let changed = self.write_one(
            SQL_UPDATE_INTENT_TAGS,
            params![serde_json::to_string(&encoded)?, updated_at, id],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    pub fn add_source_ref(
        &mut self,
        id: &str,
        path: &str,
        updated_at: &str,
    ) -> Result<Option<Vec<String>>> {
        // Atomic read-modify-write: read the current refs and write the appended
        // list inside ONE write transaction. A plain get-then-set let a
        // concurrent writer's append land between the read and the write and be
        // silently overwritten (lost-update).
        let tx = self.write_tx()?;
        let mut refs = match read_source_refs_in_tx(&tx, id)? {
            Some(refs) => refs,
            None => return Ok(None),
        };
        if !refs.iter().any(|source_ref| source_ref == path) {
            refs.push(path.to_string());
            tx.execute(
                SQL_UPDATE_INTENT_SOURCE_REFS,
                params![serde_json::to_string(&refs)?, updated_at, id],
            )?;
        }
        tx.commit()?;
        Ok(Some(refs))
    }
}

impl SqliteGraphStore {
    pub fn remove_source_ref(
        &mut self,
        id: &str,
        path: &str,
        updated_at: &str,
    ) -> Result<Option<bool>> {
        let tx = self.write_tx()?;
        let mut refs = match read_source_refs_in_tx(&tx, id)? {
            Some(refs) => refs,
            None => return Ok(None),
        };
        let before = refs.len();
        refs.retain(|source_ref| source_ref != path);
        if refs.len() == before {
            return Ok(Some(false));
        }
        tx.execute(
            SQL_UPDATE_INTENT_SOURCE_REFS,
            params![serde_json::to_string(&refs)?, updated_at, id],
        )?;
        tx.commit()?;
        Ok(Some(true))
    }
}

impl SqliteGraphStore {
    pub fn initialize(
        &self,
        schema_version: &str,
        graph_id: &str,
        graph_name: &str,
        custody: &str,
        created_at: &str,
    ) -> Result<bool> {
        let changed = self.write_one(
            "INSERT OR IGNORE INTO meta(
                id, schema_version, graph_id, graph_name, custody, created_at,
                last_synced, transition_cap, layer_order
             ) VALUES(1, ?1, ?2, ?3, ?4, ?5, '', '', '[]')",
            params![schema_version, graph_id, graph_name, custody, created_at],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    pub fn set_identity(&self, graph_id: &str, graph_name: &str, custody: &str) -> Result<()> {
        self.write_one(
            "UPDATE meta SET graph_id = ?1, graph_name = ?2, custody = ?3 WHERE id = 1",
            params![graph_id, graph_name, custody],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn insert_intent(&self, intent: &Intent) -> Result<()> {
        self.write_one(
            "INSERT INTO intent(
                id, name, description, abstraction_level, domain, layer, source_refs,
                status, aspect, tags, visibility, boundary, lifecycle, created_at, updated_at,
                criterion
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                intent.id,
                intent.name,
                intent.description,
                intent.abstraction_level,
                intent.domain,
                intent.layer,
                serde_json::to_string(&intent.source_refs)?,
                intent.status,
                intent.aspect,
                serde_json::to_string(&intent.tags)?,
                intent.visibility,
                intent.boundary,
                intent.lifecycle,
                intent.created_at,
                intent.updated_at,
                intent.criterion
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn confirm_intent(
        &mut self,
        id: &str,
        visibility: Option<&str>,
        author: &str,
        now: &str,
    ) -> Result<bool> {
        if !matches!(visibility, None | Some("user_visible") | Some("internal")) {
            anyhow::bail!(
                "Invalid --visibility '{}'. Valid: user_visible | internal",
                visibility.unwrap_or_default()
            );
        }
        let exists = self.get_intent(id)?.is_some();
        if !exists {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE intent SET status = 'confirmed', updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        tx.execute(
            "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
             VALUES(?1, 'confirm', 'meaning re-affirmed', ?2, 'intent', ?3, ?4, '')",
            params![uuid::Uuid::new_v4().to_string(), author, id, now],
        )?;
        if let Some(visibility) = visibility {
            tx.execute(SQL_UPDATE_INTENT_VISIBILITY, params![visibility, now, id])?;
            tx.execute(
                "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
                 VALUES(?1, 'decision', ?2, ?3, 'intent', ?4, ?5, '')",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    format!("visibility ruled {visibility} during alignment"),
                    author,
                    id,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn set_intent_lifecycle(
        &mut self,
        id: &str,
        lifecycle: &str,
        author: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_intent(id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE intent SET lifecycle = ?1, updated_at = ?2 WHERE id = ?3",
            params![lifecycle, now, id],
        )?;
        insert_transition_note_tx(&tx, "intent", id, &prev.lifecycle, lifecycle, author, now)?;
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn set_intent_visibility(
        &self,
        id: &str,
        visibility: &str,
        updated_at: &str,
    ) -> Result<bool> {
        if !matches!(visibility, "" | "user_visible" | "internal") {
            anyhow::bail!(
                "Invalid visibility '{visibility}'. Valid: user_visible | internal | \"\"."
            );
        }
        let changed = self.write_one(
            SQL_UPDATE_INTENT_VISIBILITY,
            params![visibility, updated_at, id],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    pub fn set_intent_layer(&self, id: &str, layer: &str, updated_at: &str) -> Result<bool> {
        let changed = self.write_one(
            "UPDATE intent SET layer = ?1, updated_at = ?2 WHERE id = ?3",
            params![layer, updated_at, id],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    /// Set the intent's first-class falsifiable criterion (v10). The caller
    /// records the prior value in a decision note (the version chain).
    pub fn set_intent_criterion(
        &self,
        id: &str,
        criterion: &str,
        updated_at: &str,
    ) -> Result<bool> {
        let changed = self.write_one(
            "UPDATE intent SET criterion = ?1, updated_at = ?2 WHERE id = ?3",
            params![criterion, updated_at, id],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    pub fn set_intent_boundary(&self, id: &str, boundary: &str, updated_at: &str) -> Result<bool> {
        if !matches!(boundary, "" | "inbound" | "outbound") {
            anyhow::bail!("Invalid boundary '{boundary}'. Valid: inbound | outbound | \"\".");
        }
        let changed = self.write_one(
            "UPDATE intent SET boundary = ?1, updated_at = ?2 WHERE id = ?3",
            params![boundary, updated_at, id],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    /// Set the intent's product/business domain facet (auth, billing, …). Metadata
    /// only — domains are not the architecture layer order, so this never ripples.
    pub fn set_intent_domain(&self, id: &str, domain: &str, updated_at: &str) -> Result<bool> {
        let changed = self.write_one(
            "UPDATE intent SET domain = ?1, updated_at = ?2 WHERE id = ?3",
            params![domain, updated_at, id],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    /// Set the intent's behavioural aspect (happy/sad/fallback/edge_case, or "" to
    /// clear). Metadata only — the happy_path_only audit reads it but no edge ripples.
    pub fn set_intent_aspect(&self, id: &str, aspect: &str, updated_at: &str) -> Result<bool> {
        if !matches!(aspect, "" | "happy" | "sad" | "fallback" | "edge_case") {
            anyhow::bail!(
                "Invalid aspect '{aspect}'. Valid: happy | sad | fallback | edge_case | \"\"."
            );
        }
        let changed = self.write_one(
            "UPDATE intent SET aspect = ?1, updated_at = ?2 WHERE id = ?3",
            params![aspect, updated_at, id],
        )?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    pub fn update_intent_meaning(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        updated_at: &str,
    ) -> Result<bool> {
        if self.get_intent(id)?.is_none() {
            return Ok(false);
        }
        match (name, description) {
            (Some(name), Some(description)) => {
                self.write_one(
                    "UPDATE intent SET name = ?1, description = ?2, updated_at = ?3 WHERE id = ?4",
                    params![name, description, updated_at, id],
                )?;
            }
            (Some(name), None) => {
                self.write_one(
                    "UPDATE intent SET name = ?1, updated_at = ?2 WHERE id = ?3",
                    params![name, updated_at, id],
                )?;
            }
            (None, Some(description)) => {
                self.write_one(
                    "UPDATE intent SET description = ?1, updated_at = ?2 WHERE id = ?3",
                    params![description, updated_at, id],
                )?;
            }
            (None, None) => {
                self.write_one(
                    "UPDATE intent SET updated_at = ?1 WHERE id = ?2",
                    params![updated_at, id],
                )?;
            }
        }
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn ripple_intent_redefinition(
        &mut self,
        intent_id: &str,
        intent_name: &str,
        now: &str,
    ) -> Result<RedefinitionRipple> {
        let cause = format!("intent '{intent_name}' redefined");
        let relates = self.edges_for_intent(intent_id)?;
        let governs = self.list_governs_for_intent(intent_id)?;
        let targets = self.list_all_targets()?;
        let implements = self.list_implements_for_intent(intent_id)?;
        let validates = self.list_all_validates()?;

        let tx = self.write_tx()?;
        let mut ripple = RedefinitionRipple::default();

        for edge in relates {
            if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
                super::stale_relates_to(&tx, &edge.from_id, &edge.to_id)?;
                insert_sync_flip_note_tx(
                    &tx,
                    "edge",
                    &edge.id,
                    &edge.inspection_status,
                    "needs_reverification",
                    &cause,
                    now,
                )?;
                ripple.relates_to_flagged += 1;
            }
        }

        for edge in governs {
            if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
                super::stale_governs(&tx, &edge.rule_id, &edge.intent_id)?;
                insert_sync_flip_note_tx(
                    &tx,
                    "edge",
                    &edge.id,
                    &edge.inspection_status,
                    "needs_reverification",
                    &cause,
                    now,
                )?;
                ripple.governs_flagged += 1;
            }
        }

        for edge in targets {
            if edge.intent_id == intent_id && edge.inspection_status == "passing" {
                super::stale_targets(&tx, &edge.hypothesis_id, &edge.intent_id)?;
                insert_sync_flip_note_tx(
                    &tx,
                    "edge",
                    &edge.id,
                    &edge.inspection_status,
                    "needs_reverification",
                    &cause,
                    now,
                )?;
                ripple.targets_flagged += 1;
            }
        }

        for edge in implements {
            if edge.inspection_status == "passing" {
                tx.execute(
                    SQL_FLAG_IMPLEMENTS_STALE,
                    params![edge.intent_id, edge.codefile_id],
                )?;
                insert_sync_flip_note_tx(
                    &tx,
                    "edge",
                    &edge.id,
                    &edge.inspection_status,
                    "needs_reverification",
                    &cause,
                    now,
                )?;
                ripple.implements_flagged += 1;
            }
        }

        for edge in validates {
            if edge.intent_id != intent_id {
                continue;
            }
            let result: Option<String> = tx
                .query_row(
                    SQL_SELECT_VALIDATION_LAST_RESULT,
                    params![edge.validation_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(result) = result {
                if result != "not_run" && result != "blocked" && !result.is_empty() {
                    tx.execute(SQL_RESET_VALIDATION, params![edge.validation_id])?;
                    ripple.validations_invalidated += 1;
                }
            }
        }

        tx.commit()?;
        Ok(ripple)
    }
}

impl SqliteGraphStore {
    pub fn retire_intent(
        &mut self,
        id: &str,
        reason: &str,
        replaced_by: Option<&str>,
        now: &str,
    ) -> Result<bool> {
        let Some(prev) = self.get_intent(id)? else {
            return Ok(false);
        };
        let relates = self.edges_for_intent(id)?;
        // Retirement also stales verdicts on the OTHER inspectable edges that
        // point AT this intent — SERVES (persona→intent), TARGETS
        // (hypothesis→intent), GOVERNS (rule→intent) — so a persona/hypothesis/
        // rule does not keep a green claim about now-dead code. Gather before the
        // tx, mirroring the RELATES_TO path; only a passing verdict reopens.
        let serves: Vec<ServesEdge> = self
            .list_all_serves()?
            .into_iter()
            .filter(|e| e.intent_id == id && e.inspection_status == "passing")
            .collect();
        let targets: Vec<TargetsEdge> = self
            .list_all_targets()?
            .into_iter()
            .filter(|e| e.intent_id == id && e.inspection_status == "passing")
            .collect();
        let governs: Vec<Governs> = self
            .list_all_governs()?
            .into_iter()
            .filter(|e| e.intent_id == id && e.inspection_status == "passing")
            .collect();
        // IMPLEMENTS: retire leaves the grounding rows in place (history is
        // preserved, not hard-dropped), but a passing/independent grounding on
        // now-dead code must not stay green — stale it like every other edge
        // type, so un-retiring forces a re-inspection. The active snapshot also
        // filters retired-intent IMPLEMENTS (query_snapshot), so this staling
        // is belt-and-suspenders: out of the active view AND honestly marked.
        let implements_edges: Vec<Implements> = self
            .list_all_implements()?
            .into_iter()
            .filter(|e| {
                e.intent_id == id
                    && (e.inspection_status == "passing" || e.inspection_status == "independent")
            })
            .collect();
        // VALIDATES was the one inspectable edge left un-staled on retirement, so a
        // raw (unfiltered) read of `validates`/`validation` could still show a green
        // proof for now-dead code while the other five edge types are honestly
        // marked. Gather the passing/independent proofs to flip below.
        let validates_edges = self
            .list_all_validates()?
            .into_iter()
            .filter(|e| {
                e.intent_id == id
                    && (e.inspection_status == "passing" || e.inspection_status == "independent")
            })
            .collect::<Vec<_>>();
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE intent SET status = 'deprecated', updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        insert_transition_note_tx(&tx, "intent", id, &prev.status, "deprecated", "loom", now)?;
        let cause = format!("intent '{}' retired", prev.name);
        for edge in relates {
            if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
                super::stale_relates_to(&tx, &edge.from_id, &edge.to_id)?;
                insert_sync_flip_note_tx(
                    &tx,
                    "edge",
                    &edge.id,
                    &edge.inspection_status,
                    "needs_reverification",
                    &cause,
                    now,
                )?;
            }
        }
        for edge in &serves {
            super::stale_serves(&tx, &edge.persona_id, &edge.intent_id)?;
            insert_sync_flip_note_tx(
                &tx,
                "edge",
                &edge.id,
                &edge.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
        }
        for edge in &targets {
            super::stale_targets(&tx, &edge.hypothesis_id, &edge.intent_id)?;
            insert_sync_flip_note_tx(
                &tx,
                "edge",
                &edge.id,
                &edge.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
        }
        for edge in &governs {
            super::stale_governs(&tx, &edge.rule_id, &edge.intent_id)?;
            insert_sync_flip_note_tx(
                &tx,
                "edge",
                &edge.id,
                &edge.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
        }
        for edge in &implements_edges {
            tx.execute(
                SQL_FLAG_IMPLEMENTS_STALE,
                params![edge.intent_id, edge.codefile_id],
            )?;
            insert_sync_flip_note_tx(
                &tx,
                "edge",
                &edge.id,
                &edge.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
        }
        for edge in &validates_edges {
            // Flip THIS intent's proof link: a passing validates edge on now-dead
            // code must not stay green (the snapshot.validates honesty gap).
            tx.execute(
                "UPDATE validates SET inspection_status = 'needs_reverification'
                 WHERE validation_id = ?1 AND intent_id = ?2",
                params![edge.validation_id, edge.intent_id],
            )?;
            insert_sync_flip_note_tx(
                &tx,
                "edge",
                &edge.id,
                &edge.inspection_status,
                "needs_reverification",
                &cause,
                now,
            )?;
            // The validation's RESULT is shared (one validation can prove many
            // intents — a saga, an integration test). Reset last_result ONLY when no
            // OTHER still-active intent relies on it; otherwise resetting would
            // wrongly invalidate a live intent's green proof. The retiring intent is
            // already 'deprecated' in this tx, so it is excluded by both clauses.
            let shared_with_active: bool = tx.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM validates v JOIN intent i ON v.intent_id = i.id
                    WHERE v.validation_id = ?1 AND v.intent_id != ?2 AND i.status != 'deprecated'
                 )",
                params![edge.validation_id, id],
                |r| r.get(0),
            )?;
            if !shared_with_active {
                tx.execute(
                    "UPDATE validation
                     SET last_result = 'not_run',
                         last_run = '',
                         last_executed_run = '',
                         discrimination_status = ''
                     WHERE id = ?1 AND last_result NOT IN ('not_run', 'blocked', '')",
                    params![edge.validation_id],
                )?;
            }
        }
        let text = match replaced_by {
            Some(successor) => format!("retired: {reason} - replaced by intent {successor}"),
            None => format!("retired: {reason}"),
        };
        tx.execute(
            "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
             VALUES(?1, 'decision', ?2, 'loom', 'intent', ?3, ?4, '')",
            params![uuid::Uuid::new_v4().to_string(), text, id, now],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn delete_intent(&mut self, id: &str) -> Result<bool> {
        let exists = self.get_intent(id)?.is_some();
        if !exists {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        tx.execute("DELETE FROM note WHERE target_id = ?1", params![id])?;
        tx.execute(SQL_DELETE_EDGE_NOTES, params![id])?;
        tx.execute("DELETE FROM intent WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn insert_ignore(&self, ignore: &Ignore) -> Result<()> {
        self.write_one(
            "INSERT INTO ignore_rule(id, pattern, reason, author, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                ignore.id,
                ignore.pattern,
                ignore.reason,
                ignore.author,
                ignore.created_at
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    /// Remove a coverage exclusion rule by its exact pattern string.
    /// Returns `true` if a row was deleted, `false` if the pattern was not found.
    pub fn delete_ignore(&self, pattern: &str) -> Result<bool> {
        let n = self.write_one(
            "DELETE FROM ignore_rule WHERE pattern = ?1",
            params![pattern],
        )?;
        Ok(n > 0)
    }
}

impl SqliteGraphStore {
    pub fn insert_delegation(&self, delegation: &Delegation) -> Result<()> {
        self.write_one(
            "INSERT INTO delegation(id, pattern, target, author, created_at, export_hash, seam_intents)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                delegation.id,
                delegation.pattern,
                delegation.target,
                delegation.author,
                delegation.created_at,
                delegation.export_hash,
                serde_json::to_string(&delegation.seam_intents)?,
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    /// Add a parent seam intent to a delegation (idempotent, sorted, deduped).
    pub fn add_delegation_seam(&mut self, delegation_id: &str, intent_id: &str) -> Result<bool> {
        let mut delegation = self.resolve_delegation(delegation_id)?;
        if delegation.seam_intents.iter().any(|i| i == intent_id) {
            return Ok(false);
        }
        delegation.seam_intents.push(intent_id.to_string());
        delegation.seam_intents.sort();
        delegation.seam_intents.dedup();
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE delegation SET seam_intents = ?1 WHERE id = ?2",
            params![
                serde_json::to_string(&delegation.seam_intents)?,
                delegation.id
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    /// Persist the observed child-export content hash (the watched baseline).
    pub fn set_delegation_export_hash(&mut self, delegation_id: &str, hash: &str) -> Result<()> {
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE delegation SET export_hash = ?1 WHERE id = ?2",
            params![hash, delegation_id],
        )?;
        tx.commit()?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn delete_delegation(&self, pattern: &str) -> Result<Option<Delegation>> {
        let existing = self
            .list_delegations()?
            .into_iter()
            .find(|delegation| delegation.pattern == pattern);
        if existing.is_none() {
            return Ok(None);
        }
        self.write_one(
            "DELETE FROM delegation WHERE pattern = ?1",
            params![pattern],
        )?;
        Ok(existing)
    }
}

impl SqliteGraphStore {
    pub fn insert_codefile(&self, codefile: &CodeFile) -> Result<()> {
        self.write_one(
            "INSERT INTO codefile(id, path, language, last_modified, imports, symbols, symbol_facts, content_hash, extractor_grade)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                codefile.id,
                codefile.path,
                codefile.language,
                codefile.last_modified,
                serde_json::to_string(&codefile.imports)?,
                serde_json::to_string(&codefile.symbols)?,
                serde_json::to_string(&codefile.symbol_facts)?,
                codefile.content_hash,
                codefile.extractor_grade
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn update_codefile_extractor_grade(&self, id: &str, grade: &str) -> Result<()> {
        self.write_one(
            "UPDATE codefile SET extractor_grade = ?1 WHERE id = ?2",
            params![grade, id],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn delete_codefile(&mut self, key: &str) -> Result<Option<CodeFile>> {
        let Some(codefile) = self
            .list_codefiles()?
            .into_iter()
            .find(|codefile| codefile.id == key || codefile.path == key)
        else {
            return Ok(None);
        };
        let tx = self.write_tx()?;
        tx.execute(SQL_DELETE_EDGE_NOTES, params![codefile.id])?;
        tx.execute("DELETE FROM codefile WHERE id = ?1", params![codefile.id])?;
        tx.commit()?;
        Ok(Some(codefile))
    }
}

impl SqliteGraphStore {
    pub fn update_codefile_hash(&self, id: &str, hash: &str) -> Result<()> {
        self.write_one(
            "UPDATE codefile SET content_hash = ?1 WHERE id = ?2",
            params![hash, id],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn update_codefile_hash_and_mtime(&self, id: &str, hash: &str, mtime: &str) -> Result<()> {
        self.write_one(
            "UPDATE codefile SET content_hash = ?1, last_modified = ?2 WHERE id = ?3",
            params![hash, mtime, id],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn update_codefile_imports(&self, id: &str, imports: &[String]) -> Result<()> {
        self.write_one(
            "UPDATE codefile SET imports = ?1 WHERE id = ?2",
            params![serde_json::to_string(imports)?, id],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn update_codefile_symbols(&self, id: &str, symbols: &[String]) -> Result<()> {
        self.write_one(
            "UPDATE codefile SET symbols = ?1 WHERE id = ?2",
            params![serde_json::to_string(symbols)?, id],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn update_codefile_symbol_facts(&self, id: &str, facts: &[SymbolFact]) -> Result<()> {
        self.write_one(
            "UPDATE codefile SET symbol_facts = ?1 WHERE id = ?2",
            params![serde_json::to_string(facts)?, id],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    /// Set the graph's autonomy mode (`autonomous` | `guided`). A behavioral
    /// tunable like `transition_cap`, not graph identity — so it lives in its own
    /// setter rather than threading through `initialize`/`set_identity`.
    pub fn set_autonomy(&self, autonomy: &str) -> Result<()> {
        self.write_one(
            "UPDATE meta SET autonomy = ?1 WHERE id = 1",
            params![autonomy],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn set_transition_cap(&self, cap: usize) -> Result<()> {
        self.write_one(
            "UPDATE meta SET transition_cap = ?1 WHERE id = 1",
            params![cap.to_string()],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn set_layer_order(&self, order: &[String]) -> Result<Vec<String>> {
        let previous = self.layer_order()?;
        let order_json = serde_json::to_string(order)?;
        self.write_one(
            "UPDATE meta SET layer_order = ?1 WHERE id = 1",
            params![order_json],
        )?;
        Ok(previous)
    }
}

impl SqliteGraphStore {
    pub fn insert_hypothesis(&self, hypothesis: &Hypothesis) -> Result<()> {
        self.write_one(
            "INSERT INTO hypothesis(
                id, name, claim, proposal, predicted_outcome, status, author,
                evidence, inspected_by, last_inspected, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                hypothesis.id,
                hypothesis.name,
                hypothesis.claim,
                hypothesis.proposal,
                hypothesis.predicted_outcome,
                hypothesis.status,
                hypothesis.author,
                hypothesis.evidence,
                hypothesis.inspected_by,
                hypothesis.last_inspected,
                hypothesis.created_at,
                hypothesis.updated_at
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn set_hypothesis_status(
        &mut self,
        hypothesis_id: &str,
        status: &str,
        author: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(previous) = self.get_hypothesis(hypothesis_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE hypothesis SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now, hypothesis_id],
        )?;
        insert_transition_note_tx(
            &tx,
            "hypothesis",
            hypothesis_id,
            &previous.status,
            status,
            author,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn update_hypothesis_verdict(
        &mut self,
        hypothesis_id: &str,
        verdict: &str,
        evidence: &str,
        inspected_by: &str,
        now: &str,
    ) -> Result<bool> {
        let Some(previous) = self.get_hypothesis(hypothesis_id)? else {
            return Ok(false);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE hypothesis
             SET status = ?1,
                 evidence = ?2,
                 inspected_by = ?3,
                 last_inspected = ?4,
                 updated_at = ?4
             WHERE id = ?5",
            params![verdict, evidence, inspected_by, now, hypothesis_id],
        )?;
        insert_transition_note_tx(
            &tx,
            "hypothesis",
            hypothesis_id,
            &previous.status,
            verdict,
            inspected_by,
            now,
        )?;
        tx.commit()?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn insert_persona(&self, persona: &Persona) -> Result<()> {
        self.write_one(
            "INSERT INTO persona(id, name, description, author, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                persona.id,
                persona.name,
                persona.description,
                persona.author,
                persona.created_at,
                persona.updated_at
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn insert_inbox_item(&self, item: &InboxItem) -> Result<()> {
        self.write_one(
            "INSERT INTO inbox_item(
                id, raw_text, normalized_claim, kind, status, source, author,
                tags, links, route_kind, route_command, route_target_kind,
                route_target_id, resolution, created_at, updated_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                item.id,
                item.raw_text,
                item.normalized_claim,
                item.kind,
                item.status,
                item.source,
                item.author,
                serde_json::to_string(&item.tags)?,
                serde_json::to_string(&item.links)?,
                item.route_kind,
                item.route_command,
                item.route_target_kind,
                item.route_target_id,
                item.resolution,
                item.created_at,
                item.updated_at
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn update_inbox_item(&self, item: &InboxItem) -> Result<()> {
        self.write_one(
            "UPDATE inbox_item
             SET raw_text = ?2,
                 normalized_claim = ?3,
                 kind = ?4,
                 status = ?5,
                 source = ?6,
                 author = ?7,
                 tags = ?8,
                 links = ?9,
                 route_kind = ?10,
                 route_command = ?11,
                 route_target_kind = ?12,
                 route_target_id = ?13,
                 resolution = ?14,
                 updated_at = ?15
             WHERE id = ?1",
            params![
                item.id,
                item.raw_text,
                item.normalized_claim,
                item.kind,
                item.status,
                item.source,
                item.author,
                serde_json::to_string(&item.tags)?,
                serde_json::to_string(&item.links)?,
                item.route_kind,
                item.route_command,
                item.route_target_kind,
                item.route_target_id,
                item.resolution,
                item.updated_at
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn get_or_create_interface_surface(
        &self,
        surface_kind: &str,
        method: &str,
        target: &str,
        description: &str,
        now: &str,
    ) -> Result<InterfaceSurface> {
        let name = interface_surface_name(surface_kind, method, target);
        self.write_one(
            "INSERT OR IGNORE INTO interface_surface(
                id, name, description, surface_kind, method, target, created_at, updated_at
             )
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                uuid::Uuid::new_v4().to_string(),
                name,
                description,
                surface_kind,
                method,
                target,
                now
            ],
        )?;
        self.conn
            .query_row(
                "SELECT id, name, description, surface_kind, method, target, created_at, updated_at
                 FROM interface_surface
                 WHERE surface_kind = ?1 AND method = ?2 AND target = ?3",
                params![surface_kind, method, target],
                |row| {
                    Ok(InterfaceSurface {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        surface_kind: row.get(3)?,
                        method: row.get(4)?,
                        target: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .map_err(Into::into)
    }
}

impl SqliteGraphStore {
    pub fn insert_vocab_term(&self, term: &VocabTerm) -> Result<()> {
        self.write_one(
            "INSERT INTO vocab_term(id, name, description, author, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                term.id,
                term.name,
                term.description,
                term.author,
                term.created_at
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn merge_vocab_terms(&mut self, from: &str, to: &str, now: &str) -> Result<usize> {
        let tx = self.write_tx()?;
        let from_exists: bool =
            tx.query_row(SQL_VOCAB_TERM_EXISTS, params![from], |row| row.get(0))?;
        if !from_exists {
            anyhow::bail!(
                "Term '{from}' is not registered — `loom vocab list` shows the registry."
            );
        }
        let to_exists: bool = tx.query_row(SQL_VOCAB_TERM_EXISTS, params![to], |row| row.get(0))?;
        if !to_exists {
            anyhow::bail!(
                "Target term '{to}' is not registered — merge dissolves '{from}' INTO an existing term; register '{to}' first if it should exist."
            );
        }

        let tagged_intents: Vec<(String, String)> = {
            let mut stmt = tx.prepare("SELECT id, tags FROM intent")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut retagged = 0usize;
        for (intent_id, tags_raw) in tagged_intents {
            let tags = string_list(&tags_raw)?;
            if !tags.iter().any(|tag| tag == from) {
                continue;
            }
            let new_tags = tags
                .into_iter()
                .map(|tag| if tag == from { to.to_string() } else { tag })
                .collect();
            let encoded = crate::db::queries::vocab::encode_tags(new_tags)?;
            tx.execute(
                SQL_UPDATE_INTENT_TAGS,
                params![serde_json::to_string(&encoded)?, now, intent_id],
            )?;
            retagged += 1;
        }
        tx.execute("DELETE FROM vocab_term WHERE name = ?1", params![from])?;
        tx.commit()?;
        Ok(retagged)
    }
}

impl SqliteGraphStore {
    pub fn insert_validation(&self, validation: &Validation) -> Result<()> {
        self.write_one(
            "INSERT INTO validation(id, name, description, validation_type, command, last_run, last_result, last_executed_run, discrimination_status)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                validation.id,
                validation.name,
                validation.description,
                validation.validation_type,
                validation.command,
                validation.last_run,
                validation.last_result,
                validation.last_executed_run,
                validation.discrimination_status
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    #[allow(clippy::too_many_arguments)]
    pub fn mark_validation_result(
        &mut self,
        key: &str,
        last_result: &str,
        edge_status: &str,
        edge_note: &str,
        marker: &str,
        now: &str,
        executed_run: Option<&str>,
        discrimination: Option<&str>,
    ) -> Result<(String, usize)> {
        let validation = self.resolve_validation(key)?;
        let tx = self.write_tx()?;
        // The executor passes Some(timestamp) to stamp last_executed_run (the
        // machine-run discriminator) AND Some(status) for discrimination (what
        // the runner was observed to do); a hand-mark passes None for both so it
        // NEVER overwrites a prior machine-run timestamp/witness — `loom
        // validation mark` on an already-executed proof keeps its executed
        // status, and a hand-mark on a never-run proof leaves last_executed_run
        // empty (asserted, not executed).
        if let Some(ts) = executed_run {
            tx.execute(
                "UPDATE validation SET last_result = ?1, last_run = ?2, last_executed_run = ?3, discrimination_status = ?4 WHERE id = ?5",
                params![last_result, now, ts, discrimination.unwrap_or(""), validation.id],
            )?;
        } else {
            tx.execute(
                "UPDATE validation SET last_result = ?1, last_run = ?2 WHERE id = ?3",
                params![last_result, now, validation.id],
            )?;
        }
        let intents_updated: i64 =
            tx.query_row(SQL_COUNT_VALIDATES, params![validation.id], |row| {
                row.get(0)
            })?;
        tx.execute(
            "UPDATE validates SET inspection_status = ?1, notes = ?2 WHERE validation_id = ?3",
            params![edge_status, edge_note, validation.id],
        )?;

        if last_result == "passed" {
            if let Some(hypothesis_id) = validation
                .description
                .lines()
                .find_map(|line| line.strip_prefix("hypothesis:"))
                .map(str::trim)
            {
                let previous: Option<String> = tx
                    .query_row(
                        "SELECT status FROM hypothesis WHERE id = ?1",
                        params![hypothesis_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if previous.as_deref() == Some("adopted") {
                    tx.execute(
                        "UPDATE hypothesis SET status = 'confirmed', updated_at = ?1 WHERE id = ?2",
                        params![now, hypothesis_id],
                    )?;
                    tx.execute(
                        "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
                         VALUES(?1, 'transition', 'adopted → confirmed', ?2, 'hypothesis', ?3, ?4, '')",
                        params![uuid::Uuid::new_v4().to_string(), marker, hypothesis_id, now],
                    )?;
                }
            }
        }

        tx.commit()?;
        Ok((validation.id, intents_updated as usize))
    }
}

impl SqliteGraphStore {
    pub fn update_validation_definition(
        &mut self,
        key: &str,
        command: Option<&str>,
        description: Option<&str>,
    ) -> Result<(String, bool, usize)> {
        let validation = self.resolve_validation(key)?;
        let command_changed = command.is_some_and(|new_command| new_command != validation.command);
        let tx = self.write_tx()?;
        if let Some(command) = command {
            tx.execute(
                "UPDATE validation SET command = ?1 WHERE id = ?2",
                params![command, validation.id],
            )?;
        }
        if let Some(description) = description {
            tx.execute(
                "UPDATE validation SET description = ?1 WHERE id = ?2",
                params![description, validation.id],
            )?;
        }
        let mut reset_edges = 0usize;
        if command_changed {
            tx.execute(SQL_RESET_VALIDATION, params![validation.id])?;
            let count: i64 = tx.query_row(SQL_COUNT_VALIDATES, params![validation.id], |row| {
                row.get(0)
            })?;
            tx.execute(
                "UPDATE validates
                 SET inspection_status = 'uninspected', notes = 'command updated — proof must be re-run'
                 WHERE validation_id = ?1",
                params![validation.id],
            )?;
            reset_edges = count as usize;
        }
        tx.commit()?;
        Ok((validation.id, command_changed, reset_edges))
    }
}

impl SqliteGraphStore {
    /// Delete an InterfaceSurface (CALLS edges cascade via FK). The escape hatch
    /// the `surface_without_calls` gap remedy points at — previously the remedy
    /// ("remove the stale interface surface") was unreachable through loom.
    pub fn delete_interface_surface(&mut self, id: &str) -> Result<bool> {
        let tx = self.write_tx()?;
        // Edge rows cascade via FK, but edge NOTES are not FK-linked — drop them
        // too (the id is embedded in the edge id, e.g. call:<validation>:<id>),
        // mirroring delete_validation, so no orphan notes survive in listings.
        tx.execute(SQL_DELETE_EDGE_NOTES, params![id])?;
        let changed = tx.execute("DELETE FROM interface_surface WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    /// Delete a Persona (SERVES + JOURNEYS edges cascade via FK).
    pub fn delete_persona(&mut self, id: &str) -> Result<bool> {
        let tx = self.write_tx()?;
        // Drop orphan edge notes (srv:<id>:… / jrn:<id>:…) the FK cascade leaves.
        tx.execute(SQL_DELETE_EDGE_NOTES, params![id])?;
        let changed = tx.execute("DELETE FROM persona WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(changed > 0)
    }
}

impl SqliteGraphStore {
    pub fn delete_validation(&mut self, key: &str) -> Result<String> {
        let validation = self.resolve_validation(key)?;
        let tx = self.write_tx()?;
        tx.execute(SQL_DELETE_EDGE_NOTES, params![validation.id])?;
        tx.execute(
            "DELETE FROM validation WHERE id = ?1",
            params![validation.id],
        )?;
        tx.commit()?;
        Ok(validation.id)
    }
}

impl SqliteGraphStore {
    pub fn insert_rule(&self, rule: &QualityRule) -> Result<()> {
        self.write_one(
            "INSERT OR REPLACE INTO quality_rule(id, name, description, detection_logic, severity, inspection_effort, kind,
                                       evidence_examples, signal_expectations, applies_when)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                rule.id,
                rule.name,
                rule.description,
                rule.detection_logic,
                rule.severity,
                rule.inspection_effort,
                rule.kind,
                rule.evidence_examples,
                rule.signal_expectations,
                rule.applies_when,
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn insert_note(&self, note: &Note) -> Result<()> {
        self.write_one(
            "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience, resolution)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                note.id,
                note.kind,
                note.text,
                note.author,
                note.target_kind,
                note.target_id,
                note.created_at,
                note.audience,
                note.resolution
            ],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    /// Close an open `todo` note with a reason (the resolution lifecycle). Returns
    /// the note's kind+text for the caller's confirmation, or None if no such id.
    /// loom can't auto-clear a free-form todo, so closing is an explicit, conscious
    /// act — but until it happens the open todo keeps surfacing in `loom next`.
    pub fn resolve_note(
        &self,
        note_id: &str,
        resolution: &str,
    ) -> Result<Option<(String, String)>> {
        let found: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT kind, text FROM note WHERE id = ?1",
                params![note_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((kind, text)) = found else {
            return Ok(None);
        };
        self.write_one(
            "UPDATE note SET resolution = ?2 WHERE id = ?1",
            params![note_id, resolution],
        )?;
        Ok(Some((kind, text)))
    }
}

impl SqliteGraphStore {
    pub fn delete_note_by_id(&self, note_id: &str) -> Result<()> {
        self.write_one("DELETE FROM note WHERE id = ?1", params![note_id])?;
        Ok(())
    }
}

impl SqliteGraphStore {
    pub fn invalidate_validation(&self, validation_id: &str) -> Result<bool> {
        let last_result: Option<String> = self
            .conn
            .query_row(
                SQL_SELECT_VALIDATION_LAST_RESULT,
                params![validation_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(last_result) = last_result else {
            return Ok(false);
        };
        if last_result == "not_run" || last_result == "blocked" || last_result.is_empty() {
            return Ok(false);
        }
        self.write_one(SQL_RESET_VALIDATION, params![validation_id])?;
        Ok(true)
    }
}

impl SqliteGraphStore {
    pub fn set_last_synced(&self, now: &str) -> Result<()> {
        self.write_one(
            "UPDATE meta SET last_synced = ?1 WHERE id = 1",
            params![now],
        )?;
        Ok(())
    }
}

impl SqliteGraphStore {
    /// SECURITY: after an import, neutralize unvetted command-carrying proofs so a
    /// bulk `loom validate --all` cannot SILENTLY execute shell commands that
    /// arrived in a (possibly untrusted) imported graph — the supply-chain RCE
    /// footgun. Only `not_run` commands are touched: a settled passed/failed
    /// result is data `--all` never re-runs, so a legitimate self-restore is
    /// unaffected. The operator vets one deliberately via `loom validate <intent>`
    /// (which runs a blocked proof). Returns how many were neutralized.
    pub fn block_unvetted_imported_commands(&self) -> Result<usize> {
        let n = self.write_one(
            "UPDATE validation SET last_result = 'blocked' \
             WHERE TRIM(command) <> '' AND last_result = 'not_run'",
            [],
        )?;
        Ok(n)
    }
}

/// Read an intent's `source_refs` list inside an open write transaction, so the
/// modify-then-write that follows is atomic with the read. `None` if the intent
/// does not exist.
fn read_source_refs_in_tx(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<Option<Vec<String>>> {
    let raw: String = match tx.query_row(
        "SELECT source_refs FROM intent WHERE id = ?1",
        params![id],
        |row| row.get(0),
    ) {
        Ok(raw) => raw,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(string_list_sql(&raw)?))
}
