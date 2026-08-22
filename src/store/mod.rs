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

#![allow(unused_imports)] // child modules glob `use super::*`; re-exports keep those names

use crate::model::*;
use crate::registry;
use crate::{
    Result, CRATE_VERSION, GRAPH_DB, JOURNEY_SCHEMA_CUT, LOOM_DIR, SCHEMA_VERSION,
    WRITER_SCHEMA_KEY, WRITER_VERSION_KEY,
};
use anyhow::{anyhow, bail, Context};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use rusqlite_migration::{Migrations, M};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub use crate::identity::Agent;

/// Marker written only into cloned Journey fixture graphs. Live stores never
/// carry it; compiler-owned proof surgery is confined to these copies.
const LOCAL_SNAPSHOT_MARKER: &str = "local_snapshot";

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
    /// Asserted claims and the anchors behind them. These travel so verification
    /// strength can travel — but strength is RECOMPUTED on import, because a
    /// claim of `verified` from another machine is a claim until this filesystem
    /// agrees.
    pub facts: Vec<crate::evidence::Fact>,
    pub evidence: Vec<crate::evidence::EvidenceRow>,
    pub facets: Vec<Facet>,
    pub tags: Vec<Tag>,
    /// Portable repo config from the meta table — ONLY the allowlisted keys in
    /// [`PORTABLE_META_KEYS`]. Never a blind meta dump: identity keys travel as
    /// top-level export fields, and anything not allowlisted stays local.
    pub config: std::collections::BTreeMap<String, String>,
}

/// What a [`Store::restore`] did with facets/tags whose target node/edge is not
/// present in the imported snapshot. Two disjoint outcomes:
///
/// - **Soft refs** — asserted `adjudication` verdicts on a derived Finding id.
///   The finding re-materializes (same deterministic id) on the next `sync`,
///   so the verdict is a valid dangling reference, not corruption. Always
///   kept, counted in `preserved_soft_refs`.
/// - **True orphans** — any other facet/tag with an absent target. A strict
///   `restore` refuses them; `restore_repairing` drops and reports them here.
#[derive(Debug, Default, Clone)]
pub struct RestoreReport {
    /// Soft-ref facets kept despite an absent target (durable adjudications).
    pub preserved_soft_refs: usize,
    /// Orphan facts dropped by repair, as `(subject_kind, subject_id, claim)`.
    pub dropped_facts: Vec<(String, String, String)>,
    /// Orphan facets dropped by repair, as `(target_kind, target_id, key)`.
    pub dropped_facets: Vec<(String, String, String)>,
    /// Orphan tags dropped by repair, as `(target_kind, target_id, term)`.
    pub dropped_tags: Vec<(String, String, String)>,
}

/// Meta keys that travel with the export. Each is repo-portable configuration
/// (what to track, what to ignore, the layer order, registered scan adapters,
/// structural finding thresholds) — without these an imported graph silently
/// loses its coverage exclusions and detectors. Local-only or identity meta
/// keys must NOT be added here.
pub const PORTABLE_META_KEYS: &[&str] = &[
    "layer_order",
    "ignores",
    "codefile_globs",
    "observed_globs",
    "scan_adapters",
    "thresholds",
    "evidence_policy",
    "upstream_graphs",
];

/// The SQLite-backed graph store. Holds an advisory lock for its lifetime —
/// exclusive for a write open, shared for a read-only open — so writers never
/// overlap and a reader never observes a half-migrated schema.
pub struct Store {
    conn: Connection,
    root: PathBuf,
    identity: std::cell::RefCell<crate::identity::ExecutionIdentity>,
    /// The advisory graph lock. A `RefCell` because the Store-owned guarded
    /// Journey settlement releases it around execution (compiled operations
    /// may spawn child `loom` processes) and re-takes it before any write,
    /// through `&Store` handles held by its callers.
    _lock: std::cell::RefCell<File>,
}

mod adjudications;
mod derived;
#[allow(unused_imports)] // consumed by diagnostics_cmd via crate::store::
pub(crate) use derived::{DebtPromotionInput, DebtPromotionResult};

mod codec;
mod edges;
mod facets;
/// The write boundary: every asserted fact enters through `assert_fact`.
pub mod facts;
mod judgments;
mod lock;
mod nodes;
mod open;
mod schema;
pub use adjudications::HitAdjudication;
pub use edges::Dependent;
pub use facts::{edge_verdict, Assertion, FactView, Subject};
pub use judgments::{JudgmentKind, JudgmentProposal, JudgmentState};

