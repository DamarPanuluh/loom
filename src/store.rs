//! SQLite graph store — durable persistence behind a focused interface.
//!
//! Plane: this is the only module that touches SQL. Callers see typed nodes,
//! edges, facets, and verdicts; the schema, ids, timestamps, and write-time
//! integrity checks are hidden here.
//!
//! Integrity guarantees enforced at this boundary (the write boundary):
//! - INV-4: `independent` verdicts require non-empty evidence.
//! - INV-5: derived status is written ONLY by `set_derived_status`; asserted
//!   verdicts ONLY by `record_verdict`. Neither path crosses the truth-class line.
//! - INV-6: passing/failing/independent verdicts require non-empty criterion + evidence.
//! - Edge typing: every edge is validated against the edge-kind registry.

use crate::model::*;
use crate::registry;
use crate::{Result, GRAPH_DB, LOOM_DIR, SCHEMA_VERSION};
use anyhow::{anyhow, bail, Context};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Identity of a graph — what other graphs reference in a federation. Travels
/// in the export.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub graph_id: String,
    pub name: String,
    pub schema_version: u32,
    pub observed: bool,
}

/// A read-only projection of the whole graph, used by export. All collections
/// are sorted by stable keys so serialization is deterministic.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub identity: Identity,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub facets: Vec<Facet>,
    pub tags: Vec<Tag>,
}

/// The SQLite-backed graph store. Holds an exclusive advisory lock for its
/// lifetime so two processes never write the same graph concurrently.
pub struct Store {
    conn: Connection,
    root: PathBuf,
    agent: std::cell::Cell<Agent>,
    _lock: File,
}

/// The acting agent. Solo (default) may drive every lane; a declared lane is
/// enforced at the write boundary (a quality agent cannot write a builder edge).
/// Evidence/integrity gates apply regardless of agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Solo,
    Lane(registry::OwnerRole),
}

impl Agent {
    /// Parse `LOOM_AGENT`: unset or `llm` → Solo; `llm:builder` etc → that lane.
    pub fn from_env() -> Agent {
        match std::env::var("LOOM_AGENT") {
            Ok(v) => Agent::parse(&v),
            Err(_) => Agent::Solo,
        }
    }

    pub fn parse(v: &str) -> Agent {
        let lane = v.strip_prefix("llm:").unwrap_or(v);
        match lane {
            "builder" => Agent::Lane(registry::OwnerRole::Builder),
            "analyzer" => Agent::Lane(registry::OwnerRole::Analyzer),
            "fixer" => Agent::Lane(registry::OwnerRole::Fixer),
            "validator" => Agent::Lane(registry::OwnerRole::Validator),
            "quality" => Agent::Lane(registry::OwnerRole::Quality),
            _ => Agent::Solo,
        }
    }
}

