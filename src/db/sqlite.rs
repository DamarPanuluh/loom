//! SQLite graph storage primitives.
//!
//! This module is the migration target for Loom's live graph store. It keeps
//! the product model graph-shaped, but stores it in typed relational tables:
//! node tables for each plane and typed edge tables with foreign keys and
//! endpoint uniqueness. The command layer now uses this store directly while
//! shared graph analysis stays in Rust over loaded snapshots.

use anyhow::{Context, Result};
use fs2::FileExt;
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Map, Value as JsonValue};
#[cfg(test)]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::db::queries::find::{
    door_matches_from_planes, rank_intents_from_parts, DoorMatches, FindHit,
};
use crate::db::queries::{
    align_candidates_from_snapshot_notes, check_graph_from_parts, compute_smells_from_parts,
    graph_state_from_snapshot_parts, prove_candidates_from_parts, AlignCandidate, DoctorInputs,
    DoctorReport, GraphMeta, GraphState, GraphStateContext, QuerySnapshot, RedefinitionRipple,
    RetireFallout, SmellInputs, DEFAULT_TRANSITION_CAP,
};
use crate::db::schema::{self, edge, label, prop};
use crate::types::{
    interface_surface_name, CallsEdge, CodeFile, Delegation, Governs, Hierarchy, Hypothesis,
    Ignore, Implements, InboxItem, Intent, InterfaceSurface, JourneysEdge, Note, Persona,
    QualityRule, RelatesTo, ServesEdge, SymbolFact, TargetsEdge, ValidatesEdge, Validation,
    VocabTerm,
};

mod edge_writes;
mod import_export;
mod reads;
mod schema_ddl;
mod search;
mod writes;

pub struct SqliteGraphStore {
    conn: Connection,
    /// Sibling lock file (`.loom/graph.lock`) backing the cross-process
    /// single-writer guarantee. `None` for the in-memory test store.
    lock_path: Option<PathBuf>,
    /// The held exclusive write lock, acquired lazily on the FIRST write (a
    /// `write_tx` OR a single-statement `write_one`) and kept for the store's
    /// lifetime. Read-only commands never write, so they never acquire it — WAL
    /// keeps readers concurrent with the single writer. `RefCell` so a writer
    /// method taking `&self` (most do) can still lazily take the lock; the store
    /// is single-threaded (one CLI process), so no `Sync` is required.
    write_lock: std::cell::RefCell<Option<std::fs::File>>,
}

#[derive(Debug, Clone, Copy)]
struct NodeSpec {
    label: &'static str,
    table: &'static str,
    props: &'static [&'static str],
    list_props: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EdgeSpec {
    edge_type: &'static str,
    table: &'static str,
    from_col: &'static str,
    to_col: &'static str,
    props: &'static [&'static str],
    numeric_props: &'static [&'static str],
    list_props: &'static [&'static str],
}

fn checked_sql_ident(ident: &str) -> Result<&str> {
    let mut bytes = ident.bytes();
    let Some(first) = bytes.next() else {
        anyhow::bail!("empty SQL identifier");
    };
    if !(first == b'_' || first.is_ascii_alphabetic()) {
        anyhow::bail!("unsafe SQL identifier: {ident:?}");
    }
    if bytes.any(|byte| !(byte == b'_' || byte.is_ascii_alphanumeric())) {
        anyhow::bail!("unsafe SQL identifier: {ident:?}");
    }
    Ok(ident)
}

fn checked_sql_ident_list(idents: &[&str]) -> Result<String> {
    let mut checked = Vec::with_capacity(idents.len());
    for ident in idents {
        checked.push(checked_sql_ident(ident)?);
    }
    Ok(checked.join(", "))
}

fn sql_string_literal(value: &str) -> String {
    let mut literal = String::with_capacity(value.len() + 2);
    literal.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            literal.push_str("''");
        } else {
            literal.push(ch);
        }
    }
    literal.push('\'');
    literal
}

const INTENT_PROPS: &[&str] = &[
    prop::ID,
    prop::NAME,
    prop::DESCRIPTION,
    prop::CRITERION,
    prop::ABSTRACTION_LEVEL,
    prop::DOMAIN,
    prop::LAYER,
    prop::SOURCE_REFS,
    prop::STATUS,
    prop::ASPECT,
    prop::LIFECYCLE,
    prop::CREATED_AT,
    prop::UPDATED_AT,
    prop::TAGS,
    prop::VISIBILITY,
    prop::BOUNDARY,
];

const CODEFILE_PROPS: &[&str] = &[
    prop::ID,
    prop::PATH,
    prop::LANGUAGE,
    prop::LAST_MODIFIED,
    prop::IMPORTS,
    prop::SYMBOLS,
    prop::SYMBOL_FACTS,
    prop::CONTENT_HASH,
    prop::EXTRACTOR_GRADE,
];

const QUALITY_RULE_PROPS: &[&str] = &[
    prop::ID,
    prop::NAME,
    prop::DESCRIPTION,
    prop::DETECTION_LOGIC,
    prop::KIND,
    prop::SEVERITY,
    prop::INSPECTION_EFFORT,
];

const VALIDATION_PROPS: &[&str] = &[
    prop::ID,
    prop::NAME,
    prop::DESCRIPTION,
    prop::VALIDATION_TYPE,
    prop::COMMAND,
    prop::LAST_RUN,
    prop::LAST_RESULT,
    prop::LAST_EXECUTED_RUN,
    prop::DISCRIMINATION_STATUS,
];

const NOTE_PROPS: &[&str] = &[
    prop::ID,
    prop::KIND,
    prop::TEXT,
    prop::AUTHOR,
    prop::TARGET_KIND,
    prop::TARGET_ID,
    prop::CREATED_AT,
    prop::AUDIENCE,
    prop::RESOLUTION,
];

const IGNORE_PROPS: &[&str] = &[
    prop::ID,
    prop::PATTERN,
    prop::REASON,
    prop::AUTHOR,
    prop::CREATED_AT,
];
const DELEGATION_PROPS: &[&str] = &[
    prop::ID,
    prop::PATTERN,
    prop::TARGET,
    prop::EXPORT_HASH,
    prop::SEAM_INTENTS,
    prop::AUTHOR,
    prop::CREATED_AT,
];
const DELEGATION_LIST_PROPS: &[&str] = &[prop::SEAM_INTENTS];