pub use codec::fnv_hex_digest;
pub(crate) use codec::{
    derived_id, id_and_now, is_derived_node_id, now, parse_col, parse_named, row_to_edge,
    row_to_node, DERIVED_TS, EDGE_COLS, NODE_COLS,
};
pub use lock::LOCK_CONTENTION_MARKER;
pub(crate) use lock::{
    acquire_lock, LOCK_WAIT_BUDGET_MS, READ_LOCK_WAIT_BUDGET_MS, SQLITE_BUSY_TIMEOUT_MS,
};
pub(crate) use schema::{
    ahead_schema_error, apply_schema_migrations, configure, configure_read,
    ensure_supported_persisted_schema, schema_migration_requires_consent,
};

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

    #[test]
    fn facet_and_tag_writes_reject_missing_typed_targets() {
        let tmp = TmpRoot::new("loom-store-annotation-targets");
        let store = Store::init(tmp.path(), Some("annotations"), false).unwrap();

        let facet_error = store
            .set_facet(
                "missing-node",
                TargetKind::Node,
                "level",
                "feature",
                TruthClass::Asserted,
            )
            .expect_err("a facet must not create an orphan node reference");
        assert!(facet_error.to_string().contains("no node target"));

        let tag_error = store
            .set_tag("missing-edge", TargetKind::Edge, "reviewed")
            .expect_err("a tag must not create an orphan edge reference");
        assert!(tag_error.to_string().contains("no edge target"));

        let intent = store
            .add_node(
                NodeType::Intent,
                "annotations keep typed targets",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        store
            .set_facet(
                &intent.id,
                TargetKind::Node,
                "level",
                "feature",
                TruthClass::Asserted,
            )
            .unwrap();
        store
            .set_tag(&intent.id, TargetKind::Node, "integrity")
            .unwrap();
    }

    #[test]
    fn grounding_roles_reject_non_implements_edges() {
        let tmp = TmpRoot::new("loom-store-grounding-role-kind");
        let store = Store::init_with_identity(
            tmp.path(),
            Some("grounding-role-kind"),
            false,
            crate::identity::ExecutionIdentity::solo(),
        )
        .unwrap();
        let first = store
            .add_node(
                NodeType::Intent,
                "first behavior",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        let second = store
            .add_node(
                NodeType::Intent,
                "second behavior",
                "",
                "planned",
                serde_json::json!({}),
            )
            .unwrap();
        let relates = store
            .add_edge(
                EdgeKind::Relates,
                &first.id,
                &second.id,
                TruthClass::Asserted,
            )
            .unwrap();

        let error = store
            .set_grounding_role(&relates.id, GroundingRole::Consumes)
            .expect_err("a relationship edge must not accept grounding semantics");
        assert!(error.to_string().contains("only to implements edges"));
        assert!(
            store
                .get_facet(&relates.id, TargetKind::Edge, "role")
                .unwrap()
                .is_none(),
            "rejected role write must leave no facet"
        );
    }

    #[test]
    fn repeated_node_status_preserves_updated_at() {
        let tmp = TmpRoot::new("loom-store-idempotent-node-status");
        let store = Store::init(tmp.path(), Some("idempotent-node-status"), false).unwrap();
        let validation = store
            .add_node(
                NodeType::Validation,
                "repeatable proof",
                "",
                "not_run",
                serde_json::json!({}),
            )
            .unwrap();

        // loom-stability-exempt: in-module test of set_node_status itself
        store.set_node_status(&validation.id, "passed").unwrap();
        store
            .conn
            .execute(
                "UPDATE node SET updated_at='stable-sentinel' WHERE id=?1",
                params![validation.id],
            )
            .unwrap();
        // loom-stability-exempt: in-module test of set_node_status itself
        store.set_node_status(&validation.id, "passed").unwrap();

        assert_eq!(
            store.get_node(&validation.id).unwrap().unwrap().updated_at,
            "stable-sentinel",
            "an unchanged status must not dirty the portable graph"
        );
    }

    #[test]
    fn delete_node_cascades_body_linked_notes_and_their_annotations() {
        let tmp = TmpRoot::new("loom-store-note-cascade");
        let store = Store::init(tmp.path(), Some("note-cascade"), false).unwrap();
        let inbox = store
            .add_node(
                NodeType::InboxItem,
                "journey fixture",
                "",
                "routed",
                serde_json::json!({}),
            )
            .unwrap();
        let decision = store
            .add_note(&inbox.id, "decision", "routed by journey")
            .unwrap();
        let nested = store
            .add_note(&decision.id, "context", "why it was routed")
            .unwrap();
        store
            .set_tag(&decision.id, TargetKind::Node, "fixture")
            .unwrap();

        store.delete_node(&inbox.id).unwrap();

        assert!(store.get_node(&decision.id).unwrap().is_none());
        assert!(store.get_node(&nested.id).unwrap().is_none());
        assert!(store
            .tags_of(&decision.id, TargetKind::Node)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn finding_hash_treats_a_vanished_codefile_as_absence() {
        let tmp = TmpRoot::new("loom-store-vanished-finding-file");
        let store = Store::init(tmp.path(), Some("vanished-file"), false).unwrap();
        let finding = store
            .add_node(
                NodeType::Finding,
                "behavior that outlived its file",
                "impact",
                "code_audit",
                serde_json::json!({
                    "file": "src/gone.rs",
                    "evidence": "src/gone.rs:1",
                    "kind": "code_audit",
                    "source": "llm",
                    "confidence": 0.7,
                }),
            )
            .unwrap();
        assert_eq!(
            store.finding_codefile_hash(&finding.id).unwrap(),
            None,
            "a finding that names an unregistered file is orphaned evidence, not a graph crash"
        );
        assert!(store.finding_owner_intents(&finding.id).unwrap().is_empty());
    }
}