const SCHEMA: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE node (
    id          TEXT PRIMARY KEY,
    node_type   TEXT NOT NULL,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT '',
    truth_class TEXT NOT NULL DEFAULT 'asserted' CHECK (truth_class IN ('derived','asserted')),
    body        TEXT NOT NULL DEFAULT '{}',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_node_type ON node(node_type);
CREATE INDEX idx_node_name ON node(name);

CREATE TABLE edge (
    id           TEXT PRIMARY KEY,
    from_id      TEXT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    to_id        TEXT NOT NULL REFERENCES node(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    truth_class  TEXT NOT NULL CHECK (truth_class IN ('derived','asserted')),
    status       TEXT NOT NULL DEFAULT 'uninspected',
    criterion    TEXT NOT NULL DEFAULT '',
    evidence     TEXT NOT NULL DEFAULT '',
    confidence   REAL NOT NULL DEFAULT 0,
    depends_on   TEXT NOT NULL DEFAULT '[]',
    inspected_by TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE (from_id, to_id, kind)
);
CREATE INDEX idx_edge_queue ON edge(truth_class, status);
CREATE INDEX idx_edge_kind  ON edge(kind, status);
CREATE INDEX idx_edge_from  ON edge(from_id, kind);
CREATE INDEX idx_edge_to    ON edge(to_id, kind);

CREATE TABLE facet (
    target_id   TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('node','edge')),
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    truth_class TEXT NOT NULL CHECK (truth_class IN ('derived','asserted')),
    PRIMARY KEY (target_id, target_kind, key)
);

CREATE TABLE tag (
    target_id   TEXT NOT NULL,
    target_kind TEXT NOT NULL CHECK (target_kind IN ('node','edge')),
    term        TEXT NOT NULL,
    PRIMARY KEY (target_id, target_kind, term)
);

CREATE TABLE tag_vocabulary (
    term        TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL
);
"#;

impl Store {
    /// Initialize a fresh graph at `root/.loom/graph.sqlite`. Idempotent: if the
    /// store already exists, opens it and backfills identity defaults.
    pub fn init(root: &Path, name: Option<&str>, observed: bool) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir)
            .with_context(|| format!("creating {}", loom_dir.display()))?;
        let db_path = loom_dir.join(GRAPH_DB);
        let fresh = !db_path.exists();
        let lock = acquire_lock(&loom_dir)?;
        let mut conn =
            Connection::open(&db_path).with_context(|| format!("opening {}", db_path.display()))?;
        configure(&conn)?;
        apply_schema_migrations(&mut conn)?;
        if fresh {
            let default_name = name
                .map(str::to_string)
                .or_else(|| {
                    root.canonicalize()
                        .ok()
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                })
                .unwrap_or_else(|| "loom".to_string());
            let (gid, now) = id_and_now(&conn)?;
            let set = |k: &str, v: &str| -> Result<()> {
                conn.execute("INSERT INTO meta(key,value) VALUES (?1,?2)", params![k, v])?;
                Ok(())
            };
            set("graph_id", &gid)?;
            set("name", &default_name)?;
            set("schema_version", &SCHEMA_VERSION.to_string())?;
            set("observed", if observed { "1" } else { "0" })?;
            set("created_at", &now)?;
        } else if name.is_some() || observed {
            // Backfill identity on an existing graph.
            if let Some(n) = name {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES ('name',?1)
                     ON CONFLICT(key) DO UPDATE SET value=?1",
                    params![n],
                )?;
            }
            if observed {
                conn.execute(
                    "INSERT INTO meta(key,value) VALUES ('observed','1')
                     ON CONFLICT(key) DO UPDATE SET value='1'",
                    [],
                )?;
            }
        }
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            agent: std::cell::Cell::new(Agent::from_env()),
            _lock: lock,
        })
    }

    /// Open an existing graph at `root/.loom/graph.sqlite`.
    pub fn open(root: &Path) -> Result<Store> {
        let loom_dir = root.join(LOOM_DIR);
        let db_path = loom_dir.join(GRAPH_DB);
        if !db_path.exists() {
            bail!(
                "no loom graph at {} — run `loom init` first",
                db_path.display()
            );
        }
        let lock = acquire_lock(&loom_dir)?;
        let mut conn = Connection::open(&db_path)?;
        configure(&conn)?;
        apply_schema_migrations(&mut conn)?;
        Ok(Store {
            conn,
            root: root.to_path_buf(),
            agent: std::cell::Cell::new(Agent::from_env()),
            _lock: lock,
        })
    }

    /// Walk up from `start` to find the nearest ancestor containing `.loom/`.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            if dir.join(LOOM_DIR).join(GRAPH_DB).exists() {
                return Some(dir.to_path_buf());
            }
            cur = dir.parent();
        }
        None
    }

    /// The acting agent.
    pub fn agent(&self) -> Agent {
        self.agent.get()
    }

    /// Override the acting agent (CLI sets this from `LOOM_AGENT`; tests set it
    /// explicitly to exercise lane gates without env races).
    pub fn set_agent(&self, agent: Agent) {
        self.agent.set(agent);
    }

    /// Lane gate: a declared lane may only write edges/verdicts it owns. Solo
    /// drives every lane. `sync` is implicit (derived paths never call this).
    fn check_lane(&self, owner: registry::OwnerRole) -> Result<()> {
        match self.agent.get() {
            Agent::Solo => Ok(()),
            Agent::Lane(role) if role == owner => Ok(()),
            Agent::Lane(role) => bail!(
                "lane gate: agent '{}' may not write '{}'-owned facts",
                role.as_str(),
                owner.as_str()
            ),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn identity(&self) -> Result<Identity> {
        let get = |k: &str| -> Result<String> {
            self.conn
                .query_row("SELECT value FROM meta WHERE key=?1", params![k], |r| {
                    r.get::<_, String>(0)
                })
                .with_context(|| format!("reading meta '{k}'"))
        };
        Ok(Identity {
            graph_id: get("graph_id")?,
            name: get("name")?,
            schema_version: get("schema_version")?.parse().unwrap_or(SCHEMA_VERSION),
            observed: get("observed").unwrap_or_else(|_| "0".into()) == "1",
        })
    }

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
        let (id, now) = id_and_now(&self.conn)?;
        self.conn.execute(
            "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,'asserted',?6,?7,?7)",
            params![
                id,
                node_type.as_str(),
                name,
                description,
                status,
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
            bail!(
                "ambiguous name '{key}': {} nodes match exactly",
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
                let names: Vec<_> = matches.iter().take(8).map(|m| m.name.clone()).collect();
                bail!(
                    "ambiguous fragment '{key}': {n} candidates: {}",
                    names.join(", ")
                )
            }
        }
    }

    fn find_nodes_by(
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
        let mut stmt;
        let rows = if let Some(t) = node_type {
            stmt = self.conn.prepare(
                "SELECT id,node_type,name,description,status,truth_class,body,created_at,updated_at
                 FROM node WHERE node_type=?1 ORDER BY name LIMIT ?2",
            )?;
            stmt.query_map(params![t.as_str(), limit as i64], row_to_node)?
        } else {
            stmt = self.conn.prepare(
                "SELECT id,node_type,name,description,status,truth_class,body,created_at,updated_at
                 FROM node ORDER BY name LIMIT ?1",
            )?;
            stmt.query_map(params![limit as i64], row_to_node)?
        };
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

    /// Redefine an intent's description — the semantic twin of `sync`. Ripples
    /// one hop: every settled asserted verdict touching the intent re-opens to
    /// needs_reverification, linked validations reset to not_run, and the old
    /// wording is preserved in a decision note. A name-only change does not call
    /// this (no ripple). Builder lane.
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
        // ripple one hop: implements/governs/validates/relationships touching it
        let mut reopened = 0usize;
        let touching_to = [EdgeKind::Implements, EdgeKind::Governs, EdgeKind::Validates];
        for k in touching_to {
            for e in self.edges_with(Some(k), None, Some(id))? {
                if self.stale_edge(&e.id)? {
                    reopened += 1;
                }
                if k == EdgeKind::Validates {
                    self.set_node_status(&e.from_id, "not_run").ok();
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
                if self.stale_edge(&e.id)? {
                    reopened += 1;
                }
            }
            for e in self.edges_with(Some(k), None, Some(id))? {
                if self.stale_edge(&e.id)? {
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
        let status = match truth_class {
            TruthClass::Derived => InspectionStatus::Current,
            TruthClass::Asserted => InspectionStatus::Uninspected,
        };
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
                if criterion.trim().is_empty() || evidence.trim().is_empty() {
                    bail!("{status} verdict requires non-empty criterion and evidence");
                }
            }
            InspectionStatus::Independent => {
                // INV-4: absence is the default; an independent row must bear evidence.
                if evidence.trim().is_empty() {
                    bail!("independent verdict requires non-empty evidence");
                }
            }
            InspectionStatus::Blocked => {
                if evidence.trim().is_empty() {
                    bail!("blocked requires a reason (evidence)");
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
        let now = now(&self.conn)?;
        self.conn.execute(
            "UPDATE edge SET status=?2,updated_at=?3 WHERE id=?1",
            params![edge_id, status.as_str(), now],
        )?;
        Ok(())
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
        Ok(Snapshot {
            identity,
            nodes,
            edges,
            facets,
            tags,
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
        tx.commit()?;
        Ok(())
    }

    // ---- ring 2: structural plane (sync + derived data) ------------------

    /// All CodeFile nodes.
    pub fn codefiles(&self) -> Result<Vec<Node>> {
        self.list_nodes(Some(NodeType::CodeFile), usize::MAX)
    }

    /// Edges filtered by any combination of kind / from / to. Used by the
    /// sync ripple to find what a changed file or intent invalidates.
    pub fn edges_with(
        &self,
        kind: Option<EdgeKind>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<Edge>> {
        let mut sql = format!("SELECT {EDGE_COLS} FROM edge WHERE 1=1");
        let mut args: Vec<String> = Vec::new();
        if let Some(k) = kind {
            sql.push_str(&format!(" AND kind=?{}", args.len() + 1));
            args.push(k.as_str().to_string());
        }
        if let Some(f) = from {
            sql.push_str(&format!(" AND from_id=?{}", args.len() + 1));
            args.push(f.to_string());
        }
        if let Some(t) = to {
            sql.push_str(&format!(" AND to_id=?{}", args.len() + 1));
            args.push(t.to_string());
        }
        sql.push_str(" ORDER BY id");
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> =
            args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(refs.as_slice(), row_to_edge)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Re-open an asserted edge whose dependency changed (sync ripple). Moves a
    /// settled verdict to `needs_reverification`. Distinct from `record_verdict`
    /// (it writes no verdict) and from `set_derived_status` (asserted only).
    /// Returns true if the edge was re-opened.
    pub fn stale_edge(&self, edge_id: &str) -> Result<bool> {
        let edge = self
            .get_edge(edge_id)?
            .ok_or_else(|| anyhow!("no edge '{edge_id}'"))?;
        if edge.truth_class != TruthClass::Asserted {
            return Ok(false);
        }
        match edge.status {
            InspectionStatus::Passing
            | InspectionStatus::Failing
            | InspectionStatus::Independent => {
                let now = now(&self.conn)?;
                self.conn.execute(
                    "UPDATE edge SET status='needs_reverification',updated_at=?2 WHERE id=?1",
                    params![edge_id, now],
                )?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Set a node's status directly (sync-owned, e.g. Validation last_result →
    /// not_run). Touches updated_at.
    pub fn set_node_status(&self, id: &str, status: &str) -> Result<()> {
        let now = now(&self.conn)?;
        let n = self.conn.execute(
            "UPDATE node SET status=?2,updated_at=?3 WHERE id=?1",
            params![id, status, now],
        )?;
        if n == 0 {
            bail!("no node '{id}'");
        }
        Ok(())
    }

    /// Add (or refresh) a derived node with a deterministic, content-addressed
    /// id and a fixed sentinel timestamp, so wipe+rebuild is byte-identical
    /// (INV-2). Sync-owned: derived truth class, never an asserted verdict.
    pub fn add_derived_node(
        &self,
        node_type: NodeType,
        det_key: &str,
        name: &str,
        description: &str,
        status: &str,
        body: serde_json::Value,
    ) -> Result<Node> {
        let id = derived_id(&[node_type.as_str(), det_key]);
        self.conn.execute(
            "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
             VALUES (?1,?2,?3,?4,?5,'derived',?6,?7,?7)
             ON CONFLICT(id) DO UPDATE SET name=?3,description=?4,status=?5,body=?6",
            params![id, node_type.as_str(), name, description, status, body.to_string(), DERIVED_TS],
        )?;
        self.get_node(&id)?
            .ok_or_else(|| anyhow!("derived node vanished"))
    }

    /// Validate an edge against the registry: truth-class allowed for the kind,
    /// both endpoints exist, and their node types match the kind's spec. Shared
    /// by `add_edge` (asserted) and `add_derived_edge` (derived) so deterministic
    /// ids never weaken edge-kind integrity.
    fn validate_edge_endpoints(
        &self,
        kind: EdgeKind,
        from_id: &str,
        to_id: &str,
        truth_class: TruthClass,
    ) -> Result<()> {
        let spec = registry::spec(kind);
        if !spec.allows_truth_class(truth_class) {
            bail!("edge kind '{kind}' does not allow truth_class '{truth_class}'");
        }
        let from = self
            .get_node(from_id)?
            .ok_or_else(|| anyhow!("from node '{from_id}' does not exist"))?;
        let to = self
            .get_node(to_id)?
            .ok_or_else(|| anyhow!("to node '{to_id}' does not exist"))?;
        if from.node_type != spec.from {
            bail!(
                "edge '{kind}' requires from-node type '{}', got '{}'",
                spec.from,
                from.node_type
            );
        }
        if to.node_type != spec.to {
            bail!(
                "edge '{kind}' requires to-node type '{}', got '{}'",
                spec.to,
                to.node_type
            );
        }
        Ok(())
    }

    /// Add (or refresh) a derived edge with a deterministic id. Sync-owned.
    pub fn add_derived_edge(&self, kind: EdgeKind, from_id: &str, to_id: &str) -> Result<Edge> {
        self.validate_edge_endpoints(kind, from_id, to_id, TruthClass::Derived)?;
        let id = derived_id(&["edge", kind.as_str(), from_id, to_id]);
        self.conn.execute(
            "INSERT INTO edge(id,from_id,to_id,kind,truth_class,status,created_at,updated_at)
             VALUES (?1,?2,?3,?4,'derived','current',?5,?5)
             ON CONFLICT(id) DO NOTHING",
            params![id, from_id, to_id, kind.as_str(), DERIVED_TS],
        )?;
        self.get_edge(&id)?
            .ok_or_else(|| anyhow!("derived edge vanished"))
    }

    /// Upsert a built-in seed node (e.g. a structural CodeRule) with a stable,
    /// content-addressed id and sentinel timestamp, so built-ins are identical
    /// across machines. Asserted truth class (a norm, not a derived occurrence).
    pub fn upsert_builtin_node(
        &self,
        node_type: NodeType,
        det_key: &str,
        name: &str,
        description: &str,
        body: serde_json::Value,
    ) -> Result<Node> {
        let id = derived_id(&["builtin", node_type.as_str(), det_key]);
        self.conn.execute(
            "INSERT INTO node(id,node_type,name,description,status,truth_class,body,created_at,updated_at)
             VALUES (?1,?2,?3,?4,'',  'asserted', ?5,?6,?6)
             ON CONFLICT(id) DO UPDATE SET description=?4, body=?5",
            params![id, node_type.as_str(), name, description, body.to_string(), DERIVED_TS],
        )?;
        self.get_node(&id)?
            .ok_or_else(|| anyhow!("builtin node vanished"))
    }

    /// Delete derived nodes + derived edges (Findings, flags, assesses, derived
    /// exposes). Run every sync before re-deriving findings.
    pub fn wipe_derived_graph(&self) -> Result<()> {
        // Derived edges first (some hang off asserted nodes); derived nodes then
        // cascade their remaining edges via FK.
        self.conn
            .execute("DELETE FROM edge WHERE truth_class='derived'", [])?;
        self.conn
            .execute("DELETE FROM node WHERE truth_class='derived'", [])?;
        Ok(())
    }

    /// Delete all derived facets (language, loc, content_hash, …).
    pub fn wipe_derived_facets(&self) -> Result<()> {
        self.conn
            .execute("DELETE FROM facet WHERE truth_class='derived'", [])?;
        Ok(())
    }

    /// Delete the ENTIRE derived plane — nodes, edges, and facets. The INV-2
    /// operation: after this, a `sync` rebuilds a byte-identical derived plane
    /// (and, because no prior content_hash remains, ripples nothing).
    pub fn wipe_derived(&self) -> Result<()> {
        self.wipe_derived_graph()?;
        self.wipe_derived_facets()?;
        Ok(())
    }
    /// Persist a durable adjudication verdict on a derived finding.
    ///
    /// Findings are rebuilt on every sync, but their ids are deterministic. Store
    /// the operator's judgment as an asserted facet on that stable id so it
    /// survives derived graph wipes, while stamping the current codefile hash so
    /// a future file edit can falsify the judgment.
    pub fn record_finding_verdict(
        &self,
        finding_id: &str,
        verdict: &str,
        reason: &str,
    ) -> Result<()> {
        let hash = self.finding_codefile_hash(finding_id)?.unwrap_or_default();
        let at = now(&self.conn)?;
        let adjudication = serde_json::json!({
            "verdict": verdict,
            "reason": reason,
            "hash": hash,
            "at": at,
        })
        .to_string();
        self.set_facet(
            finding_id,
            TargetKind::Node,
            "adjudication",
            &adjudication,
            TruthClass::Asserted,
        )
    }

    /// Current content hash of the codefile flagged by a finding.
    pub fn finding_codefile_hash(&self, finding_id: &str) -> Result<Option<String>> {
        let Some(flags) = self
            .edges_with(Some(EdgeKind::Flags), Some(finding_id), None)?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        self.get_facet(&flags.to_id, TargetKind::Node, "content_hash")
    }

    /// Intents that own (implement) the codefile a finding flags. Cohesion
    /// evidence for triage: one or two cohesive intents reads as justified
    /// length; many unrelated ones reads as a file that needs splitting.
    pub fn finding_owner_intents(&self, finding_id: &str) -> Result<Vec<Node>> {
        let Some(flags) = self
            .edges_with(Some(EdgeKind::Flags), Some(finding_id), None)?
            .into_iter()
            .next()
        else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for e in self.edges_with(Some(EdgeKind::Implements), None, Some(&flags.to_id))? {
            if let Some(n) = self.get_node(&e.from_id)? {
                if n.node_type == NodeType::Intent {
                    out.push(n);
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    /// Resolve a finding by exact id or unique id-prefix.
    ///
    /// Finding listings print short id prefixes; verdict writes must accept those
    /// without falling back to names or fragments.
    pub fn resolve_finding(&self, key: &str) -> Result<Node> {
        if let Some(n) = self.get_node(key)? {
            if n.node_type == NodeType::Finding {
                return Ok(n);
            }
        }
        let prefix = format!("{key}%");
        let matches = self.find_nodes_by(
            "id LIKE ?1",
            params![prefix],
            Some(NodeType::Finding.as_str()),
        )?;
        match matches.len() {
            0 => bail!("no finding matches '{key}'"),
            1 => Ok(matches.into_iter().next().expect("len == 1 by match arm")),
            n => bail!("ambiguous finding prefix '{key}': {n} match"),
        }
    }

    /// Read a derived facet value on a node (e.g. content_hash).
    pub fn get_facet(
        &self,
        target_id: &str,
        target_kind: TargetKind,
        key: &str,
    ) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM facet WHERE target_id=?1 AND target_kind=?2 AND key=?3",
                params![target_id, target_kind.as_str(), key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }

    // ---- ring 5: vocab + layer order -------------------------------------

    /// Register a vocabulary term (idempotent).
    pub fn add_vocab_term(&self, term: &str, description: &str) -> Result<()> {
        let now = now(&self.conn)?;
        self.conn.execute(
            "INSERT INTO tag_vocabulary(term,description,created_at) VALUES (?1,?2,?3)
             ON CONFLICT(term) DO UPDATE SET description=?2",
            params![term, description, now],
        )?;
        Ok(())
    }

    /// All registered vocabulary terms.
    pub fn list_vocab(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT term,description FROM tag_vocabulary ORDER BY term")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Whether a term is registered.
    pub fn vocab_has(&self, term: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM tag_vocabulary WHERE term=?1",
                params![term],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Set a meta key (e.g. the layer order JSON).
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
            params![key, value],
        )?;
        Ok(())
    }

    /// Read a meta key.
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(Into::into)
    }
}

// ---- helpers -------------------------------------------------------------

fn schema_migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(SCHEMA)])
}

fn apply_schema_migrations(conn: &mut Connection) -> Result<()> {
    adopt_legacy_schema_version(conn)?;
    schema_migrations()
        .to_latest(conn)
        .context("migrating graph schema")?;
    Ok(())
}

fn adopt_legacy_schema_version(conn: &Connection) -> Result<()> {
    let user_version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if user_version != 0 {
        return Ok(());
    }

    let has_meta = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='meta'",
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !has_meta {
        return Ok(());
    }

    let legacy_schema_version = conn
        .query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?;

    if legacy_schema_version
        .as_deref()
        .and_then(|s| s.parse::<u32>().ok())
        == Some(SCHEMA_VERSION)
    {
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
    Ok(())
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(())
}

fn acquire_lock(loom_dir: &Path) -> Result<File> {
    let lock_path = loom_dir.join("lock");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("opening lock {}", lock_path.display()))?;
    // Retry briefly: a just-dropped lock from a prior open in this or another
    // process can lag a few ms before the OS releases it. WAL + busy_timeout
    // handle real query concurrency; this flock only guards the open boundary.
    let mut wait = std::time::Duration::from_millis(5);
    for attempt in 0..40 {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(_) if attempt < 39 => {
                std::thread::sleep(wait);
                if wait < std::time::Duration::from_millis(50) {
                    wait *= 2;
                }
            }
            Err(_) => break,
        }
    }
    bail!("graph is locked by another loom process")
}

/// Sentinel timestamp for derived rows. Derived data is recomputed by sync, so
/// its creation time is meaningless; a fixed sentinel keeps wipe+rebuild output
/// byte-identical (INV-2).
const DERIVED_TS: &str = "";

/// Deterministic, content-addressed id for derived data (FNV-1a 64-bit over the
/// joined parts). The same inputs always yield the same id, so a wiped-and-
/// rebuilt derived plane is byte-identical.
fn derived_id(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h ^= 0x1f;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        for b in p.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("d{h:016x}")
}

/// Generate a fresh 128-bit hex id and an RFC3339 timestamp in one query, using
/// SQLite's own functions (no external rng/clock crate).
fn id_and_now(conn: &Connection) -> Result<(String, String)> {
    conn.query_row(
        "SELECT lower(hex(randomblob(16))), strftime('%Y-%m-%dT%H:%M:%fZ','now')",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .map_err(Into::into)
}

fn now(conn: &Connection) -> Result<String> {
    conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
        r.get::<_, String>(0)
    })
    .map_err(Into::into)
}

fn parse_named<T: std::str::FromStr>(row: &rusqlite::Row, col: &str) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(col)?;
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

fn parse_col<T: std::str::FromStr>(row: &rusqlite::Row, idx: usize) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(idx)?;
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

/// Column list for node SELECTs. Order-independent (mappers read by name) but
/// kept as one constant so every query selects the full row.
const NODE_COLS: &str =
    "id,node_type,name,description,status,truth_class,body,created_at,updated_at";

/// Column list for edge SELECTs.
const EDGE_COLS: &str = "id,from_id,to_id,kind,truth_class,status,criterion,evidence,\
                         confidence,depends_on,inspected_by,created_at,updated_at";

fn row_to_node(r: &rusqlite::Row) -> rusqlite::Result<Node> {
    let body_str: String = r.get("body")?;
    Ok(Node {
        id: r.get("id")?,
        node_type: parse_named(r, "node_type")?,
        name: r.get("name")?,
        description: r.get("description")?,
        status: r.get("status")?,
        truth_class: parse_named(r, "truth_class")?,
        body: serde_json::from_str(&body_str)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn row_to_edge(r: &rusqlite::Row) -> rusqlite::Result<Edge> {
    let depends_str: String = r.get("depends_on")?;
    Ok(Edge {
        id: r.get("id")?,
        from_id: r.get("from_id")?,
        to_id: r.get("to_id")?,
        kind: parse_named(r, "kind")?,
        truth_class: parse_named(r, "truth_class")?,
        status: parse_named(r, "status")?,
        criterion: r.get("criterion")?,
        evidence: r.get("evidence")?,
        confidence: r.get("confidence")?,
        depends_on: serde_json::from_str(&depends_str)
            .unwrap_or_else(|_| serde_json::Value::Array(Vec::new())),
        inspected_by: r.get("inspected_by")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TmpRoot(PathBuf);

    impl TmpRoot {
        fn new(prefix: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("{prefix}-{}-{nanos}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sqlite_user_version(conn: &Connection) -> u32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn graph_schema_migrations_are_valid() {
        schema_migrations().validate().unwrap();
    }

    #[test]
    fn fresh_init_sets_sqlite_user_version() {
        let tmp = TmpRoot::new("loom-store-fresh-migration");
        let store = Store::init(tmp.path(), Some("fresh"), false).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), SCHEMA_VERSION);
        assert_eq!(store.identity().unwrap().schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn old_style_schema_is_adopted_without_rerunning_create_table() {
        let tmp = TmpRoot::new("loom-store-legacy-migration");
        let loom_dir = tmp.path().join(LOOM_DIR);
        std::fs::create_dir_all(&loom_dir).unwrap();
        let db_path = loom_dir.join(GRAPH_DB);
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO meta(key,value) VALUES
                 ('graph_id','legacy'),
                 ('name','legacy'),
                 ('schema_version',?1),
                 ('observed','0'),
                 ('created_at','legacy')",
                params![SCHEMA_VERSION.to_string()],
            )
            .unwrap();
            assert_eq!(sqlite_user_version(&conn), 0);
        }

        let store = Store::open(tmp.path()).unwrap();
        assert_eq!(sqlite_user_version(&store.conn), SCHEMA_VERSION);
        assert_eq!(store.identity().unwrap().name, "legacy");
    }
}