const HYPOTHESIS_PROPS: &[&str] = &[
    prop::ID,
    prop::NAME,
    prop::CLAIM,
    prop::PROPOSAL,
    prop::PREDICTED_OUTCOME,
    prop::STATUS,
    prop::AUTHOR,
    prop::EVIDENCE,
    prop::LAST_INSPECTED,
    prop::INSPECTED_BY,
    prop::CREATED_AT,
    prop::UPDATED_AT,
];

const VOCAB_TERM_PROPS: &[&str] = &[
    prop::ID,
    prop::NAME,
    prop::DESCRIPTION,
    prop::AUTHOR,
    prop::CREATED_AT,
];
const PERSONA_PROPS: &[&str] = &[
    prop::ID,
    prop::NAME,
    prop::DESCRIPTION,
    prop::AUTHOR,
    prop::CREATED_AT,
    prop::UPDATED_AT,
];
const INTERFACE_SURFACE_PROPS: &[&str] = &[
    prop::ID,
    prop::NAME,
    prop::DESCRIPTION,
    prop::SURFACE_KIND,
    prop::METHOD,
    prop::TARGET,
    prop::CREATED_AT,
    prop::UPDATED_AT,
];

const INBOX_ITEM_PROPS: &[&str] = &[
    prop::ID,
    prop::RAW_TEXT,
    prop::NORMALIZED_CLAIM,
    prop::KIND,
    prop::STATUS,
    prop::SOURCE,
    prop::AUTHOR,
    prop::TAGS,
    prop::LINKS,
    prop::ROUTE_KIND,
    prop::ROUTE_COMMAND,
    prop::ROUTE_TARGET_KIND,
    prop::ROUTE_TARGET_ID,
    prop::RESOLUTION,
    prop::CREATED_AT,
    prop::UPDATED_AT,
];

const INSPECTABLE_PROPS_WITH_PRIORITY: &[&str] = &[
    prop::INSPECTION_STATUS,
    prop::CRITERION,
    prop::CONFIDENCE,
    prop::EVIDENCE,
    prop::LAST_INSPECTED,
    prop::INSPECTED_BY,
    prop::PRIORITY_SCORE,
    prop::KINDS,
    prop::STABLE,
    prop::NOTES,
    prop::CREATED_AT,
];

/// RELATES_TO is the only edge with a list-valued prop (the kind multiset).
const RELATES_TO_LIST_PROPS: &[&str] = &[prop::KINDS];

const INSPECTABLE_PROPS: &[&str] = &[
    prop::INSPECTION_STATUS,
    prop::CRITERION,
    prop::CONFIDENCE,
    prop::EVIDENCE,
    prop::LAST_INSPECTED,
    prop::INSPECTED_BY,
    prop::NOTES,
    prop::CREATED_AT,
];

const IMPLEMENTS_PROPS: &[&str] = &[
    prop::INSPECTION_STATUS,
    prop::CRITERION,
    prop::CONFIDENCE,
    prop::EVIDENCE,
    prop::LAST_INSPECTED,
    prop::INSPECTED_BY,
    prop::LOCATOR,
    prop::NOTES,
    prop::CREATED_AT,
];

const STRUCTURAL_PROPS: &[&str] = &[prop::NOTES, prop::CREATED_AT];
const VALIDATES_PROPS: &[&str] = &[prop::INSPECTION_STATUS, prop::NOTES, prop::CREATED_AT];
const CALLS_PROPS: &[&str] = &[
    prop::STEP_INDEX,
    prop::STEP_NAME,
    prop::INTENT_ID,
    prop::NOTES,
    prop::CREATED_AT,
];
const CONFIDENCE_ONLY: &[&str] = &[prop::CONFIDENCE];
const CONFIDENCE_AND_PRIORITY: &[&str] = &[prop::CONFIDENCE, prop::PRIORITY_SCORE];
const EMPTY_LIST_PROPS: &[&str] = &[];
const INTENT_LIST_PROPS: &[&str] = &[prop::SOURCE_REFS, prop::TAGS];
const CODEFILE_LIST_PROPS: &[&str] = &[prop::IMPORTS, prop::SYMBOLS, prop::SYMBOL_FACTS];
const INBOX_ITEM_LIST_PROPS: &[&str] = &[prop::TAGS, prop::LINKS];

