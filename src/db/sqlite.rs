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

pub struct SqliteGraphStore {
    conn: Connection,
    /// Sibling lock file (`.loom/graph.lock`) backing the cross-process
    /// single-writer guarantee. `None` for the in-memory test store.
    lock_path: Option<PathBuf>,
    /// The held exclusive write lock, acquired lazily on the FIRST write
    /// transaction and kept for the store's lifetime. Read-only commands never
    /// open a write transaction, so they never acquire it — WAL keeps readers
    /// concurrent with the single writer.
    write_lock: Option<std::fs::File>,
}

#[derive(Debug, Clone, Copy)]
struct NodeSpec {
    label: &'static str,
    table: &'static str,
    props: &'static [&'static str],
    list_props: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
struct EdgeSpec {
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
    prop::AUTHOR,
    prop::CREATED_AT,
];

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
        list_props: EMPTY_LIST_PROPS,
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
            write_lock: None,
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
            write_lock: None,
        })
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        configure_connection(&conn, false)?;
        let store = Self {
            conn,
            lock_path: None,
            write_lock: None,
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
        if self.write_lock.is_none() {
            if let Some(lock_path) = self.lock_path.clone() {
                self.write_lock = Some(acquire_write_lock(&lock_path, 5000)?);
            }
        }
        Ok(self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?)
    }

    pub fn import_export_json(&mut self, data: &JsonValue) -> Result<()> {
        if data.get("loom_export").and_then(JsonValue::as_i64) != Some(1) {
            anyhow::bail!("Not a loom export (missing/unknown `loom_export` marker).");
        }

        let tx = self.write_tx()?;
        clear_all(&tx)?;

        let layer_order = compact_json(data.get("layer_order").unwrap_or(&json!([])))?;
        tx.execute(
            "INSERT INTO meta(
                id, schema_version, graph_id, graph_name, custody, created_at,
                last_synced, transition_cap, layer_order
             ) VALUES(1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                // Stamp the ACTIVE schema version, not the export's: the data is
                // normalized into THIS loom's schema on import, so an older
                // export upgrades to the current version instead of carrying a
                // stale one (which would trip the doctor version check).
                crate::db::schema::SCHEMA_VERSION,
                str_top(data, "graph_id"),
                str_top(data, "graph_name"),
                str_top(data, "custody"),
                str_top(data, "created_at"),
                str_top(data, "last_synced"),
                str_top(data, "transition_cap"),
                layer_order,
            ],
        )?;

        let nodes = object_field(data, "nodes")?;
        for spec in NODE_SPECS {
            let items = section_array(nodes, "nodes", spec.label)?;
            for item in items {
                insert_node(&tx, *spec, item_object(item, spec.label)?)?;
            }
        }

        let edges = object_field(data, "edges")?;
        for spec in EDGE_SPECS {
            let items = section_array(edges, "edges", spec.edge_type)?;
            for item in items {
                insert_edge(&tx, *spec, item_object(item, spec.edge_type)?)?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn export_json(&self) -> Result<JsonValue> {
        // All meta fields travel: created_at (graph birth), last_synced and
        // transition_cap were previously dropped on export, so a round-trip reset
        // birth/sync stamps to "" and silently reverted a customized --set-cap to
        // the default (import reads all three via str_top).
        #[allow(clippy::type_complexity)]
        let (
            schema_version,
            graph_id,
            graph_name,
            custody,
            created_at,
            last_synced,
            transition_cap,
            layer_order,
        ): (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
        ) = self
            .conn
            .query_row(
                "SELECT schema_version, graph_id, graph_name, custody, created_at, \
                 last_synced, transition_cap, layer_order FROM meta WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()?
            .unwrap_or_default();

        let mut nodes = Map::new();
        for spec in NODE_SPECS {
            let mut arr = export_nodes(&self.conn, *spec)?;
            if spec.label == label::NOTE {
                // Export-time note retention: routine `transition` breadcrumbs are
                // local audit churn that dominates the artifact (97% of nodes) and
                // makes every commit churn ~20k diff lines. Full history stays in
                // .loom/graph.sqlite; the portable, diffable artifact carries only
                // the durable notes (decision/justification/confirm/todo/idea/…).
                arr.retain(|n| n.get("kind").and_then(JsonValue::as_str) != Some("transition"));
            }
            nodes.insert(spec.label.to_string(), JsonValue::Array(arr));
        }

        let mut edges = Map::new();
        for spec in EDGE_SPECS {
            edges.insert(
                spec.edge_type.to_string(),
                JsonValue::Array(export_edges(&self.conn, *spec)?),
            );
        }

        Ok(json!({
            "loom_export": 1,
            "schema_version": schema_version,
            "graph_id": graph_id,
            "graph_name": graph_name,
            "custody": custody,
            "created_at": created_at,
            "last_synced": last_synced,
            "transition_cap": transition_cap,
            "layer_order": parse_json_array(&layer_order)?,
            "nodes": nodes,
            "edges": edges,
        }))
    }

    pub fn counts(&self) -> Result<(usize, usize)> {
        let mut nodes = 0usize;
        for spec in NODE_SPECS {
            nodes += count_table(&self.conn, spec.table)?;
        }
        let mut edges = 0usize;
        for spec in EDGE_SPECS {
            edges += count_table(&self.conn, spec.table)?;
        }
        Ok((nodes, edges))
    }

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

    pub fn find_intents(&self, query: &str, limit: usize) -> Result<(Vec<FindHit>, usize)> {
        let intents = self.list_active_intents()?;
        let hierarchy = self.list_hierarchy_pairs()?;
        rank_intents_from_parts(
            &intents,
            &hierarchy,
            |intent_id| self.groundings_for_intent(intent_id),
            |intent_id| self.stale_edge_count(intent_id),
            query,
            limit,
        )
    }

    pub fn door_matches(&self, query: &str, limit: usize) -> Result<DoorMatches> {
        Ok(door_matches_from_planes(
            self.list_vocab_terms()?,
            self.list_validations()?,
            self.list_rules()?,
            query,
            limit,
        ))
    }

    pub fn query_snapshot(&self) -> Result<QuerySnapshot> {
        Ok(QuerySnapshot::from_parts(
            self.list_active_intents()?,
            self.list_hierarchy_pairs()?,
            self.list_relates_to()?,
            self.list_all_governs()?,
            self.list_rules()?,
            self.list_all_validates()?,
            self.list_validations()?,
            self.list_all_implements()?,
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

    pub fn committed_export_stale(&self, root: &Path) -> Result<Option<bool>> {
        let path = root.join("loom.graph.json");
        if !path.exists() {
            return Ok(None);
        }
        let live = serde_json::to_string_pretty(&self.export_json()?)?;
        Ok(Some(
            std::fs::read_to_string(&path).ok().as_deref() != Some(live.as_str()),
        ))
    }

    fn list_active_intents(&self) -> Result<Vec<Intent>> {
        self.list_intents_matching(true)
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

    fn list_all_intents(&self) -> Result<Vec<Intent>> {
        self.list_intents_matching(false)
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

    pub fn set_intent_tags(&self, id: &str, tags: Vec<String>, updated_at: &str) -> Result<bool> {
        let encoded = crate::db::queries::vocab::encode_tags(tags)?;
        let changed = self.conn.execute(
            "UPDATE intent SET tags = ?1, updated_at = ?2 WHERE id = ?3",
            params![serde_json::to_string(&encoded)?, updated_at, id],
        )?;
        Ok(changed > 0)
    }

    pub fn add_source_ref(
        &self,
        id: &str,
        path: &str,
        updated_at: &str,
    ) -> Result<Option<Vec<String>>> {
        let Some(intent) = self.get_intent(id)? else {
            return Ok(None);
        };
        let mut refs = intent.source_refs;
        if !refs.iter().any(|source_ref| source_ref == path) {
            refs.push(path.to_string());
            self.set_source_refs(id, &refs, updated_at)?;
        }
        Ok(Some(refs))
    }

    pub fn remove_source_ref(
        &self,
        id: &str,
        path: &str,
        updated_at: &str,
    ) -> Result<Option<bool>> {
        let Some(intent) = self.get_intent(id)? else {
            return Ok(None);
        };
        let mut refs = intent.source_refs;
        let before = refs.len();
        refs.retain(|source_ref| source_ref != path);
        if refs.len() == before {
            return Ok(Some(false));
        }
        self.set_source_refs(id, &refs, updated_at)?;
        Ok(Some(true))
    }

    fn set_source_refs(&self, id: &str, refs: &[String], updated_at: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE intent SET source_refs = ?1, updated_at = ?2 WHERE id = ?3",
            params![serde_json::to_string(refs)?, updated_at, id],
        )?;
        Ok(())
    }

    pub fn initialize(
        &self,
        schema_version: &str,
        graph_id: &str,
        graph_name: &str,
        custody: &str,
        created_at: &str,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO meta(
                id, schema_version, graph_id, graph_name, custody, created_at,
                last_synced, transition_cap, layer_order
             ) VALUES(1, ?1, ?2, ?3, ?4, ?5, '', '', '[]')",
            params![schema_version, graph_id, graph_name, custody, created_at],
        )?;
        Ok(changed > 0)
    }

    pub fn set_identity(&self, graph_id: &str, graph_name: &str, custody: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meta SET graph_id = ?1, graph_name = ?2, custody = ?3 WHERE id = 1",
            params![graph_id, graph_name, custody],
        )?;
        Ok(())
    }

    pub fn insert_intent(&self, intent: &Intent) -> Result<()> {
        self.conn.execute(
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
            tx.execute(
                "UPDATE intent SET visibility = ?1, updated_at = ?2 WHERE id = ?3",
                params![visibility, now, id],
            )?;
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
        let changed = self.conn.execute(
            "UPDATE intent SET visibility = ?1, updated_at = ?2 WHERE id = ?3",
            params![visibility, updated_at, id],
        )?;
        Ok(changed > 0)
    }

    pub fn set_intent_layer(&self, id: &str, layer: &str, updated_at: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE intent SET layer = ?1, updated_at = ?2 WHERE id = ?3",
            params![layer, updated_at, id],
        )?;
        Ok(changed > 0)
    }

    /// Set the intent's first-class falsifiable criterion (v10). The caller
    /// records the prior value in a decision note (the version chain).
    pub fn set_intent_criterion(
        &self,
        id: &str,
        criterion: &str,
        updated_at: &str,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE intent SET criterion = ?1, updated_at = ?2 WHERE id = ?3",
            params![criterion, updated_at, id],
        )?;
        Ok(changed > 0)
    }

    pub fn set_intent_boundary(&self, id: &str, boundary: &str, updated_at: &str) -> Result<bool> {
        if !matches!(boundary, "" | "inbound" | "outbound") {
            anyhow::bail!("Invalid boundary '{boundary}'. Valid: inbound | outbound | \"\".");
        }
        let changed = self.conn.execute(
            "UPDATE intent SET boundary = ?1, updated_at = ?2 WHERE id = ?3",
            params![boundary, updated_at, id],
        )?;
        Ok(changed > 0)
    }

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
                self.conn.execute(
                    "UPDATE intent SET name = ?1, description = ?2, updated_at = ?3 WHERE id = ?4",
                    params![name, description, updated_at, id],
                )?;
            }
            (Some(name), None) => {
                self.conn.execute(
                    "UPDATE intent SET name = ?1, updated_at = ?2 WHERE id = ?3",
                    params![name, updated_at, id],
                )?;
            }
            (None, Some(description)) => {
                self.conn.execute(
                    "UPDATE intent SET description = ?1, updated_at = ?2 WHERE id = ?3",
                    params![description, updated_at, id],
                )?;
            }
            (None, None) => {
                self.conn.execute(
                    "UPDATE intent SET updated_at = ?1 WHERE id = ?2",
                    params![updated_at, id],
                )?;
            }
        }
        Ok(true)
    }

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
                tx.execute(
                    "UPDATE relates_to
                     SET inspection_status = 'needs_reverification'
                     WHERE from_id = ?1 AND to_id = ?2",
                    params![edge.from_id, edge.to_id],
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
                ripple.relates_to_flagged += 1;
            }
        }

        for edge in governs {
            if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
                tx.execute(
                    "UPDATE governs
                     SET inspection_status = 'needs_reverification'
                     WHERE rule_id = ?1 AND intent_id = ?2",
                    params![edge.rule_id, edge.intent_id],
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
                ripple.governs_flagged += 1;
            }
        }

        for edge in targets {
            if edge.intent_id == intent_id && edge.inspection_status == "passing" {
                tx.execute(
                    "UPDATE targets
                     SET inspection_status = 'needs_reverification'
                     WHERE hypothesis_id = ?1 AND intent_id = ?2",
                    params![edge.hypothesis_id, edge.intent_id],
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
                ripple.targets_flagged += 1;
            }
        }

        for edge in implements {
            if edge.inspection_status == "passing" {
                tx.execute(
                    "UPDATE implements
                     SET inspection_status = 'needs_reverification'
                     WHERE intent_id = ?1 AND codefile_id = ?2",
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
                    "SELECT last_result FROM validation WHERE id = ?1",
                    params![edge.validation_id],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(result) = result {
                if result != "not_run" && result != "blocked" && !result.is_empty() {
                    tx.execute(
                        "UPDATE validation SET last_result = 'not_run' WHERE id = ?1",
                        params![edge.validation_id],
                    )?;
                    ripple.validations_invalidated += 1;
                }
            }
        }

        tx.commit()?;
        Ok(ripple)
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
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE intent SET status = 'deprecated', updated_at = ?1 WHERE id = ?2",
            params![now, id],
        )?;
        insert_transition_note_tx(&tx, "intent", id, &prev.status, "deprecated", "loom", now)?;
        let cause = format!("intent '{}' retired", prev.name);
        for edge in relates {
            if edge.inspection_status == "passing" || edge.inspection_status == "independent" {
                tx.execute(
                    "UPDATE relates_to
                     SET inspection_status = 'needs_reverification'
                     WHERE from_id = ?1 AND to_id = ?2",
                    params![edge.from_id, edge.to_id],
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
        }
        for edge in &serves {
            tx.execute(
                "UPDATE serves SET inspection_status = 'needs_reverification'
                 WHERE persona_id = ?1 AND intent_id = ?2",
                params![edge.persona_id, edge.intent_id],
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
        for edge in &targets {
            tx.execute(
                "UPDATE targets SET inspection_status = 'needs_reverification'
                 WHERE hypothesis_id = ?1 AND intent_id = ?2",
                params![edge.hypothesis_id, edge.intent_id],
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
        for edge in &governs {
            tx.execute(
                "UPDATE governs SET inspection_status = 'needs_reverification'
                 WHERE rule_id = ?1 AND intent_id = ?2",
                params![edge.rule_id, edge.intent_id],
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

    pub fn delete_intent(&mut self, id: &str) -> Result<bool> {
        let exists = self.get_intent(id)?.is_some();
        if !exists {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        tx.execute("DELETE FROM note WHERE target_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM note
             WHERE target_kind = 'edge'
               AND instr(target_id, ?1) > 0",
            params![id],
        )?;
        tx.execute("DELETE FROM intent WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(true)
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
            let table = checked_sql_ident(spec.table)?;
            let from_col = checked_sql_ident(spec.from_col)?;
            let to_col = checked_sql_ident(spec.to_col)?;
            let sql = format!("SELECT {from_col}, {to_col} FROM {table}");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (from, to) = row?;
                ids.insert(crate::db::schema::edge_key(spec.edge_type, &from, &to));
            }
        }
        Ok(ids)
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

    pub fn insert_implements(
        &self,
        intent_id: &str,
        codefile_id: &str,
        locator: &str,
        notes: &str,
        now: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
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
        let changed = self.conn.execute(
            "DELETE FROM implements WHERE intent_id = ?1 AND codefile_id = ?2",
            params![intent_id, codefile_id],
        )?;
        Ok(changed > 0)
    }

    pub fn validations_for_intent(&self, intent_id: &str) -> Result<Vec<Validation>> {
        let mut stmt = self.conn.prepare(
            "SELECT v.id, v.name, v.description, v.validation_type, v.command,
                    v.last_run, v.last_result
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

    pub fn insert_ignore(&self, ignore: &Ignore) -> Result<()> {
        self.conn.execute(
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

    pub fn list_delegations(&self) -> Result<Vec<Delegation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, pattern, target, author, created_at
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
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn insert_delegation(&self, delegation: &Delegation) -> Result<()> {
        self.conn.execute(
            "INSERT INTO delegation(id, pattern, target, author, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![
                delegation.id,
                delegation.pattern,
                delegation.target,
                delegation.author,
                delegation.created_at
            ],
        )?;
        Ok(())
    }

    pub fn delete_delegation(&self, pattern: &str) -> Result<Option<Delegation>> {
        let existing = self
            .list_delegations()?
            .into_iter()
            .find(|delegation| delegation.pattern == pattern);
        if existing.is_none() {
            return Ok(None);
        }
        self.conn.execute(
            "DELETE FROM delegation WHERE pattern = ?1",
            params![pattern],
        )?;
        Ok(existing)
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

        self.conn.execute(
            "INSERT INTO hierarchy(parent_id, child_id, notes, created_at)
             VALUES(?1, ?2, ?3, ?4)",
            params![parent_id, child_id, notes, now],
        )?;
        Ok(())
    }

    fn hierarchy_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT parent_id, child_id FROM hierarchy")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
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
            "SELECT id, path, language, last_modified, imports, symbols, symbol_facts, content_hash
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
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn insert_codefile(&self, codefile: &CodeFile) -> Result<()> {
        self.conn.execute(
            "INSERT INTO codefile(id, path, language, last_modified, imports, symbols, symbol_facts, content_hash)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                codefile.id,
                codefile.path,
                codefile.language,
                codefile.last_modified,
                serde_json::to_string(&codefile.imports)?,
                serde_json::to_string(&codefile.symbols)?,
                serde_json::to_string(&codefile.symbol_facts)?,
                codefile.content_hash
            ],
        )?;
        Ok(())
    }

    pub fn delete_codefile(&mut self, key: &str) -> Result<Option<CodeFile>> {
        let Some(codefile) = self
            .list_codefiles()?
            .into_iter()
            .find(|codefile| codefile.id == key || codefile.path == key)
        else {
            return Ok(None);
        };
        let tx = self.write_tx()?;
        tx.execute(
            "DELETE FROM note
             WHERE target_kind = 'edge'
               AND instr(target_id, ?1) > 0",
            params![codefile.id],
        )?;
        tx.execute("DELETE FROM codefile WHERE id = ?1", params![codefile.id])?;
        tx.commit()?;
        Ok(Some(codefile))
    }

    pub fn update_codefile_hash(&self, id: &str, hash: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE codefile SET content_hash = ?1 WHERE id = ?2",
            params![hash, id],
        )?;
        Ok(())
    }

    pub fn update_codefile_hash_and_mtime(&self, id: &str, hash: &str, mtime: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE codefile SET content_hash = ?1, last_modified = ?2 WHERE id = ?3",
            params![hash, mtime, id],
        )?;
        Ok(())
    }

    pub fn update_codefile_imports(&self, id: &str, imports: &[String]) -> Result<()> {
        self.conn.execute(
            "UPDATE codefile SET imports = ?1 WHERE id = ?2",
            params![serde_json::to_string(imports)?, id],
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
        self.conn.execute(
            "UPDATE relates_to SET kinds = ?1 WHERE from_id = ?2 AND to_id = ?3",
            params![serde_json::to_string(kinds)?, from_id, to_id],
        )?;
        Ok(())
    }

    pub fn update_codefile_symbols(&self, id: &str, symbols: &[String]) -> Result<()> {
        self.conn.execute(
            "UPDATE codefile SET symbols = ?1 WHERE id = ?2",
            params![serde_json::to_string(symbols)?, id],
        )?;
        Ok(())
    }

    pub fn update_codefile_symbol_facts(&self, id: &str, facts: &[SymbolFact]) -> Result<()> {
        self.conn.execute(
            "UPDATE codefile SET symbol_facts = ?1 WHERE id = ?2",
            params![serde_json::to_string(facts)?, id],
        )?;
        Ok(())
    }

    fn list_hierarchy_pairs(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT parent_id, child_id FROM hierarchy ORDER BY parent_id, child_id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
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

    pub fn set_transition_cap(&self, cap: usize) -> Result<()> {
        self.conn.execute(
            "UPDATE meta SET transition_cap = ?1 WHERE id = 1",
            params![cap.to_string()],
        )?;
        Ok(())
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

    pub fn set_layer_order(&self, order: &[String]) -> Result<Vec<String>> {
        let previous = self.layer_order()?;
        let order_json = serde_json::to_string(order)?;
        self.conn.execute(
            "UPDATE meta SET layer_order = ?1 WHERE id = 1",
            params![order_json],
        )?;
        Ok(previous)
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

    pub fn get_relates_to_between(&self, from_id: &str, to_id: &str) -> Result<Option<RelatesTo>> {
        self.conn
            .query_row(
                "SELECT e.from_id, e.to_id, src.name, dst.name, e.inspection_status, e.criterion,
                        e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                        e.priority_score, e.notes, e.kinds, e.stable
                 FROM relates_to e
                 JOIN intent src ON src.id = e.from_id
                 JOIN intent dst ON dst.id = e.to_id
                 WHERE e.from_id = ?1 AND e.to_id = ?2",
                params![from_id, to_id],
                |row| {
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
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn get_or_create_relates_to(
        &self,
        from_id: &str,
        to_id: &str,
        now: &str,
    ) -> Result<RelatesTo> {
        self.conn.execute(
            "INSERT OR IGNORE INTO relates_to(
                from_id, to_id, inspection_status, criterion, confidence, evidence,
                last_inspected, inspected_by, priority_score, notes, created_at
             )
             SELECT ?1, ?2, 'uninspected', '', 0, '', '', '', 0, '', ?3
             WHERE ?1 <> ?2
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)",
            params![from_id, to_id, now],
        )?;
        match self.get_relates_to_between(from_id, to_id)? {
            Some(edge) => Ok(edge),
            None => anyhow::bail!(
                "Cannot create edge: one or both intents not found.\n\
                 intent-a id: {}\n\
                 intent-b id: {}\n\
                 Run `loom intent list` to see available intents.",
                from_id,
                to_id
            ),
        }
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

    fn get_relates_to_between_tx(
        tx: &rusqlite::Transaction<'_>,
        from_id: &str,
        to_id: &str,
    ) -> Result<Option<RelatesTo>> {
        tx.query_row(
            "SELECT e.from_id, e.to_id, src.name, dst.name, e.inspection_status, e.criterion,
                    e.confidence, e.evidence, e.last_inspected, e.inspected_by,
                    e.priority_score, e.notes, e.kinds, e.stable
             FROM relates_to e
             JOIN intent src ON src.id = e.from_id
             JOIN intent dst ON dst.id = e.to_id
             WHERE e.from_id = ?1 AND e.to_id = ?2",
            params![from_id, to_id],
            |row| {
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
            },
        )
        .optional()
        .map_err(Into::into)
    }

    fn get_or_create_relates_to_tx(
        tx: &rusqlite::Transaction<'_>,
        from_id: &str,
        to_id: &str,
        now: &str,
    ) -> Result<RelatesTo> {
        tx.execute(
            "INSERT OR IGNORE INTO relates_to(
                from_id, to_id, inspection_status, criterion, confidence, evidence,
                last_inspected, inspected_by, priority_score, notes, created_at
             )
             SELECT ?1, ?2, 'uninspected', '', 0, '', '', '', 0, '', ?3
             WHERE ?1 <> ?2
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?1)
               AND EXISTS(SELECT 1 FROM intent WHERE id = ?2)",
            params![from_id, to_id, now],
        )?;
        match Self::get_relates_to_between_tx(tx, from_id, to_id)? {
            Some(edge) => Ok(edge),
            None => anyhow::bail!(
                "Cannot create edge: one or both intents not found.\n\
                 intent-a id: {}\n\
                 intent-b id: {}\n\
                 Run `loom intent list` to see available intents.",
                from_id,
                to_id
            ),
        }
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
        let edge = Self::get_or_create_relates_to_tx(&tx, from_id, to_id, now)?;
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
        let edge = Self::get_or_create_relates_to_tx(&tx, from_id, to_id, now)?;
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
        let edge = Self::get_or_create_relates_to_tx(&tx, from_id, to_id, now)?;
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
        tx.execute(
            "UPDATE relates_to
             SET inspection_status = 'needs_reverification'
             WHERE from_id = ?1 AND to_id = ?2",
            params![edge.from_id, edge.to_id],
        )?;
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

    pub fn insert_validates(
        &self,
        validation_id: &str,
        intent_id: &str,
        notes: &str,
        now: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
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
        let hypotheses = self.list_hypotheses(None)?;
        if hypotheses.iter().any(|hypothesis| hypothesis.id == key) {
            return Ok(key.to_string());
        }
        let kl = key.to_lowercase();
        let exact: Vec<_> = hypotheses
            .iter()
            .filter(|hypothesis| hypothesis.name.to_lowercase() == kl)
            .collect();
        if exact.len() == 1 {
            return Ok(exact[0].id.clone());
        }
        let subs: Vec<_> = hypotheses
            .iter()
            .filter(|hypothesis| hypothesis.name.to_lowercase().contains(&kl))
            .collect();
        match subs.len() {
            1 => Ok(subs[0].id.clone()),
            0 => anyhow::bail!(
                "No hypothesis matches '{}' (by id, name, or fragment). Run `loom hypothesis list`.",
                key
            ),
            _ => anyhow::bail!(
                "'{}' is ambiguous — matches {} hypotheses. Use the id (`loom hypothesis list`).",
                key,
                subs.len()
            ),
        }
    }

    pub fn insert_hypothesis(&self, hypothesis: &Hypothesis) -> Result<()> {
        self.conn.execute(
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

    fn count_hypotheses(&self, status: Option<&str>) -> Result<usize> {
        Ok(self.list_hypotheses(status)?.len())
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

    pub fn insert_persona(&self, persona: &Persona) -> Result<()> {
        self.conn.execute(
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

    pub fn insert_inbox_item(&self, item: &InboxItem) -> Result<()> {
        self.conn.execute(
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

    pub fn update_inbox_item(&self, item: &InboxItem) -> Result<()> {
        self.conn.execute(
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

    pub fn get_or_create_interface_surface(
        &self,
        surface_kind: &str,
        method: &str,
        target: &str,
        description: &str,
        now: &str,
    ) -> Result<InterfaceSurface> {
        let name = interface_surface_name(surface_kind, method, target);
        self.conn.execute(
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

    pub fn insert_call(
        &self,
        validation_id: &str,
        interface_id: &str,
        step_index: usize,
        step_name: &str,
        intent_id: &str,
        now: &str,
    ) -> Result<()> {
        self.conn.execute(
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
        let deleted = self.conn.execute(
            "DELETE FROM calls WHERE validation_id = ?1",
            params![validation_id],
        )?;
        Ok(deleted)
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

    pub fn get_or_create_serves(
        &self,
        persona_id: &str,
        intent_id: &str,
        now: &str,
    ) -> Result<ServesEdge> {
        self.conn.execute(
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

    pub fn get_or_create_journeys(
        &self,
        persona_id: &str,
        validation_id: &str,
        now: &str,
    ) -> Result<JourneysEdge> {
        self.conn.execute(
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

    pub fn insert_targets(&self, hypothesis_id: &str, intent_id: &str, now: &str) -> Result<()> {
        let changed = self.conn.execute(
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
        tx.execute(
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
        let changed = tx.changes() as usize;
        tx.commit()?;
        Ok(changed)
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

    pub fn flag_targets_needs_reverification(
        &mut self,
        edge: &TargetsEdge,
        cause: &str,
        now: &str,
    ) -> Result<bool> {
        if edge.inspection_status != "passing" {
            return Ok(false);
        }
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE targets
             SET inspection_status = 'needs_reverification'
             WHERE hypothesis_id = ?1 AND intent_id = ?2",
            params![edge.hypothesis_id, edge.intent_id],
        )?;
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
        tx.execute(
            "UPDATE serves
             SET inspection_status = 'needs_reverification'
             WHERE persona_id = ?1 AND intent_id = ?2",
            params![edge.persona_id, edge.intent_id],
        )?;
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
        let changed = self.conn.execute(
            "UPDATE implements
             SET inspection_status = 'needs_reverification'
             WHERE intent_id = ?1 AND codefile_id = ?2 AND inspection_status = 'passing'",
            params![intent_id, codefile_id],
        )?;
        Ok(changed > 0)
    }

    pub fn invalidate_validation(&self, validation_id: &str) -> Result<bool> {
        let last_result: Option<String> = self
            .conn
            .query_row(
                "SELECT last_result FROM validation WHERE id = ?1",
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
        self.conn.execute(
            "UPDATE validation SET last_result = 'not_run', last_run = '' WHERE id = ?1",
            params![validation_id],
        )?;
        Ok(true)
    }

    pub fn set_last_synced(&self, now: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meta SET last_synced = ?1 WHERE id = 1",
            params![now],
        )?;
        Ok(())
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

    pub fn insert_vocab_term(&self, term: &VocabTerm) -> Result<()> {
        self.conn.execute(
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

    pub fn merge_vocab_terms(&mut self, from: &str, to: &str, now: &str) -> Result<usize> {
        let tx = self.write_tx()?;
        let from_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM vocab_term WHERE name = ?1)",
            params![from],
            |row| row.get(0),
        )?;
        if !from_exists {
            anyhow::bail!(
                "Term '{from}' is not registered — `loom vocab list` shows the registry."
            );
        }
        let to_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM vocab_term WHERE name = ?1)",
            params![to],
            |row| row.get(0),
        )?;
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
                "UPDATE intent SET tags = ?1, updated_at = ?2 WHERE id = ?3",
                params![serde_json::to_string(&encoded)?, now, intent_id],
            )?;
            retagged += 1;
        }
        tx.execute("DELETE FROM vocab_term WHERE name = ?1", params![from])?;
        tx.commit()?;
        Ok(retagged)
    }

    pub fn list_validations(&self) -> Result<Vec<Validation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, description, validation_type, command, last_run, last_result
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
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn insert_validation(&self, validation: &Validation) -> Result<()> {
        self.conn.execute(
            "INSERT INTO validation(id, name, description, validation_type, command, last_run, last_result)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                validation.id,
                validation.name,
                validation.description,
                validation.validation_type,
                validation.command,
                validation.last_run,
                validation.last_result
            ],
        )?;
        Ok(())
    }

    pub fn mark_validation_result(
        &mut self,
        key: &str,
        last_result: &str,
        edge_status: &str,
        edge_note: &str,
        marker: &str,
        now: &str,
    ) -> Result<(String, usize)> {
        let validation = self.resolve_validation(key)?;
        let tx = self.write_tx()?;
        tx.execute(
            "UPDATE validation SET last_result = ?1, last_run = ?2 WHERE id = ?3",
            params![last_result, now, validation.id],
        )?;
        let intents_updated: i64 = tx.query_row(
            "SELECT count(*) FROM validates WHERE validation_id = ?1",
            params![validation.id],
            |row| row.get(0),
        )?;
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
            tx.execute(
                "UPDATE validation SET last_result = 'not_run', last_run = '' WHERE id = ?1",
                params![validation.id],
            )?;
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM validates WHERE validation_id = ?1",
                params![validation.id],
                |row| row.get(0),
            )?;
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

    /// Delete an InterfaceSurface (CALLS edges cascade via FK). The escape hatch
    /// the `surface_without_calls` gap remedy points at — previously the remedy
    /// ("remove the stale interface surface") was unreachable through loom.
    pub fn delete_interface_surface(&mut self, id: &str) -> Result<bool> {
        let tx = self.write_tx()?;
        let changed = tx.execute("DELETE FROM interface_surface WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(changed > 0)
    }

    /// Delete a Persona (SERVES + JOURNEYS edges cascade via FK).
    pub fn delete_persona(&mut self, id: &str) -> Result<bool> {
        let tx = self.write_tx()?;
        let changed = tx.execute("DELETE FROM persona WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(changed > 0)
    }

    pub fn delete_validation(&mut self, key: &str) -> Result<String> {
        let validation = self.resolve_validation(key)?;
        let tx = self.write_tx()?;
        tx.execute(
            "DELETE FROM note
             WHERE target_kind = 'edge'
               AND instr(target_id, ?1) > 0",
            params![validation.id],
        )?;
        tx.execute(
            "DELETE FROM validation WHERE id = ?1",
            params![validation.id],
        )?;
        tx.commit()?;
        Ok(validation.id)
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

    pub fn insert_rule(&self, rule: &QualityRule) -> Result<()> {
        self.conn.execute(
            "INSERT INTO quality_rule(id, name, description, detection_logic, severity, inspection_effort, kind)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                rule.id,
                rule.name,
                rule.description,
                rule.detection_logic,
                rule.severity,
                rule.inspection_effort,
                rule.kind
            ],
        )?;
        Ok(())
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

    pub fn insert_governs(
        &self,
        rule_id: &str,
        intent_id: &str,
        criterion: &str,
        now: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
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
                 last_inspected = ?6
             WHERE rule_id = ?7 AND intent_id = ?8",
            params![
                status,
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
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
                 last_inspected = ?6
             WHERE rule_id = ?7 AND intent_id = ?8",
            params![
                status,
                criterion,
                evidence,
                confidence,
                inspected_by,
                now,
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
        tx.execute(
            "UPDATE governs
             SET inspection_status = 'needs_reverification'
             WHERE rule_id = ?1 AND intent_id = ?2",
            params![edge.rule_id, edge.intent_id],
        )?;
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

    pub fn insert_note(&self, note: &Note) -> Result<()> {
        self.conn.execute(
            "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                note.id,
                note.kind,
                note.text,
                note.author,
                note.target_kind,
                note.target_id,
                note.created_at,
                note.audience
            ],
        )?;
        Ok(())
    }

    pub fn delete_note_by_id(&self, note_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM note WHERE id = ?1", params![note_id])?;
        Ok(())
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
                if note.text.ends_with("-> failing")
                    || note.text.ends_with("-> needs_change")
                    || note.text.ends_with("→ failing")
                    || note.text.ends_with("→ needs_change")
                {
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
            let table = checked_sql_ident(spec.table)?;
            let from_col = checked_sql_ident(spec.from_col)?;
            let to_col = checked_sql_ident(spec.to_col)?;
            let sql = format!("SELECT {from_col}, {to_col} FROM {table}");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (from_id, to_id) = row?;
                if crate::db::schema::edge_key(spec.edge_type, &from_id, &to_id) == edge_id {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn list_all_notes(&self) -> Result<Vec<Note>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, text, author, target_kind, target_id, audience, created_at
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

fn create_table_batch() -> &'static str {
    r#"
CREATE TABLE IF NOT EXISTS meta(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  schema_version TEXT NOT NULL,
  graph_id TEXT NOT NULL,
  graph_name TEXT NOT NULL,
  custody TEXT NOT NULL CHECK(custody IN ('owned','observed','')),
  created_at TEXT NOT NULL DEFAULT '',
  last_synced TEXT NOT NULL DEFAULT '',
  transition_cap TEXT NOT NULL DEFAULT '',
  layer_order TEXT NOT NULL CHECK(json_valid(layer_order))
);

CREATE TABLE IF NOT EXISTS intent(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL,
  criterion TEXT NOT NULL DEFAULT '',
  abstraction_level TEXT NOT NULL,
  domain TEXT NOT NULL DEFAULT '',
  layer TEXT NOT NULL DEFAULT '',
  source_refs TEXT NOT NULL CHECK(json_valid(source_refs)),
  status TEXT NOT NULL,
  aspect TEXT NOT NULL DEFAULT '',
  lifecycle TEXT NOT NULL CHECK(lifecycle IN ('planned','implemented','needs_change','deferred','to_be_removed')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  visibility TEXT NOT NULL DEFAULT '' CHECK(visibility IN ('user_visible','internal','')),
  boundary TEXT NOT NULL DEFAULT '' CHECK(boundary IN ('inbound','outbound',''))
);

CREATE TABLE IF NOT EXISTS codefile(
  id TEXT PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  language TEXT NOT NULL,
  last_modified TEXT NOT NULL,
  imports TEXT NOT NULL CHECK(json_valid(imports)),
  symbols TEXT NOT NULL CHECK(json_valid(symbols)),
  symbol_facts TEXT NOT NULL CHECK(json_valid(symbol_facts)),
  content_hash TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS quality_rule(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL,
  detection_logic TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT '',
  severity TEXT NOT NULL CHECK(severity IN ('warning','error')),
  inspection_effort TEXT NOT NULL DEFAULT '' CHECK(inspection_effort IN ('low','mid','high',''))
);

CREATE TABLE IF NOT EXISTS validation(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL DEFAULT '',
  validation_type TEXT NOT NULL CHECK(validation_type IN ('test','assertion','benchmark','manual_check','saga')),
  command TEXT NOT NULL DEFAULT '',
  last_run TEXT NOT NULL DEFAULT '',
  last_result TEXT NOT NULL CHECK(last_result IN ('passed','failed','not_run','blocked',''))
);

CREATE TABLE IF NOT EXISTS note(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  text TEXT NOT NULL,
  author TEXT NOT NULL,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  audience TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS ignore_rule(
  id TEXT PRIMARY KEY,
  pattern TEXT NOT NULL UNIQUE,
  reason TEXT NOT NULL,
  author TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS delegation(
  id TEXT PRIMARY KEY,
  pattern TEXT NOT NULL UNIQUE,
  target TEXT NOT NULL,
  author TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS hypothesis(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  claim TEXT NOT NULL,
  proposal TEXT NOT NULL,
  predicted_outcome TEXT NOT NULL,
  status TEXT NOT NULL,
  author TEXT NOT NULL,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS vocab_term(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL,
  author TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS persona(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  description TEXT NOT NULL,
  author TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS interface_surface(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  surface_kind TEXT NOT NULL,
  method TEXT NOT NULL DEFAULT '',
  target TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(surface_kind, method, target)
);

CREATE TABLE IF NOT EXISTS inbox_item(
  id TEXT PRIMARY KEY,
  raw_text TEXT NOT NULL,
  normalized_claim TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL CHECK(kind IN (__INBOX_KIND_SQL_VALUES__)),
  status TEXT NOT NULL CHECK(status IN ('new','triaged','routed','rejected','deferred','duplicate')),
  source TEXT NOT NULL CHECK(source IN ('chat','user','llm','code_audit','validation','import','unknown')),
  author TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  links TEXT NOT NULL CHECK(json_valid(links)),
  route_kind TEXT NOT NULL DEFAULT '' CHECK(route_kind IN (
    'intent','hypothesis','validation','quality_rule','vocab','note','ignore','answer','none',''
  )),
  route_command TEXT NOT NULL DEFAULT '',
  route_target_kind TEXT NOT NULL DEFAULT '',
  route_target_id TEXT NOT NULL DEFAULT '',
  resolution TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS relates_to(
  from_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  to_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  priority_score REAL NOT NULL DEFAULT 0,
  notes TEXT NOT NULL DEFAULT '',
  kinds TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(kinds)),
  stable TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(from_id, to_id),
  CHECK(from_id <> to_id)
);

CREATE TABLE IF NOT EXISTS hierarchy(
  parent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  child_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(parent_id, child_id),
  CHECK(parent_id <> child_id)
);

CREATE TABLE IF NOT EXISTS implements(
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  codefile_id TEXT NOT NULL REFERENCES codefile(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  locator TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(intent_id, codefile_id)
);

CREATE TABLE IF NOT EXISTS governs(
  rule_id TEXT NOT NULL REFERENCES quality_rule(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(rule_id, intent_id)
);

CREATE TABLE IF NOT EXISTS validates(
  validation_id TEXT NOT NULL REFERENCES validation(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','needs_reverification')),
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(validation_id, intent_id)
);

CREATE TABLE IF NOT EXISTS targets(
  hypothesis_id TEXT NOT NULL REFERENCES hypothesis(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(hypothesis_id, intent_id)
);

CREATE TABLE IF NOT EXISTS serves(
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  inspection_status TEXT NOT NULL CHECK(inspection_status IN ('uninspected','passing','failing','independent','needs_reverification')),
  criterion TEXT NOT NULL DEFAULT '',
  confidence REAL NOT NULL DEFAULT 0,
  evidence TEXT NOT NULL DEFAULT '',
  last_inspected TEXT NOT NULL DEFAULT '',
  inspected_by TEXT NOT NULL DEFAULT '',
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(persona_id, intent_id)
);

CREATE TABLE IF NOT EXISTS journeys(
  persona_id TEXT NOT NULL REFERENCES persona(id) ON DELETE CASCADE,
  validation_id TEXT NOT NULL REFERENCES validation(id) ON DELETE CASCADE,
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(persona_id, validation_id)
);

CREATE TABLE IF NOT EXISTS calls(
  validation_id TEXT NOT NULL REFERENCES validation(id) ON DELETE CASCADE,
  interface_id TEXT NOT NULL REFERENCES interface_surface(id) ON DELETE CASCADE,
  step_index TEXT NOT NULL,
  step_name TEXT NOT NULL DEFAULT '',
  intent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  notes TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(validation_id, step_index)
);

CREATE INDEX IF NOT EXISTS idx_intent_lifecycle_status ON intent(lifecycle, status);
CREATE INDEX IF NOT EXISTS idx_intent_name ON intent(name);
CREATE INDEX IF NOT EXISTS idx_codefile_path ON codefile(path);
CREATE INDEX IF NOT EXISTS idx_relates_status_priority ON relates_to(inspection_status, priority_score DESC);
CREATE INDEX IF NOT EXISTS idx_relates_to ON relates_to(to_id);
CREATE INDEX IF NOT EXISTS idx_implements_file ON implements(codefile_id);
CREATE INDEX IF NOT EXISTS idx_implements_status ON implements(inspection_status);
CREATE INDEX IF NOT EXISTS idx_governs_status ON governs(inspection_status);
CREATE INDEX IF NOT EXISTS idx_validates_status ON validates(inspection_status);
CREATE INDEX IF NOT EXISTS idx_targets_status ON targets(inspection_status);
CREATE INDEX IF NOT EXISTS idx_serves_status ON serves(inspection_status);
CREATE INDEX IF NOT EXISTS idx_interface_surface_identity ON interface_surface(surface_kind, method, target);
CREATE INDEX IF NOT EXISTS idx_calls_interface ON calls(interface_id);
CREATE INDEX IF NOT EXISTS idx_inbox_status_kind ON inbox_item(status, kind, created_at);
CREATE INDEX IF NOT EXISTS idx_note_target ON note(target_kind, target_id, kind, created_at);
CREATE INDEX IF NOT EXISTS idx_note_target_only ON note(target_id, created_at);
CREATE INDEX IF NOT EXISTS idx_note_kind ON note(kind, created_at);

CREATE VIRTUAL TABLE IF NOT EXISTS intent_fts USING fts5(
  intent_id UNINDEXED,
  name,
  description,
  domain,
  layer,
  content='',
  tokenize='unicode61'
);
"#
}

impl SqliteGraphStore {
    fn create_schema(&self) -> Result<()> {
        let inbox_kind_values = inbox_kind_sql_values();
        self.conn.execute_batch(
            &create_table_batch().replace("__INBOX_KIND_SQL_VALUES__", &inbox_kind_values),
        )?;
        self.ensure_meta_columns()?;
        self.ensure_taxonomy_columns()?;
        self.ensure_inbox_kind_vocabulary()?;
        // The additive ensure_* migrations above bring an older graph to the
        // current shape; stamp the version so `doctor`/`export` agree with this
        // binary. Opening with a newer loom IS the migration — there is no
        // separate migrate step. No-op when the meta row doesn't exist yet (a
        // fresh `init` inserts it immediately after).
        self.conn.execute(
            "UPDATE meta SET schema_version = ?1 WHERE id = 1",
            params![schema::SCHEMA_VERSION],
        )?;
        Ok(())
    }

    /// Additive taxonomy columns (the edge-kind program): RELATES_TO.kinds (the
    /// relationship multiset, JSON list) and QualityRule.kind (the norm
    /// category). Additive — existing graphs gain them on open with their
    /// defaults, no version bump (loom's convention for additive changes).
    fn ensure_taxonomy_columns(&self) -> Result<()> {
        for (table, column, definition) in [
            ("relates_to", "kinds", "TEXT NOT NULL DEFAULT '[]'"),
            ("relates_to", "stable", "TEXT NOT NULL DEFAULT ''"),
            ("quality_rule", "kind", "TEXT NOT NULL DEFAULT ''"),
            ("intent", "criterion", "TEXT NOT NULL DEFAULT ''"),
        ] {
            if !table_has_column(&self.conn, table, column)? {
                let table = checked_sql_ident(table)?;
                let column = checked_sql_ident(column)?;
                self.conn.execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    fn ensure_meta_columns(&self) -> Result<()> {
        for (column, definition) in [
            ("created_at", "TEXT NOT NULL DEFAULT ''"),
            ("last_synced", "TEXT NOT NULL DEFAULT ''"),
            ("transition_cap", "TEXT NOT NULL DEFAULT ''"),
        ] {
            if !table_has_column(&self.conn, "meta", column)? {
                let column = checked_sql_ident(column)?;
                self.conn.execute(
                    &format!("ALTER TABLE meta ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    fn ensure_inbox_kind_vocabulary(&self) -> Result<()> {
        let create_sql: Option<String> = self
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'inbox_item'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if match create_sql.as_deref() {
            None => true,
            Some(sql) => schema::INBOX_KINDS
                .iter()
                .all(|kind| sql.contains(&sql_string_literal(kind))),
        } {
            return Ok(());
        }

        let rebuild_sql = format!(
            r#"
ALTER TABLE inbox_item RENAME TO inbox_item_old;
CREATE TABLE inbox_item(
  id TEXT PRIMARY KEY,
  raw_text TEXT NOT NULL,
  normalized_claim TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL CHECK(kind IN ({inbox_kind_values})),
  status TEXT NOT NULL CHECK(status IN ('new','triaged','routed','rejected','deferred','duplicate')),
  source TEXT NOT NULL CHECK(source IN ('chat','user','llm','code_audit','validation','import','unknown')),
  author TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  links TEXT NOT NULL CHECK(json_valid(links)),
  route_kind TEXT NOT NULL DEFAULT '' CHECK(route_kind IN (
    'intent','hypothesis','validation','quality_rule','vocab','note','ignore','answer','none',''
  )),
  route_command TEXT NOT NULL DEFAULT '',
  route_target_kind TEXT NOT NULL DEFAULT '',
  route_target_id TEXT NOT NULL DEFAULT '',
  resolution TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO inbox_item(
  id, raw_text, normalized_claim, kind, status, source, author, tags, links,
  route_kind, route_command, route_target_kind, route_target_id, resolution,
  created_at, updated_at
)
SELECT
  id, raw_text, normalized_claim, kind, status, source, author, tags, links,
  route_kind, route_command, route_target_kind, route_target_id, resolution,
  created_at, updated_at
FROM inbox_item_old;
DROP TABLE inbox_item_old;
CREATE INDEX IF NOT EXISTS idx_inbox_status_kind ON inbox_item(status, kind, created_at);
"#,
            inbox_kind_values = inbox_kind_sql_values()
        );
        self.conn.execute_batch(&rebuild_sql)?;
        Ok(())
    }
}

fn inbox_kind_sql_values() -> String {
    schema::INBOX_KINDS
        .iter()
        .map(|kind| sql_string_literal(kind))
        .collect::<Vec<_>>()
        .join(",")
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
mod tests {
    use super::*;
    use serde::Serialize;

    fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("table_info");
        let cols = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect");
        cols
    }

    /// Every column of every NODE/EDGE table must be covered by its props spec
    /// (edge FK from/to columns excepted). A column added to CREATE TABLE but
    /// forgotten in *_PROPS is silently dropped on export and ignored on import,
    /// symmetrically — so `export --check` stays green and doctor can't see it.
    /// This is the mechanical guard for that latent trap.
    #[test]
    fn every_table_column_is_covered_by_its_props_spec() {
        let store = SqliteGraphStore::in_memory().unwrap();
        for spec in NODE_SPECS {
            for col in table_columns(&store.conn, spec.table) {
                assert!(
                    spec.props.contains(&col.as_str()),
                    "node table '{}' column '{}' is missing from its props spec — it would be \
                     silently dropped on export/import",
                    spec.table,
                    col
                );
            }
        }
        for spec in EDGE_SPECS {
            for col in table_columns(&store.conn, spec.table) {
                if col == spec.from_col || col == spec.to_col {
                    continue; // carried as from/to, not a prop
                }
                assert!(
                    spec.props.contains(&col.as_str()),
                    "edge table '{}' column '{}' is missing from its props spec — it would be \
                     silently dropped on export/import",
                    spec.table,
                    col
                );
            }
        }
    }

    #[test]
    fn write_lock_serializes_writers_with_a_named_error() {
        let path = std::env::temp_dir().join(format!(
            "loom-write-lock-{}-{}.lock",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        let held = acquire_write_lock(&path, 100).expect("first writer acquires the lock");
        // A second writer (short deadline) is refused with the NAMED, actionable
        // error — never a raw OS/rusqlite "database is locked".
        let err = acquire_write_lock(&path, 100).expect_err("second writer must be blocked");
        assert!(
            err.to_string()
                .contains("write lock is held by another loom session"),
            "expected the named lock error, got: {err}"
        );
        drop(held);
        // Once released, the next writer acquires it.
        acquire_write_lock(&path, 100).expect("re-acquire after release");
        let _ = std::fs::remove_file(&path);
    }

    fn current_export() -> JsonValue {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("loom.graph.json");
        let raw = std::fs::read_to_string(path).expect("read committed export");
        serde_json::from_str(&raw).expect("parse committed export")
    }

    fn assert_find_hits_well_formed(
        query: &str,
        hits: &[crate::db::queries::FindHit],
        total: usize,
    ) {
        assert!(
            total >= hits.len(),
            "{query}: total matches must not be less than shown hits"
        );
        for (idx, hit) in hits.iter().enumerate() {
            assert!(
                !hit.intent.id.trim().is_empty(),
                "{query}: hit {idx} has no intent id"
            );
            assert!(
                !hit.intent.name.trim().is_empty(),
                "{query}: hit {idx} has no intent name"
            );
            assert!(
                hit.score.is_finite(),
                "{query}: hit {idx} score is not finite"
            );
        }
    }

    fn plane_signature(
        hits: &[crate::db::queries::find::PlaneHit],
    ) -> Vec<(String, String, String, Vec<String>)> {
        hits.iter()
            .map(|h| {
                (
                    h.id.clone(),
                    h.name.clone(),
                    h.detail.clone(),
                    h.matched.clone(),
                )
            })
            .collect()
    }

    fn sorted_json<T: Serialize>(items: &[T]) -> Vec<JsonValue> {
        let mut values = items
            .iter()
            .map(|item| serde_json::to_value(item).unwrap())
            .collect::<Vec<_>>();
        values.sort_by_key(|value| serde_json::to_string(value).unwrap());
        values
    }

    fn sorted_strings(items: &std::collections::HashSet<String>) -> Vec<String> {
        let mut values = items.iter().cloned().collect::<Vec<_>>();
        values.sort();
        values
    }

    fn sorted_degrees(items: &std::collections::HashMap<String, i64>) -> Vec<(String, i64)> {
        let mut values = items
            .iter()
            .map(|(intent, degree)| (intent.clone(), *degree))
            .collect::<Vec<_>>();
        values.sort();
        values
    }

    fn snapshot_signature(
        snapshot: &crate::db::queries::QuerySnapshot,
        notes: &[Note],
    ) -> JsonValue {
        json!({
            "intents": sorted_json(&snapshot.intents),
            "hierarchy": sorted_json(&snapshot.hierarchy),
            "relates": sorted_json(&snapshot.relates),
            "governs": sorted_json(&snapshot.governs),
            "rules": sorted_json(&snapshot.rules),
            "validates": sorted_json(&snapshot.validates),
            "validations": sorted_json(&snapshot.validations),
            "implements": sorted_json(&snapshot.implements),
            "codefiles": sorted_json(&snapshot.codefiles),
            "with_code": sorted_strings(&snapshot.with_code),
            "degrees": sorted_degrees(&snapshot.degrees),
            "notes": sorted_json(notes),
        })
    }

    #[test]
    fn sqlite_implements_regrounding_updates_locator_and_status() {
        let now = "2026-01-01T00:00:00Z";
        let store = SqliteGraphStore::in_memory().unwrap();
        store
            .initialize(
                crate::db::schema::SCHEMA_VERSION,
                "graph-a",
                "test",
                "owned",
                now,
            )
            .unwrap();
        store
            .insert_intent(&Intent {
                id: "intent-a".into(),
                name: "locator update".into(),
                description: "Grounding can move to a better symbol.".into(),
                criterion: String::new(),
                abstraction_level: "feature".into(),
                domain: "".into(),
                layer: "".into(),
                source_refs: Vec::new(),
                status: "proposed".into(),
                aspect: "happy".into(),
                lifecycle: "implemented".into(),
                created_at: now.into(),
                updated_at: now.into(),
                tags: Vec::new(),
                visibility: "internal".into(),
                boundary: "".into(),
            })
            .unwrap();
        store
            .insert_codefile(&CodeFile {
                id: "code-a".into(),
                path: "src/example.rs".into(),
                language: "rust".into(),
                last_modified: now.into(),
                imports: Vec::new(),
                symbols: vec!["better_anchor".into()],
                symbol_facts: Vec::new(),
                content_hash: "hash-a".into(),
            })
            .unwrap();

        store
            .insert_implements("intent-a", "code-a", "old anchor", "old notes", now)
            .unwrap();
        store
            .flag_implements_needs_reverification("intent-a", "code-a")
            .unwrap();
        store
            .insert_implements(
                "intent-a",
                "code-a",
                "better_anchor",
                "updated notes",
                "2026-01-02T00:00:00Z",
            )
            .unwrap();

        let grounding = store.list_implements_for_intent("intent-a").unwrap();
        assert_eq!(grounding.len(), 1);
        assert_eq!(grounding[0].locator, "better_anchor");
        assert_eq!(grounding[0].notes, "updated notes");
        assert_eq!(grounding[0].inspection_status, "passing");
        assert_eq!(grounding[0].created_at, now);
    }

    #[test]
    fn sqlite_schema_contract() {
        let data = current_export();
        let mut store = SqliteGraphStore::in_memory().unwrap();
        store.import_export_json(&data).unwrap();
        let (nodes, edges) = store.counts().unwrap();
        let expected_nodes: usize = data["nodes"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_array().unwrap().len())
            .sum();
        let expected_edges: usize = data["edges"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_array().unwrap().len())
            .sum();
        assert_eq!(nodes, expected_nodes);
        assert_eq!(edges, expected_edges);
    }

    #[test]
    fn sqlite_import_export_round_trip() {
        let data = current_export();
        let mut store = SqliteGraphStore::in_memory().unwrap();
        store.import_export_json(&data).unwrap();
        let exported = store.export_json().unwrap();
        assert_eq!(
            normalized_for_semantic_compare(data),
            normalized_for_semantic_compare(exported)
        );
    }

    #[test]
    fn sqlite_snapshot_matches_imported_export_shape() {
        let data = current_export();
        let mut sqlite = SqliteGraphStore::in_memory().unwrap();
        sqlite.import_export_json(&data).unwrap();

        let sqlite_snapshot = sqlite.query_snapshot().unwrap();
        // query_snapshot is lazy for notes; load them through the shared accessor.
        let sqlite_notes = sqlite_snapshot
            .notes_or_load(|| sqlite.list_all_notes())
            .expect("load notes");
        assert!(
            sqlite_notes
                .windows(2)
                .all(|pair| pair[0].created_at <= pair[1].created_at),
            "SQLite snapshot notes must stay newest-last"
        );

        let signature = snapshot_signature(&sqlite_snapshot, sqlite_notes);
        assert!(!signature["intents"].as_array().unwrap().is_empty());
        assert!(
            signature["intents"].as_array().unwrap().len()
                <= data["nodes"]["Intent"].as_array().unwrap().len(),
            "QuerySnapshot keeps active intents while export also carries history"
        );
        assert_eq!(
            signature["codefiles"].as_array().unwrap().len(),
            data["nodes"]["CodeFile"].as_array().unwrap().len()
        );
        assert_eq!(
            signature["relates"].as_array().unwrap().len(),
            data["edges"]["RELATES_TO"].as_array().unwrap().len()
        );
        assert_eq!(
            signature["implements"].as_array().unwrap().len(),
            data["edges"]["IMPLEMENTS"].as_array().unwrap().len()
        );
        assert_eq!(
            signature["notes"].as_array().unwrap().len(),
            data["nodes"]["Note"].as_array().unwrap().len()
        );
    }

    #[test]
    fn sqlite_list_fields_fail_loudly_on_wrong_item_type() {
        let data = current_export();
        let first_intent = data["nodes"]["Intent"].as_array().unwrap()[0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let mut store = SqliteGraphStore::in_memory().unwrap();
        store.import_export_json(&data).unwrap();

        store
            .conn
            .execute(
                "UPDATE intent SET source_refs = '[1]' WHERE id = ?1",
                params![first_intent],
            )
            .unwrap();

        let err = store.list_intents(None, None).unwrap_err();
        let chain = format!("{:#}", err);
        assert!(
            chain.contains("stored JSON list item is not a string"),
            "unexpected error: {chain}"
        );
        assert!(
            chain.contains("parse source_refs for intent"),
            "unexpected error: {chain}"
        );
    }

    #[test]
    fn sqlite_rejects_dangling_edges_and_duplicate_endpoints() {
        let mut data = current_export();
        data["edges"]["RELATES_TO"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "from": "missing-a",
                "to": "missing-b",
                "inspection_status": "passing",
                "criterion": "dangling edge must fail",
                "confidence": 0.9,
                "evidence": "",
                "last_inspected": "",
                "inspected_by": "",
                "priority_score": 0.0,
                "notes": "",
                "created_at": "",
            }));
        let mut store = SqliteGraphStore::in_memory().unwrap();
        assert!(store.import_export_json(&data).is_err());

        let mut data = current_export();
        let first = data["edges"]["RELATES_TO"].as_array().unwrap()[0].clone();
        data["edges"]["RELATES_TO"]
            .as_array_mut()
            .unwrap()
            .push(first);
        let mut store = SqliteGraphStore::in_memory().unwrap();
        assert!(store.import_export_json(&data).is_err());
    }

    #[test]
    fn sqlite_search_contract() {
        let data = current_export();
        let mut sqlite = SqliteGraphStore::in_memory().unwrap();
        sqlite.import_export_json(&data).unwrap();

        for query in [
            "sqlite storage",
            "global local migration",
            "validation command",
            "quality rule",
            "qwertyuiop zxcvbn",
        ] {
            let (hits, total) = sqlite.find_intents(query, 10).unwrap();
            assert_find_hits_well_formed(query, &hits, total);
        }

        let mut saw_plane_match = false;
        for query in ["sqlite storage", "validation command", "quality rule"] {
            let sqlite = sqlite.door_matches(query, 10).unwrap();
            let _ = plane_signature(&sqlite.vocab);
            let _ = plane_signature(&sqlite.sagas);
            let _ = plane_signature(&sqlite.rules);
            saw_plane_match |=
                !(sqlite.vocab.is_empty() && sqlite.sagas.is_empty() && sqlite.rules.is_empty());
        }
        assert!(
            saw_plane_match,
            "door search must exercise at least one non-intent plane"
        );
    }

    #[test]
    fn sqlite_interface_surface_calls_round_trip() {
        let now = "2026-01-01T00:00:00Z";
        let store = SqliteGraphStore::in_memory().unwrap();
        store
            .initialize(
                crate::db::schema::SCHEMA_VERSION,
                "graph-a",
                "test",
                "owned",
                now,
            )
            .unwrap();
        store
            .insert_intent(&Intent {
                id: "intent-a".into(),
                name: "create cart".into(),
                description: "Creates a cart over HTTP.".into(),
                criterion: String::new(),
                abstraction_level: "feature".into(),
                domain: "checkout".into(),
                layer: "".into(),
                source_refs: Vec::new(),
                status: "proposed".into(),
                aspect: "happy".into(),
                lifecycle: "implemented".into(),
                created_at: now.into(),
                updated_at: now.into(),
                tags: Vec::new(),
                visibility: "user_visible".into(),
                boundary: "inbound".into(),
            })
            .unwrap();
        store
            .insert_validation(&Validation {
                id: "validation-a".into(),
                name: "checkout-flow".into(),
                description: "spec:checkout.yaml".into(),
                validation_type: "saga".into(),
                command: "loom saga run checkout-flow".into(),
                last_run: "".into(),
                last_result: "not_run".into(),
            })
            .unwrap();

        let surface = store
            .get_or_create_interface_surface(
                "http_endpoint",
                "POST",
                "/carts",
                "HTTP endpoint called by saga 'checkout-flow'",
                now,
            )
            .unwrap();
        store
            .insert_call(
                "validation-a",
                &surface.id,
                1,
                "create cart",
                "intent-a",
                now,
            )
            .unwrap();

        let exported = store.export_json().unwrap();
        assert_eq!(
            exported["nodes"]["InterfaceSurface"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(exported["edges"]["CALLS"].as_array().unwrap().len(), 1);

        let mut imported = SqliteGraphStore::in_memory().unwrap();
        imported.import_export_json(&exported).unwrap();
        let calls = imported.list_calls_for_interface(&surface.id).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].validation_name, "checkout-flow");
        assert_eq!(calls[0].interface_name, "POST /carts");
        assert_eq!(calls[0].intent_name, "create cart");
    }

    #[test]
    fn sqlite_targets_status_update_records_confidence() {
        let now = "2026-01-01T00:00:00Z";
        let mut store = SqliteGraphStore::in_memory().unwrap();
        store
            .initialize(
                crate::db::schema::SCHEMA_VERSION,
                "graph-a",
                "test",
                "owned",
                now,
            )
            .unwrap();
        store
            .insert_intent(&Intent {
                id: "intent-a".into(),
                name: "hypothesis lifecycle commands".into(),
                description: "Commands manage hypothesis proof lifecycle.".into(),
                criterion: String::new(),
                abstraction_level: "feature".into(),
                domain: "analysis".into(),
                layer: "".into(),
                source_refs: Vec::new(),
                status: "proposed".into(),
                aspect: "happy".into(),
                lifecycle: "implemented".into(),
                created_at: now.into(),
                updated_at: now.into(),
                tags: Vec::new(),
                visibility: "internal".into(),
                boundary: "inbound".into(),
            })
            .unwrap();
        store
            .insert_hypothesis(&Hypothesis {
                id: "hypothesis-a".into(),
                name: "prove stamps confidence".into(),
                claim: "Prove should write confidence to TARGETS.".into(),
                proposal: "Pass explicit confidence when stamping TARGETS.".into(),
                predicted_outcome: "Doctor sees passing TARGETS as earned.".into(),
                status: "proposed".into(),
                author: "llm:analyzer".into(),
                evidence: String::new(),
                inspected_by: String::new(),
                last_inspected: String::new(),
                created_at: now.into(),
                updated_at: now.into(),
            })
            .unwrap();
        store
            .insert_targets("hypothesis-a", "intent-a", now)
            .unwrap();

        store
            .set_targets_status_for_hypothesis(
                "hypothesis-a",
                "passing",
                "proof establishes target impact",
                "read the proof path and verified confidence is passed through",
                0.87,
                "llm:analyzer",
                now,
            )
            .unwrap();

        let targets = store.list_targets_for_hypothesis("hypothesis-a").unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].inspection_status, "passing");
        assert_eq!(targets[0].confidence, 0.87);
    }

    #[test]
    fn sqlite_wal_concurrency_contract() {
        let root =
            std::env::temp_dir().join(format!("loom-sqlite-wal-{}.sqlite", uuid::Uuid::new_v4()));
        let data = current_export();
        {
            let mut store = SqliteGraphStore::open(&root).unwrap();
            store.import_export_json(&data).unwrap();
        }

        let writer = Connection::open(&root).unwrap();
        configure_connection(&writer, true).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE; UPDATE relates_to SET notes = 'held writer' WHERE rowid = (SELECT rowid FROM relates_to LIMIT 1);").unwrap();

        let reader = Connection::open(&root).unwrap();
        configure_connection(&reader, true).unwrap();
        let count: i64 = reader
            .query_row("SELECT count(*) FROM relates_to", [], |row| row.get(0))
            .unwrap();
        assert!(count > 0);
        let unseen: i64 = reader
            .query_row(
                "SELECT count(*) FROM relates_to WHERE notes = 'held writer'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unseen, 0, "reader sees a stable pre-commit snapshot");

        writer.execute_batch("ROLLBACK;").unwrap();
        let _ = std::fs::remove_file(&root);
        let _ = std::fs::remove_file(root.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(root.with_extension("sqlite-shm"));
    }
}
