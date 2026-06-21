use super::SqliteGraphStore;
use super::*;

impl SqliteGraphStore {
    pub fn ensure_owned(&self, action: &str) -> Result<()> {
        if let Some(meta) = self.graph_meta()? {
            if meta.observed() {
                anyhow::bail!(
                    "Custody gate: this graph OBSERVES '{}' — code its drivers don't own — so you \
                     cannot {action}. Record what you found instead: `loom edge explore … issue`, \
                     `loom rule verdict … --status failing`, or `loom note add --kind todo` \
                     (an upstream issue to hand to the owners).",
                    if meta.graph_name.is_empty() {
                        "this repo"
                    } else {
                        &meta.graph_name
                    },
                );
            }
        }
        Ok(())
    }
    pub fn count_all_intents(&self) -> Result<usize> {
        count_table(&self.conn, "intent")
    }
    /// Active + deprecated intent count as `i64` — an inherent alias so the
    /// GraphReadRepository delegation macro can forward uniformly by trait-method
    /// name (it would otherwise need a hand-written exception).
    pub fn count_intents_including_deprecated(&self) -> Result<i64> {
        Ok(self.count_all_intents()? as i64)
    }
    pub fn query_snapshot(&self) -> Result<QuerySnapshot> {
        // Snapshot.intents is active-only (list_active_intents excludes
        // deprecated). The IMPLEMENTS table has no such filter — retire_intent
        // stales RELATES_TO/SERVES/TARGETS/GOVERNS but leaves an intent's
        // IMPLEMENTS in place (grounding history preserved, not hard-dropped).
        // Without this filter, snapshot.implements would carry dangling edges
        // keyed by retired-intent UUIDs that are NOT in snapshot.intents, and
        // every snapshot consumer (smells' undeclared_coupling / tangled_file,
        // coverage's grounding, status' realized-leaves) would fire against
        // dead code. Drop them here so the active snapshot is self-consistent:
        // implements aligns with the active intents it joins against. The
        // unfiltered set stays available via list_all_implements (e.g.
        // retire_fallout, which filters by active itself).
        let intents = self.list_active_intents()?;
        let active_ids: std::collections::HashSet<String> =
            intents.iter().map(|i| i.id.clone()).collect();
        Ok(QuerySnapshot::from_parts(
            intents,
            self.list_hierarchy_pairs()?,
            self.list_relates_to()?,
            self.list_all_governs()?,
            self.list_rules()?,
            self.list_all_validates()?,
            self.list_validations()?,
            self.list_all_implements()?
                .into_iter()
                .filter(|im| active_ids.contains(&im.intent_id))
                .collect(),
            self.list_codefiles()?,
            // Notes are loaded lazily (notes_or_load) — a note-free consumer
            // (report/coverage/hotspots) must not pay the full Note-table scan.
            None,
        ))
    }
    pub fn graph_state(&self, snapshot: &QuerySnapshot) -> Result<GraphState> {
        let notes = snapshot.notes_or_load(|| self.list_all_notes())?;
        let vocab_terms = self.list_vocab_terms()?;
        let layer_order = self.layer_order()?;
        let proposed_hypotheses = self.list_hypotheses(Some("proposed"))?;
        let targets = self.list_all_targets()?;
        graph_state_from_snapshot_parts(
            snapshot,
            GraphStateContext {
                meta: self.graph_meta()?,
                notes: notes.len() as i64,
                transition_cap: self.transition_cap()?,
            },
            |snapshot| {
                compute_smells_from_parts(
                    snapshot,
                    SmellInputs {
                        notes,
                        vocab_terms: &vocab_terms,
                        layer_order: &layer_order,
                        proposed_hypotheses: &proposed_hypotheses,
                        targets: &targets,
                    },
                )
                .map(|report| report.open.len())
            },
            || self.count_hypotheses(Some("proposed")),
            // Map-vs-territory: files on disk the graph doesn't account for.
            // Lazy (only the audit-gate else branch calls it, i.e. near-green
            // graphs), so post-mutation `graph_state` pulses that land on an
            // earlier phase never pay for the walk + content-hash pass. The
            // root is the repo root that holds `.loom/` (resolve_root honors
            // LOOM_GRAPH / --graph), so this reconciles the graph's OWN
            // territory — delegated subtrees are excluded via delegation
            // patterns inside disk_reconciliation_from_parts.
            |snapshot| {
                let root = crate::db::resolve_root()?;
                let disk = crate::repo::walk_files(&root)?;
                let ignores = self.list_ignores()?;
                let delegations = self.list_delegations()?;
                Ok(
                    crate::db::queries::integrity::disk_reconciliation_from_parts(
                        &disk,
                        &snapshot.codefiles,
                        &ignores,
                        &delegations,
                        &|p| {
                            std::fs::read(root.join(p))
                                .ok()
                                .map(|b| crate::repo::content_hash(&b))
                        },
                    )
                    .issue_count(),
                )
            },
        )
    }
    pub fn smell_report(
        &self,
        snapshot: &QuerySnapshot,
    ) -> Result<crate::db::queries::SmellReport> {
        let notes = snapshot.notes_or_load(|| self.list_all_notes())?;
        let vocab_terms = self.list_vocab_terms()?;
        let layer_order = self.layer_order()?;
        let proposed_hypotheses = self.list_hypotheses(Some("proposed"))?;
        let targets = self.list_all_targets()?;
        compute_smells_from_parts(
            snapshot,
            SmellInputs {
                notes,
                vocab_terms: &vocab_terms,
                layer_order: &layer_order,
                proposed_hypotheses: &proposed_hypotheses,
                targets: &targets,
            },
        )
    }
    pub fn vocab_term_count(&self) -> Result<usize> {
        Ok(self.list_vocab_terms()?.len())
    }
    pub fn align_candidates(&self, snapshot: &QuerySnapshot) -> Result<Vec<AlignCandidate>> {
        let notes = snapshot.notes_or_load(|| self.list_all_notes())?;
        Ok(align_candidates_from_snapshot_notes(snapshot, notes))
    }
    pub fn doctor_report(&self, snapshot: &QuerySnapshot) -> Result<DoctorReport> {
        let notes = snapshot.notes_or_load(|| self.list_all_notes())?;
        let meta = self.graph_meta()?;
        let found_version = meta
            .as_ref()
            .map(|meta| meta.version.clone())
            .unwrap_or_default();
        check_graph_from_parts(
            snapshot,
            DoctorInputs {
                found_version,
                meta,
                node_counts: self.doctor_node_counts(snapshot)?,
                edge_counts: self.doctor_edge_counts(snapshot)?,
                missing_node_props: self.missing_node_props()?,
                missing_edge_props: self.missing_edge_props()?,
                intents: self.list_all_intents()?,
                hypotheses: self.list_hypotheses(None)?,
                vocab_terms: self.list_vocab_terms()?,
                target_edges: self.list_all_targets()?,
                serves_edges: self.list_all_serves()?,
                edge_ids: self.collect_edge_ids()?,
                notes,
            },
        )
        // Map-vs-territory reconciliation (files on disk the graph doesn't
        // account for) is NOT folded in here: doctor's scope is graph-INTERNAL
        // structural integrity (schema version, tree shape, prop completeness),
        // and the disk walk needs the repo root the command layer holds. The
        // compass phase gate — `graph_state` below — IS the read path that
        // gates green on map=territory, routing to `loom coverage` for the
        // per-file detail. Folding disk gaps into doctor ISSUES would re-create
        // the overstatement in the other direction (doctor "unhealthy" on a
        // synthetic tree-less checkout) without adding a gate the compass
        // doesn't already hold.
    }
    pub fn align_candidate_count(&self, snapshot: &QuerySnapshot) -> Result<i64> {
        let notes = snapshot.notes_or_load(|| self.list_all_notes())?;
        Ok(align_candidates_from_snapshot_notes(snapshot, notes).len() as i64)
    }
    pub fn prove_candidates(&self, snapshot: &QuerySnapshot) -> Result<Vec<(Hypothesis, f64)>> {
        Ok(prove_candidates_from_parts(
            self.list_hypotheses(None)?,
            self.list_all_targets()?,
            &snapshot.degrees,
        ))
    }
    pub fn list_intents(
        &self,
        status_filter: Option<&str>,
        level_filter: Option<&str>,
    ) -> Result<Vec<Intent>> {
        let mut intents = self.list_all_intents()?;
        if let Some(status_filter) = status_filter {
            intents.retain(|intent| intent.status == status_filter);
        }
        if let Some(level_filter) = level_filter {
            intents.retain(|intent| intent.abstraction_level == level_filter);
        }
        Ok(intents)
    }
    pub fn get_intent(&self, id: &str) -> Result<Option<Intent>> {
        self.conn
            .query_row(
                "SELECT id, name, description, abstraction_level, domain, layer, source_refs,
                        status, aspect, tags, visibility, boundary, lifecycle, created_at,
                        updated_at, criterion
                 FROM intent
                 WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Intent {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        abstraction_level: row.get(3)?,
                        domain: row.get(4)?,
                        layer: row.get(5)?,
                        source_refs: string_list_sql(row.get::<_, String>(6)?.as_str())?,
                        status: row.get(7)?,
                        aspect: row.get(8)?,
                        tags: string_list_sql(row.get::<_, String>(9)?.as_str())?,
                        visibility: row.get(10)?,
                        boundary: row.get(11)?,
                        lifecycle: row.get(12)?,
                        created_at: row.get(13)?,
                        updated_at: row.get(14)?,
                        criterion: row.get(15)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn retire_fallout(&self, id: &str) -> Result<RetireFallout> {
        let name_of: std::collections::HashMap<String, String> = self
            .list_all_intents()?
            .into_iter()
            .map(|intent| (intent.id, intent.name))
            .collect();
        let active: std::collections::HashSet<String> = self
            .list_active_intents()?
            .into_iter()
            .filter(|intent| intent.id != id)
            .map(|intent| intent.id)
            .collect();

        let mut orphaned_children: Vec<String> = self
            .list_hierarchy_pairs()?
            .into_iter()
            .filter(|(parent, child)| parent == id && active.contains(child))
            .map(|(_, child)| name_of.get(&child).cloned().unwrap_or(child))
            .collect();
        orphaned_children.sort();

        let mut owners: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for edge in self.list_all_implements()? {
            owners
                .entry(edge.codefile_path)
                .or_default()
                .push(edge.intent_id);
        }
        let mut solely_grounded_files: Vec<String> = owners
            .into_iter()
            .filter(|(_, owners)| {
                owners.iter().any(|owner| owner == id)
                    && !owners.iter().any(|owner| active.contains(owner))
            })
            .map(|(path, _)| path)
            .collect();
        solely_grounded_files.sort();

        let mut linked: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut validation_name: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for edge in self.list_all_validates()? {
            linked
                .entry(edge.validation_id.clone())
                .or_default()
                .push(edge.intent_id);
            validation_name.insert(edge.validation_id, edge.validation_name);
        }
        let mut dangling_validations: Vec<String> = linked
            .into_iter()
            .filter(|(_, intents)| {
                intents.iter().any(|intent| intent == id)
                    && !intents.iter().any(|intent| active.contains(intent))
            })
            .map(|(validation_id, _)| {
                validation_name
                    .get(&validation_id)
                    .cloned()
                    .unwrap_or(validation_id)
            })
            .collect();
        dangling_validations.sort();

        let edges_leaving_computation = self
            .list_relates_to()?
            .into_iter()
            .filter(|edge| edge.from_id == id || edge.to_id == id)
            .count();

        Ok(RetireFallout {
            orphaned_children,
            solely_grounded_files,
            dangling_validations,
            edges_leaving_computation,
        })
    }
    pub fn list_implements_for_intent(&self, intent_id: &str) -> Result<Vec<Implements>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.intent_id, e.codefile_id, i.name, cf.path, e.inspection_status,
                    e.criterion, e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                    e.locator, e.notes, e.created_at
             FROM implements e
             JOIN intent i ON i.id = e.intent_id
             JOIN codefile cf ON cf.id = e.codefile_id
             WHERE e.intent_id = ?1",
        )?;
        let rows = stmt.query_map(params![intent_id], |row| {
            let intent_id: String = row.get(0)?;
            let codefile_id: String = row.get(1)?;
            Ok(Implements {
                id: crate::db::schema::edge_key(edge::IMPLEMENTS, &intent_id, &codefile_id),
                intent_id,
                codefile_id,
                intent_name: row.get(2)?,
                codefile_path: row.get(3)?,
                inspection_status: row.get(4)?,
                criterion: row.get(5)?,
                confidence: row.get(6)?,
                evidence: row.get(7)?,
                last_inspected: row.get(8)?,
                inspected_by: row.get(9)?,
                locator: row.get(10)?,
                notes: row.get(11)?,
                created_at: row.get(12)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn validations_for_intent(&self, intent_id: &str) -> Result<Vec<Validation>> {
        let mut stmt = self.conn.prepare(
            "SELECT v.id, v.name, v.description, v.validation_type, v.command,
                    v.last_run, v.last_result, v.last_executed_run, v.discrimination_status
             FROM validates e
             JOIN validation v ON v.id = e.validation_id
             WHERE e.intent_id = ?1",
        )?;
        let rows = stmt.query_map(params![intent_id], |row| {
            Ok(Validation {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                validation_type: row.get(3)?,
                command: row.get(4)?,
                last_run: row.get(5)?,
                last_result: row.get(6)?,
                last_executed_run: row.get(7)?,
                discrimination_status: row.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn notes_for_target(&self, target_id: &str) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, text, author, target_kind, target_id, audience, created_at
             FROM note
             WHERE target_id = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![target_id], |row| {
            Ok(Note {
                id: row.get(0)?,
                kind: row.get(1)?,
                text: row.get(2)?,
                author: row.get(3)?,
                target_kind: row.get(4)?,
                target_id: row.get(5)?,
                audience: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn notes_by_kind(&self, kind: &str) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, text, author, target_kind, target_id, audience, created_at
             FROM note
             WHERE kind = ?1
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![kind], |row| {
            Ok(Note {
                id: row.get(0)?,
                kind: row.get(1)?,
                text: row.get(2)?,
                author: row.get(3)?,
                target_kind: row.get(4)?,
                target_id: row.get(5)?,
                audience: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_ignores(&self) -> Result<Vec<Ignore>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pattern, reason, author, created_at
             FROM ignore_rule
             ORDER BY pattern",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Ignore {
                id: row.get(0)?,
                pattern: row.get(1)?,
                reason: row.get(2)?,
                author: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_delegations(&self) -> Result<Vec<Delegation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pattern, target, author, created_at, export_hash, seam_intents
             FROM delegation
             ORDER BY pattern",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Delegation {
                id: row.get(0)?,
                pattern: row.get(1)?,
                target: row.get(2)?,
                author: row.get(3)?,
                created_at: row.get(4)?,
                export_hash: row.get(5)?,
                seam_intents: string_list_sql(row.get::<_, String>(6)?.as_str())?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    /// Resolve a delegation by id or pattern.
    pub fn resolve_delegation(&self, key: &str) -> Result<Delegation> {
        let delegations = self.list_delegations()?;
        delegations
            .iter()
            .find(|d| d.id == key || d.pattern == key)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No delegation matches '{key}' (by id or pattern). `loom delegate list`."
                )
            })
    }
    pub fn list_hierarchy_for_intent(&self, intent_id: &str) -> Result<Vec<Hierarchy>> {
        let mut edges = Vec::new();
        for (sql, param) in [
            (
                "SELECT e.parent_id, e.child_id, p.name, c.name, e.notes
                 FROM hierarchy e
                 JOIN intent p ON p.id = e.parent_id
                 JOIN intent c ON c.id = e.child_id
                 WHERE e.parent_id = ?1",
                intent_id,
            ),
            (
                "SELECT e.parent_id, e.child_id, p.name, c.name, e.notes
                 FROM hierarchy e
                 JOIN intent p ON p.id = e.parent_id
                 JOIN intent c ON c.id = e.child_id
                 WHERE e.child_id = ?1",
                intent_id,
            ),
        ] {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(params![param], |row| {
                let parent_id: String = row.get(0)?;
                let child_id: String = row.get(1)?;
                Ok(Hierarchy {
                    id: crate::db::schema::edge_key(edge::HIERARCHY, &parent_id, &child_id),
                    parent_id,
                    child_id,
                    parent_name: row.get(2)?,
                    child_name: row.get(3)?,
                    notes: row.get(4)?,
                })
            })?;
            for row in rows {
                edges.push(row?);
            }
        }
        let mut seen = std::collections::HashSet::new();
        edges.retain(|edge: &Hierarchy| seen.insert(edge.id.clone()));
        Ok(edges)
    }
    pub fn edges_for_intent(&self, intent_id: &str) -> Result<Vec<RelatesTo>> {
        let mut edges = Vec::new();
        for (sql, param) in [
            (
                "SELECT e.from_id, e.to_id, src.name, dst.name, e.inspection_status,
                        e.criterion, e.confidence, e.evidence, e.last_inspected,
                        e.inspected_by, e.priority_score, e.notes, e.kinds, e.stable
                 FROM relates_to e
                 JOIN intent src ON src.id = e.from_id
                 JOIN intent dst ON dst.id = e.to_id
                 WHERE e.from_id = ?1",
                intent_id,
            ),
            (
                "SELECT e.from_id, e.to_id, src.name, dst.name, e.inspection_status,
                        e.criterion, e.confidence, e.evidence, e.last_inspected,
                        e.inspected_by, e.priority_score, e.notes, e.kinds, e.stable
                 FROM relates_to e
                 JOIN intent src ON src.id = e.from_id
                 JOIN intent dst ON dst.id = e.to_id
                 WHERE e.to_id = ?1",
                intent_id,
            ),
        ] {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(params![param], |row| {
                let from_id: String = row.get(0)?;
                let to_id: String = row.get(1)?;
                Ok(RelatesTo {
                    id: crate::db::schema::edge_key(edge::RELATES_TO, &from_id, &to_id),
                    from_id,
                    to_id,
                    from_name: row.get(2)?,
                    to_name: row.get(3)?,
                    inspection_status: row.get(4)?,
                    criterion: row.get(5)?,
                    confidence: row.get(6)?,
                    evidence: row.get(7)?,
                    last_inspected: row.get(8)?,
                    inspected_by: row.get(9)?,
                    priority_score: row.get(10)?,
                    notes: row.get(11)?,
                    kinds: string_list_sql(row.get::<_, String>(12)?.as_str())?,
                    stable: row.get::<_, String>(13)? == "true",
                    discovery_class: String::new(),
                    discovery_signals: Vec::new(),
                    discovery_centrality: Default::default(),
                })
            })?;
            for row in rows {
                edges.push(row?);
            }
        }
        let mut seen = std::collections::HashSet::new();
        edges.retain(|edge: &RelatesTo| seen.insert(edge.id.clone()));
        Ok(edges)
    }
    pub fn list_codefiles(&self) -> Result<Vec<CodeFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, language, last_modified, imports, symbols, symbol_facts, content_hash, extractor_grade
             FROM codefile
             ORDER BY path",
        )?;
        let rows = stmt.query_map([], |row| {
            let symbol_facts_raw: String = row.get(6)?;
            Ok(CodeFile {
                id: row.get(0)?,
                path: row.get(1)?,
                language: row.get(2)?,
                last_modified: row.get(3)?,
                imports: string_list_sql(row.get::<_, String>(4)?.as_str())?,
                symbols: string_list_sql(row.get::<_, String>(5)?.as_str())?,
                symbol_facts: symbol_facts(&symbol_facts_raw)
                    .map_err(|err| rusqlite::Error::ToSqlConversionFailure(err.into()))?,
                content_hash: row.get(7)?,
                extractor_grade: row.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn graph_meta(&self) -> Result<Option<GraphMeta>> {
        self.conn
            .query_row(
                "SELECT schema_version, created_at, last_synced, graph_id, graph_name, custody
                 FROM meta
                 WHERE id = 1",
                [],
                |row| {
                    Ok(GraphMeta {
                        version: row.get(0)?,
                        created_at: row.get(1)?,
                        last_synced: row.get(2)?,
                        graph_id: row.get(3)?,
                        graph_name: row.get(4)?,
                        custody: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn transition_cap(&self) -> Result<usize> {
        let raw = self
            .conn
            .query_row("SELECT transition_cap FROM meta WHERE id = 1", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .unwrap_or_default();
        if raw.is_empty() {
            Ok(DEFAULT_TRANSITION_CAP)
        } else {
            Ok(raw.parse::<usize>().unwrap_or(DEFAULT_TRANSITION_CAP))
        }
    }
    pub fn layer_order(&self) -> Result<Vec<String>> {
        let raw = self
            .conn
            .query_row("SELECT layer_order FROM meta WHERE id = 1", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .unwrap_or_else(|| "[]".to_string());
        string_list(&raw)
    }
    pub fn get_relates_to_between(&self, from_id: &str, to_id: &str) -> Result<Option<RelatesTo>> {
        super::get_relates_to_between_conn(&self.conn, from_id, to_id)
    }
    pub fn list_hypotheses(&self, status: Option<&str>) -> Result<Vec<Hypothesis>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, claim, proposal, predicted_outcome, status, author, evidence,
                    inspected_by, last_inspected, created_at, updated_at
             FROM hypothesis
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Hypothesis {
                id: row.get(0)?,
                name: row.get(1)?,
                claim: row.get(2)?,
                proposal: row.get(3)?,
                predicted_outcome: row.get(4)?,
                status: row.get(5)?,
                author: row.get(6)?,
                evidence: row.get(7)?,
                inspected_by: row.get(8)?,
                last_inspected: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;
        let mut hypotheses = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(anyhow::Error::from)?;
        if let Some(status) = status {
            hypotheses.retain(|hypothesis| hypothesis.status == status);
        }
        Ok(hypotheses)
    }
    pub fn get_hypothesis(&self, id: &str) -> Result<Option<Hypothesis>> {
        Ok(self
            .list_hypotheses(None)?
            .into_iter()
            .find(|hypothesis| hypothesis.id == id))
    }
    pub fn resolve_hypothesis(&self, key: &str) -> Result<String> {
        crate::db::queries::resolve_hypothesis_from_list(&self.list_hypotheses(None)?, key)
    }
    pub fn list_personas(&self) -> Result<Vec<Persona>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, author, created_at, updated_at
             FROM persona
             ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Persona {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                author: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_interface_surfaces(&self) -> Result<Vec<InterfaceSurface>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, surface_kind, method, target, created_at, updated_at
             FROM interface_surface
             ORDER BY surface_kind, method, target",
        )?;
        let rows = stmt.query_map([], |row| {
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
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_inbox_items(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<InboxItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, raw_text, normalized_claim, kind, status, source, author,
                    tags, links, route_kind, route_command, route_target_kind,
                    route_target_id, resolution, created_at, updated_at
             FROM inbox_item
             WHERE (?1 IS NULL OR status = ?1)
               AND (?2 IS NULL OR kind = ?2)
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![status, kind], inbox_item_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn resolve_inbox_item(&self, key: &str) -> Result<InboxItem> {
        let mut exact = self.conn.prepare(
            "SELECT id, raw_text, normalized_claim, kind, status, source, author,
                    tags, links, route_kind, route_command, route_target_kind,
                    route_target_id, resolution, created_at, updated_at
             FROM inbox_item
             WHERE id = ?1",
        )?;
        if let Some(item) = exact
            .query_row(params![key], inbox_item_from_row)
            .optional()?
        {
            return Ok(item);
        }

        let mut prefix = self.conn.prepare(
            "SELECT id, raw_text, normalized_claim, kind, status, source, author,
                    tags, links, route_kind, route_command, route_target_kind,
                    route_target_id, resolution, created_at, updated_at
             FROM inbox_item
             WHERE substr(id, 1, length(?1)) = ?1
             ORDER BY created_at",
        )?;
        let matches = prefix
            .query_map(params![key], inbox_item_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match matches.len() {
            1 => Ok(matches.into_iter().next().expect("one match")),
            0 => anyhow::bail!("No inbox item matches '{}'. Run `loom inbox list`.", key),
            _ => anyhow::bail!(
                "'{}' is ambiguous — matches {} inbox items. Use the full id (`loom inbox list`).",
                key,
                matches.len()
            ),
        }
    }
    pub fn resolve_interface_surface(&self, key: &str) -> Result<InterfaceSurface> {
        let surfaces = self.list_interface_surfaces()?;
        if let Some(surface) = surfaces.iter().find(|surface| surface.id == key) {
            return Ok(surface.clone());
        }
        let kl = key.to_lowercase();
        let exact: Vec<_> = surfaces
            .iter()
            .filter(|surface| surface.name.to_lowercase() == kl)
            .collect();
        if exact.len() == 1 {
            return Ok(exact[0].clone());
        }
        let subs: Vec<_> = surfaces
            .iter()
            .filter(|surface| {
                surface.name.to_lowercase().contains(&kl)
                    || surface.target.to_lowercase().contains(&kl)
            })
            .collect();
        match subs.len() {
            1 => Ok(subs[0].clone()),
            0 => anyhow::bail!(
                "No interface surface matches '{}' (by id, name, or target fragment). Run `loom interface list`.",
                key
            ),
            _ => anyhow::bail!(
                "'{}' is ambiguous — matches {} interface surfaces. Use the id (`loom interface list`).",
                key,
                subs.len()
            ),
        }
    }
    pub fn list_calls_for_interface(&self, interface_id: &str) -> Result<Vec<CallsEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.validation_id, c.interface_id, v.name, s.name, c.step_index,
                    c.step_name, c.intent_id, i.name, c.notes, c.created_at
             FROM calls c
             JOIN validation v ON v.id = c.validation_id
             JOIN interface_surface s ON s.id = c.interface_id
             JOIN intent i ON i.id = c.intent_id
             WHERE c.interface_id = ?1
             ORDER BY v.name, CAST(c.step_index AS INTEGER)",
        )?;
        let rows = stmt.query_map(params![interface_id], calls_edge_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_all_calls(&self) -> Result<Vec<CallsEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.validation_id, c.interface_id, v.name, s.name, c.step_index,
                    c.step_name, c.intent_id, i.name, c.notes, c.created_at
             FROM calls c
             JOIN validation v ON v.id = c.validation_id
             JOIN interface_surface s ON s.id = c.interface_id
             JOIN intent i ON i.id = c.intent_id
             ORDER BY v.name, CAST(c.step_index AS INTEGER)",
        )?;
        let rows = stmt.query_map([], calls_edge_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn get_serves_between(
        &self,
        persona_id: &str,
        intent_id: &str,
    ) -> Result<Option<ServesEdge>> {
        self.conn
            .query_row(
                "SELECT e.persona_id, e.intent_id, p.name, i.name, e.inspection_status,
                        e.criterion, e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                        e.notes, e.created_at
                 FROM serves e
                 JOIN persona p ON p.id = e.persona_id
                 JOIN intent i ON i.id = e.intent_id
                 WHERE e.persona_id = ?1 AND e.intent_id = ?2",
                params![persona_id, intent_id],
                |row| {
                    let persona_id: String = row.get(0)?;
                    let intent_id: String = row.get(1)?;
                    Ok(ServesEdge {
                        id: crate::db::schema::edge_key(edge::SERVES, &persona_id, &intent_id),
                        persona_id,
                        intent_id,
                        persona_name: row.get(2)?,
                        intent_name: row.get(3)?,
                        inspection_status: row.get(4)?,
                        criterion: row.get(5)?,
                        confidence: row.get(6)?,
                        evidence: row.get(7)?,
                        last_inspected: row.get(8)?,
                        inspected_by: row.get(9)?,
                        priority_score: 0.0,
                        notes: row.get(10)?,
                        created_at: row.get(11)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn list_serves_for_persona(&self, persona_id: &str) -> Result<Vec<ServesEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.persona_id, e.intent_id, p.name, i.name, e.inspection_status,
                    e.criterion, e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                    e.notes, e.created_at
             FROM serves e
             JOIN persona p ON p.id = e.persona_id
             JOIN intent i ON i.id = e.intent_id
             WHERE e.persona_id = ?1
             ORDER BY e.rowid",
        )?;
        let rows = stmt.query_map(params![persona_id], |row| {
            let persona_id: String = row.get(0)?;
            let intent_id: String = row.get(1)?;
            Ok(ServesEdge {
                id: crate::db::schema::edge_key(edge::SERVES, &persona_id, &intent_id),
                persona_id,
                intent_id,
                persona_name: row.get(2)?,
                intent_name: row.get(3)?,
                inspection_status: row.get(4)?,
                criterion: row.get(5)?,
                confidence: row.get(6)?,
                evidence: row.get(7)?,
                last_inspected: row.get(8)?,
                inspected_by: row.get(9)?,
                priority_score: 0.0,
                notes: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn get_journeys_between(
        &self,
        persona_id: &str,
        validation_id: &str,
    ) -> Result<Option<JourneysEdge>> {
        self.conn
            .query_row(
                "SELECT e.persona_id, e.validation_id, p.name, v.name, e.notes, e.created_at
                 FROM journeys e
                 JOIN persona p ON p.id = e.persona_id
                 JOIN validation v ON v.id = e.validation_id
                 WHERE e.persona_id = ?1 AND e.validation_id = ?2",
                params![persona_id, validation_id],
                |row| {
                    let persona_id: String = row.get(0)?;
                    let validation_id: String = row.get(1)?;
                    Ok(JourneysEdge {
                        id: crate::db::schema::edge_key(
                            edge::JOURNEYS,
                            &persona_id,
                            &validation_id,
                        ),
                        persona_id,
                        validation_id,
                        persona_name: row.get(2)?,
                        validation_name: row.get(3)?,
                        notes: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    pub fn list_journeys_for_persona(&self, persona_id: &str) -> Result<Vec<JourneysEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.persona_id, e.validation_id, p.name, v.name, e.notes, e.created_at
             FROM journeys e
             JOIN persona p ON p.id = e.persona_id
             JOIN validation v ON v.id = e.validation_id
             WHERE e.persona_id = ?1
             ORDER BY e.rowid",
        )?;
        let rows = stmt.query_map(params![persona_id], |row| {
            let persona_id: String = row.get(0)?;
            let validation_id: String = row.get(1)?;
            Ok(JourneysEdge {
                id: crate::db::schema::edge_key(edge::JOURNEYS, &persona_id, &validation_id),
                persona_id,
                validation_id,
                persona_name: row.get(2)?,
                validation_name: row.get(3)?,
                notes: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_targets_for_hypothesis(&self, hypothesis_id: &str) -> Result<Vec<TargetsEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.hypothesis_id, e.intent_id, h.name, i.name, e.inspection_status,
                    e.criterion, e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                    e.notes
             FROM targets e
             JOIN hypothesis h ON h.id = e.hypothesis_id
             JOIN intent i ON i.id = e.intent_id
             WHERE e.hypothesis_id = ?1",
        )?;
        let rows = stmt.query_map(params![hypothesis_id], |row| {
            let hypothesis_id: String = row.get(0)?;
            let intent_id: String = row.get(1)?;
            Ok(TargetsEdge {
                id: crate::db::schema::edge_key(edge::TARGETS, &hypothesis_id, &intent_id),
                hypothesis_id,
                intent_id,
                hypothesis_name: row.get(2)?,
                intent_name: row.get(3)?,
                inspection_status: row.get(4)?,
                criterion: row.get(5)?,
                confidence: row.get(6)?,
                evidence: row.get(7)?,
                last_inspected: row.get(8)?,
                inspected_by: row.get(9)?,
                notes: row.get(10)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn get_targets_between(
        &self,
        hypothesis_id: &str,
        intent_id: &str,
    ) -> Result<Option<TargetsEdge>> {
        Ok(self
            .list_targets_for_hypothesis(hypothesis_id)?
            .into_iter()
            .find(|edge| edge.intent_id == intent_id))
    }
    pub fn list_all_targets(&self) -> Result<Vec<TargetsEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.hypothesis_id, e.intent_id, h.name, i.name, e.inspection_status,
                    e.criterion, e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                    e.notes
             FROM targets e
             JOIN hypothesis h ON h.id = e.hypothesis_id
             JOIN intent i ON i.id = e.intent_id
             ORDER BY h.name, i.name",
        )?;
        let rows = stmt.query_map([], |row| {
            let hypothesis_id: String = row.get(0)?;
            let intent_id: String = row.get(1)?;
            Ok(TargetsEdge {
                id: crate::db::schema::edge_key(edge::TARGETS, &hypothesis_id, &intent_id),
                hypothesis_id,
                intent_id,
                hypothesis_name: row.get(2)?,
                intent_name: row.get(3)?,
                inspection_status: row.get(4)?,
                criterion: row.get(5)?,
                confidence: row.get(6)?,
                evidence: row.get(7)?,
                last_inspected: row.get(8)?,
                inspected_by: row.get(9)?,
                notes: row.get(10)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_all_serves(&self) -> Result<Vec<ServesEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.persona_id, e.intent_id, p.name, i.name, e.inspection_status,
                    e.criterion, e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                    e.notes, e.created_at
             FROM serves e
             JOIN persona p ON p.id = e.persona_id
             JOIN intent i ON i.id = e.intent_id
             ORDER BY p.name, i.name",
        )?;
        let rows = stmt.query_map([], |row| {
            let persona_id: String = row.get(0)?;
            let intent_id: String = row.get(1)?;
            Ok(ServesEdge {
                id: crate::db::schema::edge_key(edge::SERVES, &persona_id, &intent_id),
                persona_id,
                intent_id,
                persona_name: row.get(2)?,
                intent_name: row.get(3)?,
                inspection_status: row.get(4)?,
                criterion: row.get(5)?,
                confidence: row.get(6)?,
                evidence: row.get(7)?,
                last_inspected: row.get(8)?,
                inspected_by: row.get(9)?,
                priority_score: 0.0,
                notes: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_vocab_terms(&self) -> Result<Vec<VocabTerm>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, author, created_at FROM vocab_term ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(VocabTerm {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                author: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_validations(&self) -> Result<Vec<Validation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, validation_type, command, last_run, last_result, last_executed_run, discrimination_status
             FROM validation
             ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Validation {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                validation_type: row.get(3)?,
                command: row.get(4)?,
                last_run: row.get(5)?,
                last_result: row.get(6)?,
                last_executed_run: row.get(7)?,
                discrimination_status: row.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_rules(&self) -> Result<Vec<QualityRule>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, detection_logic, severity, inspection_effort, kind
             FROM quality_rule
             ORDER BY name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(QualityRule {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                detection_logic: row.get(3)?,
                severity: row.get(4)?,
                inspection_effort: row.get(5)?,
                kind: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn resolve_rule(&self, key: &str) -> Result<String> {
        let rules = self.list_rules()?;
        if rules.iter().any(|rule| rule.id == key) {
            return Ok(key.to_string());
        }
        let kl = key.to_lowercase();
        let exact: Vec<_> = rules
            .iter()
            .filter(|rule| rule.name.to_lowercase() == kl)
            .collect();
        if exact.len() == 1 {
            return Ok(exact[0].id.clone());
        }
        let subs: Vec<_> = rules
            .iter()
            .filter(|rule| rule.name.to_lowercase().contains(&kl))
            .collect();
        match subs.len() {
            1 => Ok(subs[0].id.clone()),
            0 => anyhow::bail!(
                "No quality rule matches '{}' (by id, exact name, or fragment). Run `loom rule list`.",
                key
            ),
            _ => anyhow::bail!(
                "'{}' is ambiguous — matches {} quality rules. Use the id (`loom rule list`).",
                key,
                subs.len()
            ),
        }
    }
    pub fn list_governs_for_intent(&self, intent_id: &str) -> Result<Vec<Governs>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.rule_id, e.intent_id, r.name, i.name, e.inspection_status, e.criterion,
                    e.confidence, e.evidence, e.last_inspected, e.inspected_by, e.notes
             FROM governs e
             JOIN quality_rule r ON r.id = e.rule_id
             JOIN intent i ON i.id = e.intent_id
             WHERE e.intent_id = ?1
             ORDER BY e.rowid",
        )?;
        let rows = stmt.query_map(params![intent_id], |row| {
            let rule_id: String = row.get(0)?;
            let intent_id: String = row.get(1)?;
            Ok(Governs {
                id: crate::db::schema::edge_key(edge::GOVERNS, &rule_id, &intent_id),
                rule_id,
                intent_id,
                rule_name: row.get(2)?,
                intent_name: row.get(3)?,
                inspection_status: row.get(4)?,
                criterion: row.get(5)?,
                confidence: row.get(6)?,
                evidence: row.get(7)?,
                last_inspected: row.get(8)?,
                inspected_by: row.get(9)?,
                notes: row.get(10)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn list_notes(&self, target_id: Option<&str>, kind: Option<&str>) -> Result<Vec<Note>> {
        // Push the filters into SQL so SQLite serves them from idx_note_target_only
        // / idx_note_kind instead of materializing every note body and discarding
        // most (the read path carries thousands of transition notes).
        let mut sql = String::from(
            "SELECT id, kind, text, author, target_kind, target_id, audience, created_at FROM note",
        );
        let mut clauses: Vec<&str> = Vec::new();
        if target_id.is_some() {
            clauses.push("target_id = ?");
        }
        if kind.is_some() {
            clauses.push("kind = ?");
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at");
        let bound: Vec<&str> = [target_id, kind].into_iter().flatten().collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bound), |row| {
            Ok(Note {
                id: row.get(0)?,
                kind: row.get(1)?,
                text: row.get(2)?,
                author: row.get(3)?,
                target_kind: row.get(4)?,
                target_id: row.get(5)?,
                audience: row.get(6)?,
                created_at: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    pub fn dangling_notes(&self) -> Result<Vec<Note>> {
        let intent_ids: std::collections::HashSet<String> =
            self.list_all_intents()?.into_iter().map(|i| i.id).collect();
        let hypothesis_ids: std::collections::HashSet<String> = self
            .list_hypotheses(None)?
            .into_iter()
            .map(|h| h.id)
            .collect();
        let edge_ids = self.collect_edge_ids()?;
        Ok(self
            .list_all_notes()?
            .into_iter()
            .filter(|note| match note.target_kind.as_str() {
                "intent" => !intent_ids.contains(&note.target_id),
                "hypothesis" => !hypothesis_ids.contains(&note.target_id),
                "edge" => !edge_ids.contains(&note.target_id),
                _ => false,
            })
            .collect())
    }
    pub fn prunable_transition_notes(&self, keep_per_target: usize) -> Result<Vec<Note>> {
        let transitions = self.list_notes(None, Some("transition"))?;
        let mut by_target: std::collections::HashMap<&str, Vec<&Note>> =
            std::collections::HashMap::new();
        for note in &transitions {
            by_target
                .entry(note.target_id.as_str())
                .or_default()
                .push(note);
        }
        let mut to_drop: Vec<Note> = Vec::new();
        for notes in by_target.values() {
            let mut kept_routine = 0usize;
            for note in notes.iter().rev() {
                if note.text.ends_with("→ failing") || note.text.ends_with("→ needs_change") {
                    continue;
                }
                if kept_routine < keep_per_target {
                    kept_routine += 1;
                    continue;
                }
                to_drop.push((*note).clone());
            }
        }
        Ok(to_drop)
    }
    pub fn edge_id_exists(&self, edge_id: &str) -> Result<bool> {
        for spec in EDGE_SPECS {
            for (from_id, to_id) in super::edge_pairs(&self.conn, spec)? {
                if crate::db::schema::edge_key(spec.edge_type, &from_id, &to_id) == edge_id {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}