const NODE_SPECS: &[NodeSpec] = &[
    NodeSpec {
        label: label::INTENT,
        table: "intent",
        props: INTENT_PROPS,
        list_props: INTENT_LIST_PROPS,
    },
    NodeSpec {
        label: label::CODE_FILE,
        table: "codefile",
        props: CODEFILE_PROPS,
        list_props: CODEFILE_LIST_PROPS,
    },
    NodeSpec {
        label: label::QUALITY_RULE,
        table: "quality_rule",
        props: QUALITY_RULE_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::VALIDATION,
        table: "validation",
        props: VALIDATION_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::NOTE,
        table: "note",
        props: NOTE_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::IGNORE,
        table: "ignore_rule",
        props: IGNORE_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::DELEGATION,
        table: "delegation",
        props: DELEGATION_PROPS,
        list_props: DELEGATION_LIST_PROPS,
    },
    NodeSpec {
        label: label::HYPOTHESIS,
        table: "hypothesis",
        props: HYPOTHESIS_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::VOCAB_TERM,
        table: "vocab_term",
        props: VOCAB_TERM_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::PERSONA,
        table: "persona",
        props: PERSONA_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::INTERFACE_SURFACE,
        table: "interface_surface",
        props: INTERFACE_SURFACE_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    NodeSpec {
        label: label::INBOX_ITEM,
        table: "inbox_item",
        props: INBOX_ITEM_PROPS,
        list_props: INBOX_ITEM_LIST_PROPS,
    },
];

const EDGE_SPECS: &[EdgeSpec] = &[
    EdgeSpec {
        edge_type: edge::RELATES_TO,
        table: "relates_to",
        from_col: "from_id",
        to_col: "to_id",
        props: INSPECTABLE_PROPS_WITH_PRIORITY,
        numeric_props: CONFIDENCE_AND_PRIORITY,
        list_props: RELATES_TO_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::HIERARCHY,
        table: "hierarchy",
        from_col: "parent_id",
        to_col: "child_id",
        props: STRUCTURAL_PROPS,
        numeric_props: EMPTY_LIST_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::IMPLEMENTS,
        table: "implements",
        from_col: "intent_id",
        to_col: "codefile_id",
        props: IMPLEMENTS_PROPS,
        numeric_props: CONFIDENCE_ONLY,
        list_props: EMPTY_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::GOVERNS,
        table: "governs",
        from_col: "rule_id",
        to_col: "intent_id",
        props: INSPECTABLE_PROPS,
        numeric_props: CONFIDENCE_ONLY,
        list_props: EMPTY_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::VALIDATES,
        table: "validates",
        from_col: "validation_id",
        to_col: "intent_id",
        props: VALIDATES_PROPS,
        numeric_props: EMPTY_LIST_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::TARGETS,
        table: "targets",
        from_col: "hypothesis_id",
        to_col: "intent_id",
        props: INSPECTABLE_PROPS,
        numeric_props: CONFIDENCE_ONLY,
        list_props: EMPTY_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::SERVES,
        table: "serves",
        from_col: "persona_id",
        to_col: "intent_id",
        props: INSPECTABLE_PROPS,
        numeric_props: CONFIDENCE_ONLY,
        list_props: EMPTY_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::JOURNEYS,
        table: "journeys",
        from_col: "persona_id",
        to_col: "validation_id",
        props: STRUCTURAL_PROPS,
        numeric_props: EMPTY_LIST_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
    EdgeSpec {
        edge_type: edge::CALLS,
        table: "calls",
        from_col: "validation_id",
        to_col: "interface_id",
        props: CALLS_PROPS,
        numeric_props: EMPTY_LIST_PROPS,
        list_props: EMPTY_LIST_PROPS,
    },
];

impl SqliteGraphStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Could not open SQLite graph at {}", path.display()))?;
        configure_connection(&conn, true)?;
        let store = Self {
            conn,
            lock_path: Some(path.with_extension("lock")),
            write_lock: std::cell::RefCell::new(None),
        };
        store.create_schema()?;
        Ok(store)
    }
    /// Read-only open: the graph file is opened `SQLITE_OPEN_READ_ONLY` (no write
    /// access, no WAL write-lock, no schema setup), for read consumers that must
    /// not mutate the file or pay per-invocation schema setup. Falls back to the
    /// read-write `open` (which migrates) when the on-disk schema version differs
    /// from this binary's — so an older graph still upgrades on first touch.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        use rusqlite::OpenFlags;
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
        // Any obstacle to a clean read-only open (can't open the flag, can't
        // configure, stale/unreadable schema) falls back to the read-write
        // `open`, which migrates and always works — so this fast path is a pure
        // optimization that never changes behaviour.
        let Ok(conn) = Connection::open_with_flags(path, flags) else {
            return Self::open(path);
        };
        if configure_connection(&conn, false).is_err() {
            drop(conn);
            return Self::open(path);
        }
        let current: Option<String> = conn
            .query_row("SELECT schema_version FROM meta WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .unwrap_or(None);
        if current.as_deref() != Some(schema::SCHEMA_VERSION) {
            drop(conn);
            return Self::open(path);
        }
        Ok(Self {
            conn,
            lock_path: None,
            write_lock: std::cell::RefCell::new(None),
        })
    }
    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn, false)?;
        let store = Self {
            conn,
            lock_path: None,
            write_lock: std::cell::RefCell::new(None),
        };
        store.create_schema()?;
        Ok(store)
    }
    /// Begin a WRITE transaction. Acquires the cross-process exclusive write
    /// lock (lazily, once per store) so at most one loom process writes at a
    /// time, then opens a `BEGIN IMMEDIATE` transaction — taking the reserved
    /// lock up front rather than on first write, which sidesteps the
    /// `SQLITE_BUSY_SNAPSHOT` that a read-then-upgrade DEFERRED transaction can
    /// hit (and which `busy_timeout` does NOT retry).
    fn write_tx(&mut self) -> Result<rusqlite::Transaction<'_>> {
        self.ensure_write_lock()?;
        Ok(self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?)
    }
    /// Acquire the cross-process exclusive write lock if this store hasn't yet
    /// (lazy, once per session). EVERY write path — `write_tx` and the
    /// single-statement `write_one` — funnels through here, so the
    /// single-writer / named-error contract holds for ALL writes, not just
    /// transactions. A bare `self.conn.execute` that skips this would surface a
    /// raw "database is locked" that `LOOM_LOCK_DEADLINE_MS` can't bound.
    fn ensure_write_lock(&self) -> Result<()> {
        let mut held = self.write_lock.borrow_mut();
        if held.is_none() {
            if let Some(lock_path) = self.lock_path.as_ref() {
                *held = Some(acquire_write_lock(lock_path, lock_deadline_ms())?);
            }
        }
        Ok(())
    }
    /// A single-statement write that takes the flock first. The `&self`-friendly
    /// counterpart to `write_tx`: use it for one-shot `UPDATE`/`INSERT`/`DELETE`
    /// writers (most of them) so they serialize cross-process like everything
    /// else. Read-modify-write sequences must use `write_tx` for atomicity, not
    /// a pair of `write_one` calls.
    fn write_one<P: rusqlite::Params>(&self, sql: &str, params: P) -> Result<usize> {
        self.ensure_write_lock()?;
        Ok(self.conn.execute(sql, params)?)
    }
    fn list_active_intents(&self) -> Result<Vec<Intent>> {
        self.list_intents_matching(true)
    }
    fn list_all_intents(&self) -> Result<Vec<Intent>> {
        self.list_intents_matching(false)
    }
    fn list_intents_matching(&self, active_only: bool) -> Result<Vec<Intent>> {
        let sql = if active_only {
            "SELECT id, name, description, abstraction_level, domain, layer, source_refs, status,
                    aspect, tags, visibility, boundary, lifecycle, created_at, updated_at, criterion
             FROM intent
             WHERE status <> 'deprecated'
             ORDER BY name, id"
        } else {
            "SELECT id, name, description, abstraction_level, domain, layer, source_refs, status,
                    aspect, tags, visibility, boundary, lifecycle, created_at, updated_at, criterion
             FROM intent
             ORDER BY name, id"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let mut intents = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0).context("load intent id")?;
            let source_refs_raw: String = row
                .get(6)
                .with_context(|| format!("load source_refs for intent {id}"))?;
            let tags_raw: String = row
                .get(9)
                .with_context(|| format!("load tags for intent {id}"))?;
            let source_refs = string_list_sql(source_refs_raw.as_str())
                .with_context(|| format!("parse source_refs for intent {id}"))?;
            let tags = string_list_sql(tags_raw.as_str())
                .with_context(|| format!("parse tags for intent {id}"))?;
            intents.push(Intent {
                id,
                name: row.get(1)?,
                description: row.get(2)?,
                abstraction_level: row.get(3)?,
                domain: row.get(4)?,
                layer: row.get(5)?,
                source_refs,
                status: row.get(7)?,
                aspect: row.get(8)?,
                tags,
                visibility: row.get(10)?,
                boundary: row.get(11)?,
                lifecycle: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
                criterion: row.get(15)?,
            });
        }
        Ok(intents)
    }
    fn doctor_node_counts(&self, snapshot: &QuerySnapshot) -> Result<Vec<(String, i64)>> {
        NODE_SPECS
            .iter()
            .map(|spec| {
                let snapshot_count = match spec.label {
                    label::INTENT => Some(snapshot.intents.len() as i64),
                    label::CODE_FILE => Some(snapshot.codefiles.len() as i64),
                    label::QUALITY_RULE => Some(snapshot.rules.len() as i64),
                    label::VALIDATION => Some(snapshot.validations.len() as i64),
                    _ => None,
                };
                Ok((
                    spec.label.to_string(),
                    snapshot_count.unwrap_or(count_table(&self.conn, spec.table)? as i64),
                ))
            })
            .collect()
    }
    fn doctor_edge_counts(&self, snapshot: &QuerySnapshot) -> Result<Vec<(String, i64)>> {
        EDGE_SPECS
            .iter()
            .map(|spec| {
                let snapshot_count = match spec.edge_type {
                    edge::RELATES_TO => Some(snapshot.relates.len() as i64),
                    edge::HIERARCHY => Some(snapshot.hierarchy.len() as i64),
                    edge::IMPLEMENTS => Some(snapshot.implements.len() as i64),
                    edge::GOVERNS => Some(snapshot.governs.len() as i64),
                    edge::VALIDATES => Some(snapshot.validates.len() as i64),
                    _ => None,
                };
                Ok((
                    spec.edge_type.to_string(),
                    snapshot_count.unwrap_or(count_table(&self.conn, spec.table)? as i64),
                ))
            })
            .collect()
    }
    fn missing_node_props(&self) -> Result<Vec<(String, String, i64)>> {
        let mut missing = Vec::new();
        for spec in NODE_SPECS {
            for &prop in spec.props {
                let count = self.count_null_column(spec.table, prop)?;
                if count > 0 {
                    missing.push((spec.label.to_string(), prop.to_string(), count));
                }
            }
        }
        Ok(missing)
    }
    fn missing_edge_props(&self) -> Result<Vec<(String, String, i64)>> {
        let mut missing = Vec::new();
        for spec in EDGE_SPECS {
            for &prop in spec.props {
                let count = self.count_null_column(spec.table, prop)?;
                if count > 0 {
                    missing.push((spec.edge_type.to_string(), prop.to_string(), count));
                }
            }
        }
        Ok(missing)
    }
    fn count_null_column(&self, table: &str, column: &str) -> Result<i64> {
        let table = checked_sql_ident(table)?;
        let column = checked_sql_ident(column)?;
        if !table_has_column(&self.conn, table, column)? {
            return Ok(count_table(&self.conn, table)? as i64);
        }
        let sql = format!("SELECT count(*) FROM {table} WHERE {column} IS NULL");
        self.conn
            .query_row(&sql, [], |row| row.get::<_, i64>(0))
            .map_err(Into::into)
    }
    fn collect_edge_ids(&self) -> Result<std::collections::HashSet<String>> {
        let mut ids = std::collections::HashSet::new();
        for spec in EDGE_SPECS {
            if spec.edge_type == edge::CALLS {
                for call in self.list_all_calls()? {
                    ids.insert(call.id);
                }
                continue;
            }
            for (from, to) in edge_pairs(&self.conn, spec)? {
                ids.insert(crate::db::schema::edge_key(spec.edge_type, &from, &to));
            }
        }
        Ok(ids)
    }
    fn hierarchy_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT parent_id, child_id FROM hierarchy")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    fn list_hierarchy_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT parent_id, child_id FROM hierarchy ORDER BY parent_id, child_id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    fn list_relates_to(&self) -> Result<Vec<RelatesTo>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.from_id, e.to_id, src.name, dst.name, e.inspection_status, e.criterion,
                    e.confidence, e.evidence, e.last_inspected, e.inspected_by, e.priority_score,
                    e.notes, e.kinds, e.stable
             FROM relates_to e
             JOIN intent src ON src.id = e.from_id
             JOIN intent dst ON dst.id = e.to_id
             ORDER BY e.priority_score DESC",
        )?;
        let rows = stmt.query_map([], |row| {
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
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    fn list_all_governs(&self) -> Result<Vec<Governs>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.rule_id, e.intent_id, r.name, i.name, e.inspection_status, e.criterion,
                    e.confidence, e.evidence, e.last_inspected, e.inspected_by, e.notes
             FROM governs e
             JOIN quality_rule r ON r.id = e.rule_id
             JOIN intent i ON i.id = e.intent_id
             ORDER BY e.last_inspected DESC",
        )?;
        let rows = stmt.query_map([], |row| {
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
    fn list_all_validates(&self) -> Result<Vec<ValidatesEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.validation_id, e.intent_id, v.name, i.name, e.created_at,
                    e.inspection_status, e.notes
             FROM validates e
             JOIN validation v ON v.id = e.validation_id
             JOIN intent i ON i.id = e.intent_id
             ORDER BY v.name, i.name",
        )?;
        let rows = stmt.query_map([], |row| {
            let validation_id: String = row.get(0)?;
            let intent_id: String = row.get(1)?;
            Ok(ValidatesEdge {
                id: crate::db::schema::edge_key(edge::VALIDATES, &validation_id, &intent_id),
                validation_id,
                intent_id,
                validation_name: row.get(2)?,
                intent_name: row.get(3)?,
                created_at: row.get(4)?,
                inspection_status: row.get(5)?,
                notes: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    fn list_all_implements(&self) -> Result<Vec<Implements>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.intent_id, e.codefile_id, i.name, cf.path, e.inspection_status,
                    e.criterion, e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                    e.locator, e.notes, e.created_at
             FROM implements e
             JOIN intent i ON i.id = e.intent_id
             JOIN codefile cf ON cf.id = e.codefile_id
             ORDER BY e.rowid",
        )?;
        let rows = stmt.query_map([], |row| {
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
    fn count_hypotheses(&self, status: Option<&str>) -> Result<usize> {
        Ok(self.list_hypotheses(status)?.len())
    }
    fn groundings_for_intent(&self, intent_id: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT cf.path, e.locator
             FROM implements e
             JOIN codefile cf ON cf.id = e.codefile_id
             WHERE e.intent_id = ?1
             ORDER BY cf.path, e.locator",
        )?;
        let rows = stmt.query_map(params![intent_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
    fn stale_edge_count(&self, intent_id: &str) -> Result<usize> {
        const STALE: &str = "needs_reverification";
        let relates = self.conn.query_row(
            "SELECT count(*) FROM relates_to
             WHERE inspection_status = ?1 AND (from_id = ?2 OR to_id = ?2)",
            params![STALE, intent_id],
            |row| row.get::<_, i64>(0),
        )?;
        let governs = self.conn.query_row(
            "SELECT count(*) FROM governs WHERE inspection_status = ?1 AND intent_id = ?2",
            params![STALE, intent_id],
            |row| row.get::<_, i64>(0),
        )?;
        let validates = self.conn.query_row(
            "SELECT count(*) FROM validates WHERE inspection_status = ?1 AND intent_id = ?2",
            params![STALE, intent_id],
            |row| row.get::<_, i64>(0),
        )?;
        let implements = self.conn.query_row(
            "SELECT count(*) FROM implements WHERE inspection_status = ?1 AND intent_id = ?2",
            params![STALE, intent_id],
            |row| row.get::<_, i64>(0),
        )?;
        Ok((relates + governs + validates + implements) as usize)
    }
    fn resolve_validation(&self, key: &str) -> Result<Validation> {
        let validations = self.list_validations()?;
        if let Some(validation) = validations.iter().find(|validation| validation.id == key) {
            return Ok(validation.clone());
        }
        let kl = key.to_lowercase();
        let exact: Vec<_> = validations
            .iter()
            .filter(|validation| validation.name.to_lowercase() == kl)
            .collect();
        if exact.len() == 1 {
            return Ok(exact[0].clone());
        }
        let subs: Vec<_> = validations
            .iter()
            .filter(|validation| validation.name.to_lowercase().contains(&kl))
            .collect();
        match subs.len() {
            1 => Ok(subs[0].clone()),
            0 => anyhow::bail!(
                "No validation matches '{}' (by id, name, or fragment). Run `loom validation list`.",
                key
            ),
            _ => anyhow::bail!(
                "'{}' is ambiguous — matches {} validations. Use the id (`loom validation list`).",
                key,
                subs.len()
            ),
        }
    }
    fn list_all_notes(&self) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, text, author, target_kind, target_id, audience, created_at, resolution
             FROM note
             ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Note {
                id: row.get(0)?,
                kind: row.get(1)?,
                text: row.get(2)?,
                author: row.get(3)?,
                target_kind: row.get(4)?,
                target_id: row.get(5)?,
                audience: row.get(6)?,
                created_at: row.get(7)?,
                resolution: row.get(8)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn insert_transition_note_tx(
    tx: &rusqlite::Transaction<'_>,
    target_kind: &str,
    target_id: &str,
    old_status: &str,
    new_status: &str,
    author: &str,
    now: &str,
) -> Result<()> {
    if old_status == new_status {
        return Ok(());
    }
    let old_status = if old_status.is_empty() {
        "?"
    } else {
        old_status
    };
    tx.execute(
        "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
         VALUES(?1, 'transition', ?2, ?3, ?4, ?5, ?6, '')",
        params![
            uuid::Uuid::new_v4().to_string(),
            format!("{old_status} → {new_status}"),
            author,
            target_kind,
            target_id,
            now
        ],
    )?;
    Ok(())
}

fn insert_sync_flip_note_tx(
    tx: &rusqlite::Transaction<'_>,
    target_kind: &str,
    target_id: &str,
    old_status: &str,
    new_status: &str,
    cause: &str,
    now: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
         VALUES(?1, 'transition', ?2, 'loom', ?3, ?4, ?5, '')",
        params![
            uuid::Uuid::new_v4().to_string(),
            format!(
                "{} → {} (sync: {})",
                if old_status.is_empty() {
                    "?"
                } else {
                    old_status
                },
                new_status,
                cause
            ),
            target_kind,
            target_id,
            now
        ],
    )?;
    Ok(())
}

fn inbox_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InboxItem> {
    Ok(InboxItem {
        id: row.get(0)?,
        raw_text: row.get(1)?,
        normalized_claim: row.get(2)?,
        kind: row.get(3)?,
        status: row.get(4)?,
        source: row.get(5)?,
        author: row.get(6)?,
        tags: string_list_sql(row.get::<_, String>(7)?.as_str())?,
        links: string_list_sql(row.get::<_, String>(8)?.as_str())?,
        route_kind: row.get(9)?,
        route_command: row.get(10)?,
        route_target_kind: row.get(11)?,
        route_target_id: row.get(12)?,
        resolution: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}
fn hierarchy_reaches(edges: &[(String, String)], start: &str, target: &str) -> bool {
    if start == target {
        return true;
    }
    let mut stack = vec![start.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = stack.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        for (_, child) in edges.iter().filter(|(parent, _)| parent == &current) {
            if child == target {
                return true;
            }
            stack.push(child.clone());
        }
    }
    false
}

fn calls_edge_key(validation_id: &str, interface_id: &str, step_index: usize) -> String {
    format!("call:{validation_id}:{interface_id}:{step_index}")
}

fn calls_edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CallsEdge> {
    let validation_id: String = row.get(0)?;
    let interface_id: String = row.get(1)?;
    let raw_step_index: String = row.get(4)?;
    let step_index = raw_step_index.parse::<usize>().unwrap_or(0);
    Ok(CallsEdge {
        id: calls_edge_key(&validation_id, &interface_id, step_index),
        validation_id,
        interface_id,
        validation_name: row.get(2)?,
        interface_name: row.get(3)?,
        step_index,
        step_name: row.get(5)?,
        intent_id: row.get(6)?,
        intent_name: row.get(7)?,
        notes: row.get(8)?,
        created_at: row.get(9)?,
    })
}

/// Shared RELATES_TO column projection + endpoint filter, used by both the
/// pooled-connection reader (`reads::get_relates_to_between`) and the
/// in-transaction reader (`get_relates_to_between_tx`) so the projection and
/// the row mapping below stay in lockstep.
pub(crate) const RELATES_TO_SELECT_BETWEEN: &str =
    "SELECT e.from_id, e.to_id, src.name, dst.name, e.inspection_status, e.criterion,
            e.confidence, e.evidence, e.last_inspected, e.inspected_by,
            e.priority_score, e.notes, e.kinds, e.stable
     FROM relates_to e
     JOIN intent src ON src.id = e.from_id
     JOIN intent dst ON dst.id = e.to_id
     WHERE e.from_id = ?1 AND e.to_id = ?2";

/// Idempotent RELATES_TO creation guarded by endpoint existence (and self-edge
/// rejection). One source of truth for the pooled and in-transaction creators.
pub(crate) const RELATES_TO_UPSERT: &str = "INSERT OR IGNORE INTO relates_to(
        from_id, to_id, inspection_status, criterion, confidence, evidence,
        last_inspected, inspected_by, priority_score, notes, created_at
     )
     SELECT ?1, ?2, 'uninspected', '', 0, '', '', '', 0, '', ?3
     WHERE ?1 <> ?2
       AND EXISTS(SELECT 1 FROM intent WHERE id = ?1)
       AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)";

/// Map a `RELATES_TO_SELECT_BETWEEN` row (or any row with the same 14-column
/// projection) into a `RelatesTo`. Discovery fields are reader-populated later.
pub(crate) fn map_relates_to_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RelatesTo> {
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
}

/// Read one RELATES_TO edge by endpoints on any connection (pooled or a
/// borrowed transaction — `Transaction` derefs to `Connection`).
pub(crate) fn get_relates_to_between_conn(
    conn: &Connection,
    from_id: &str,
    to_id: &str,
) -> Result<Option<RelatesTo>> {
    conn.query_row(
        RELATES_TO_SELECT_BETWEEN,
        params![from_id, to_id],
        map_relates_to_row,
    )
    .optional()
    .map_err(Into::into)
}

/// Create-if-absent then read back one RELATES_TO edge on any connection,
/// failing with a named error when an endpoint intent is missing.
pub(crate) fn get_or_create_relates_to_conn(
    conn: &Connection,
    from_id: &str,
    to_id: &str,
    now: &str,
) -> Result<RelatesTo> {
    conn.execute(RELATES_TO_UPSERT, params![from_id, to_id, now])?;
    get_relates_to_between_conn(conn, from_id, to_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot create edge: one or both intents not found.\n\
             intent-a id: {from_id}\n\
             intent-b id: {to_id}\n\
             Run `loom intent list` to see available intents."
        )
    })
}

/// Stale a passing/independent edge to `needs_reverification` on any
/// connection. One per edge table because the natural-key columns differ; the
/// shared sentinel keeps the staling contract identical across the standalone
/// `flag_*` API and the ripple/retire cascades.
pub(crate) fn stale_relates_to(
    conn: &Connection,
    from_id: &str,
    to_id: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE relates_to SET inspection_status = 'needs_reverification' WHERE from_id = ?1 AND to_id = ?2",
        params![from_id, to_id],
    )
}
pub(crate) fn stale_targets(
    conn: &Connection,
    hypothesis_id: &str,
    intent_id: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE targets SET inspection_status = 'needs_reverification' WHERE hypothesis_id = ?1 AND intent_id = ?2",
        params![hypothesis_id, intent_id],
    )
}
pub(crate) fn stale_serves(
    conn: &Connection,
    persona_id: &str,
    intent_id: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE serves SET inspection_status = 'needs_reverification' WHERE persona_id = ?1 AND intent_id = ?2",
        params![persona_id, intent_id],
    )
}
pub(crate) fn stale_governs(
    conn: &Connection,
    rule_id: &str,
    intent_id: &str,
) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE governs SET inspection_status = 'needs_reverification' WHERE rule_id = ?1 AND intent_id = ?2",
        params![rule_id, intent_id],
    )
}

/// Read every (from, to) endpoint pair for one edge table on a connection.
/// Shared by `collect_edge_ids` and `edge_id_exists` so the dynamic edge-pair
/// projection is written once.
pub(crate) fn edge_pairs(conn: &Connection, spec: &EdgeSpec) -> Result<Vec<(String, String)>> {
    let table = checked_sql_ident(spec.table)?;
    let from_col = checked_sql_ident(spec.from_col)?;
    let to_col = checked_sql_ident(spec.to_col)?;
    let sql = format!("SELECT {from_col}, {to_col} FROM {table}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn configure_connection(conn: &Connection, persistent: bool) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    if persistent {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(())
}

/// Acquire the cross-process exclusive WRITE lock on `lock_path`
/// (`.loom/graph.lock`). loom serializes writers: at most one process holds an
/// open write transaction against a given graph at a time (WAL still lets
/// readers run concurrently). Retries for a few seconds — matching the
/// connection `busy_timeout` — then fails with a NAMED, actionable error
/// instead of a raw OS/rusqlite "database is locked".
/// How long a writer waits for the cross-process write lock before failing with
/// the named error. `LOOM_LOCK_DEADLINE_MS` tunes it (an ops knob, and lets
/// tests fail fast); defaults to 5000 to match the connection busy_timeout.
fn lock_deadline_ms() -> u64 {
    std::env::var("LOOM_LOCK_DEADLINE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000)
}

fn acquire_write_lock(lock_path: &Path, deadline_ms: u64) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .with_context(|| format!("opening graph write-lock file {}", lock_path.display()))?;
    const STEP_MS: u64 = 50;
    let mut waited = 0u64;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if waited >= deadline_ms {
                    anyhow::bail!(
                        "graph write lock is held by another loom session ({}). loom serializes \
                         writers — only one write session runs at a time. Wait for the other \
                         lane/command to finish, then retry; never force it.",
                        lock_path.display()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(STEP_MS));
                waited += STEP_MS;
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context("locking graph write-lock file"));
            }
        }
    }
}

