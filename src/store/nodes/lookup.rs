use super::super::*;

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

    /// Like `resolve_node`, but a total miss is `None` rather than an error.
    /// Ambiguity still fails: a missing subject is absence, several matches is
    /// a write that must not guess.
    pub fn resolve_node_optional(
        &self,
        key: &str,
        node_type: Option<NodeType>,
    ) -> Result<Option<Node>> {
        match self.resolve_node(key, node_type) {
            Ok(n) => Ok(Some(n)),
            Err(err) if err.to_string().starts_with("no node matches") => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub(crate) fn find_nodes_by(
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
}