fn count_table(conn: &Connection, table: &str) -> Result<usize> {
    let table = checked_sql_ident(table)?;
    let sql = format!("SELECT count(*) FROM {table}");
    Ok(conn.query_row(&sql, [], |row| row.get::<_, i64>(0))? as usize)
}

fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let table = checked_sql_ident(table)?;
    let column = checked_sql_ident(column)?;
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in rows {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn clear_all(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    for table in [
        "calls",
        "journeys",
        "serves",
        "targets",
        "validates",
        "governs",
        "implements",
        "hierarchy",
        "relates_to",
        "persona",
        "interface_surface",
        "vocab_term",
        "hypothesis",
        "delegation",
        "ignore_rule",
        "inbox_item",
        "note",
        "validation",
        "quality_rule",
        "codefile",
        "intent",
        "meta",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    Ok(())
}

fn insert_node(
    tx: &rusqlite::Transaction<'_>,
    spec: NodeSpec,
    obj: &Map<String, JsonValue>,
) -> Result<()> {
    let cols = spec.props.join(", ");
    let placeholders = (1..=spec.props.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({cols}) VALUES ({placeholders})",
        spec.table
    );
    let values = spec
        .props
        .iter()
        .map(|p| text_value(obj.get(*p), spec.list_props.contains(p)))
        .collect::<Result<Vec<_>>>()?;
    tx.execute(&sql, params_from_iter(values.iter()))?;
    Ok(())
}

fn insert_edge(
    tx: &rusqlite::Transaction<'_>,
    spec: EdgeSpec,
    obj: &Map<String, JsonValue>,
) -> Result<()> {
    let mut cols = vec![spec.from_col, spec.to_col];
    cols.extend_from_slice(spec.props);
    let placeholders = (1..=cols.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({placeholders})",
        spec.table,
        cols.join(", ")
    );
    let mut values = vec![
        SqlValue::Text(required_str(obj, "from", spec.edge_type)?.to_string()),
        SqlValue::Text(required_str(obj, "to", spec.edge_type)?.to_string()),
    ];
    for p in spec.props {
        if spec.numeric_props.contains(p) {
            values.push(SqlValue::Real(number_value(obj.get(*p))));
        } else if spec.list_props.contains(p) {
            values.push(text_value(obj.get(*p), true)?);
        } else {
            values.push(SqlValue::Text(string_value(obj.get(*p))));
        }
    }
    tx.execute(&sql, params_from_iter(values.iter()))?;
    Ok(())
}

fn export_nodes(conn: &Connection, spec: NodeSpec) -> Result<Vec<JsonValue>> {
    let table = checked_sql_ident(spec.table)?;
    let columns = checked_sql_ident_list(spec.props)?;
    let sql = format!("SELECT {columns} FROM {table} ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let mut obj = Map::new();
        for (idx, prop_name) in spec.props.iter().enumerate() {
            let raw: String = row.get(idx)?;
            let value = if spec.list_props.contains(prop_name) {
                parse_json_array_sql(&raw)?
            } else {
                JsonValue::String(raw)
            };
            obj.insert((*prop_name).to_string(), value);
        }
        Ok(JsonValue::Object(obj))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn export_edges(conn: &Connection, spec: EdgeSpec) -> Result<Vec<JsonValue>> {
    let table = checked_sql_ident(spec.table)?;
    let from_col = checked_sql_ident(spec.from_col)?;
    let to_col = checked_sql_ident(spec.to_col)?;
    let mut select = vec![from_col, to_col];
    for prop in spec.props {
        select.push(checked_sql_ident(prop)?);
    }
    let columns = select.join(", ");
    let sql = format!("SELECT {columns} FROM {table} ORDER BY {from_col}, {to_col}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        let mut obj = Map::new();
        let from: String = row.get(0)?;
        let to: String = row.get(1)?;
        obj.insert("from".to_string(), JsonValue::String(from));
        obj.insert("to".to_string(), JsonValue::String(to));
        for (idx, prop_name) in spec.props.iter().enumerate() {
            let col = idx + 2;
            let value = if spec.numeric_props.contains(prop_name) {
                JsonValue::from(row.get::<_, f64>(col)?)
            } else if spec.list_props.contains(prop_name) {
                parse_json_array_sql(&row.get::<_, String>(col)?)?
            } else {
                JsonValue::String(row.get::<_, String>(col)?)
            };
            obj.insert((*prop_name).to_string(), value);
        }
        Ok(JsonValue::Object(obj))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn object_field<'a>(data: &'a JsonValue, field: &str) -> Result<&'a Map<String, JsonValue>> {
    data.get(field)
        .with_context(|| format!("Export is missing `{field}` object"))?
        .as_object()
        .with_context(|| format!("Export `{field}` is not an object"))
}

fn section_array<'a>(
    obj: &'a Map<String, JsonValue>,
    section: &str,
    key: &str,
) -> Result<&'a Vec<JsonValue>> {
    static EMPTY: std::sync::OnceLock<Vec<JsonValue>> = std::sync::OnceLock::new();
    match obj.get(key) {
        Some(value) => value
            .as_array()
            .with_context(|| format!("Export `{section}.{key}` is not an array")),
        None => Ok(EMPTY.get_or_init(Vec::new)),
    }
}

fn item_object<'a>(item: &'a JsonValue, ctx: &str) -> Result<&'a Map<String, JsonValue>> {
    item.as_object()
        .with_context(|| format!("Export `{ctx}` item is not an object"))
}

fn required_str<'a>(obj: &'a Map<String, JsonValue>, key: &str, ctx: &str) -> Result<&'a str> {
    obj.get(key)
        .with_context(|| format!("Export `{ctx}` is missing string field `{key}`"))?
        .as_str()
        .with_context(|| format!("Export `{ctx}.{key}` is not a string"))
}

fn str_top(data: &JsonValue, key: &str) -> String {
    data.get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_string()
}

fn text_value(value: Option<&JsonValue>, list: bool) -> Result<SqlValue> {
    if list {
        Ok(SqlValue::Text(list_json_text(value)?))
    } else {
        Ok(SqlValue::Text(string_value(value)))
    }
}

fn string_value(value: Option<&JsonValue>) -> String {
    match value {
        Some(JsonValue::String(s)) => s.clone(),
        Some(JsonValue::Number(n)) => n.to_string(),
        Some(JsonValue::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn number_value(value: Option<&JsonValue>) -> f64 {
    value.and_then(JsonValue::as_f64).unwrap_or(0.0)
}

fn list_json_text(value: Option<&JsonValue>) -> Result<String> {
    match value {
        Some(array @ JsonValue::Array(_)) => compact_json(array),
        Some(JsonValue::String(s)) if s.trim().is_empty() => Ok("[]".to_string()),
        Some(JsonValue::String(s)) => match serde_json::from_str::<JsonValue>(s) {
            Ok(JsonValue::Array(_)) => compact_json(&serde_json::from_str::<JsonValue>(s)?),
            _ => Ok(json!([s]).to_string()),
        },
        Some(JsonValue::Null) | None => Ok("[]".to_string()),
        Some(other) => Ok(json!([other]).to_string()),
    }
}

fn compact_json(value: &JsonValue) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn parse_json_array(raw: &str) -> Result<JsonValue> {
    let value: JsonValue = serde_json::from_str(raw).context("parse stored JSON list field")?;
    match value {
        JsonValue::Array(_) => Ok(value),
        other => anyhow::bail!("stored JSON list field is not an array: {other}"),
    }
}

fn parse_json_array_items(raw: &str) -> Result<Vec<JsonValue>> {
    match parse_json_array(raw)? {
        JsonValue::Array(items) => Ok(items),
        _ => unreachable!("parse_json_array enforces array shape"),
    }
}

fn parse_json_array_sql(raw: &str) -> rusqlite::Result<JsonValue> {
    parse_json_array(raw).map_err(|err| rusqlite::Error::ToSqlConversionFailure(err.into()))
}

fn string_list(raw: &str) -> Result<Vec<String>> {
    parse_json_array_items(raw)?
        .into_iter()
        .map(|item| match item {
            JsonValue::String(s) => Ok(s),
            other => anyhow::bail!("stored JSON list item is not a string: {other}"),
        })
        .collect()
}

fn string_list_sql(raw: &str) -> rusqlite::Result<Vec<String>> {
    string_list(raw).map_err(|err| rusqlite::Error::ToSqlConversionFailure(err.into()))
}

fn symbol_facts(raw: &str) -> Result<Vec<SymbolFact>> {
    parse_json_array_items(raw)?
        .into_iter()
        .map(|item| match item {
            JsonValue::String(s) => serde_json::from_str::<SymbolFact>(&s)
                .with_context(|| format!("parse CodeFile.symbol_facts item `{s}`")),
            other => serde_json::from_value::<SymbolFact>(other)
                .context("parse CodeFile.symbol_facts object"),
        })
        .collect()
}

#[cfg(test)]
pub fn normalized_for_semantic_compare(mut value: JsonValue) -> JsonValue {
    // schema_version is metadata that import legitimately upgrades to the active
    // version — it is not part of the SEMANTIC graph, so a round-trip that
    // upgrades an older export is still faithful. Exclude it from the compare.
    if let JsonValue::Object(map) = &mut value {
        map.remove("schema_version");
        if let Some(JsonValue::Object(nodes)) = map.get_mut("nodes") {
            for spec in NODE_SPECS {
                nodes
                    .entry(spec.label.to_string())
                    .or_insert_with(|| JsonValue::Array(Vec::new()));
            }
        }
        if let Some(JsonValue::Object(edges)) = map.get_mut("edges") {
            for spec in EDGE_SPECS {
                edges
                    .entry(spec.edge_type.to_string())
                    .or_insert_with(|| JsonValue::Array(Vec::new()));
            }
        }
    }
    normalize_lists(&mut value);
    value
}

#[cfg(test)]
fn normalize_lists(value: &mut JsonValue) {
    let list_keys: BTreeSet<&str> = [
        prop::SOURCE_REFS,
        prop::TAGS,
        prop::IMPORTS,
        prop::SYMBOLS,
        prop::SYMBOL_FACTS,
        "layer_order",
    ]
    .into_iter()
    .collect();
    match value {
        JsonValue::Object(map) => {
            for (key, v) in map.iter_mut() {
                if list_keys.contains(key.as_str()) {
                    match v {
                        JsonValue::String(s) if s.trim().is_empty() => *v = json!([]),
                        JsonValue::String(s) => {
                            if let Ok(JsonValue::Array(a)) = serde_json::from_str::<JsonValue>(s) {
                                *v = JsonValue::Array(a);
                            }
                        }
                        _ => normalize_lists(v),
                    }
                } else {
                    normalize_lists(v);
                }
            }
        }
        JsonValue::Array(items) => {
            for item in items {
                normalize_lists(item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
