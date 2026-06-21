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

/// A pre-v10 graph (old 4-value lifecycle CHECK, schema_version=9) must
/// migrate on open: the intent-table rebuild widens the CHECK to admit
/// to_be_removed, PRESERVES intent rows AND their inbound-FK edges (the
/// foreign_keys=OFF rebuild must not cascade-delete them), and the version is
/// stamped 10 only after the migration. This is the in-place v9→v10 upgrade
/// real users hit — the fresh-table tests never exercise it.
#[test]
fn v9_graph_migrates_lifecycle_check_without_losing_edges() {
    let path = std::env::temp_dir().join(format!(
        "loom-v9-migrate-{}-{}.sqlite",
        std::process::id(),
        line!()
    ));
    let _ = std::fs::remove_file(&path);
    // Seed a v9-shaped graph directly: old 4-value lifecycle CHECK, no
    // criterion column, two intents + a hierarchy edge (FK ON DELETE CASCADE).
    {
        let seed = Connection::open(&path).unwrap();
        seed.execute_batch(
                r#"
CREATE TABLE meta(
  id INTEGER PRIMARY KEY CHECK(id = 1),
  schema_version TEXT NOT NULL,
  graph_id TEXT NOT NULL DEFAULT '',
  graph_name TEXT NOT NULL DEFAULT '',
  custody TEXT NOT NULL DEFAULT '' CHECK(custody IN ('owned','observed','')),
  layer_order TEXT NOT NULL DEFAULT '[]'
);
INSERT INTO meta(id, schema_version, graph_id, graph_name, custody, layer_order)
VALUES(1, '9', 'gid', 'g', 'owned', '[]');
CREATE TABLE intent(
  id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL,
  abstraction_level TEXT NOT NULL, domain TEXT NOT NULL DEFAULT '', layer TEXT NOT NULL DEFAULT '',
  source_refs TEXT NOT NULL CHECK(json_valid(source_refs)), status TEXT NOT NULL,
  aspect TEXT NOT NULL DEFAULT '',
  lifecycle TEXT NOT NULL CHECK(lifecycle IN ('planned','implemented','needs_change','deferred')),
  created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  tags TEXT NOT NULL CHECK(json_valid(tags)),
  visibility TEXT NOT NULL DEFAULT '' CHECK(visibility IN ('user_visible','internal','')),
  boundary TEXT NOT NULL DEFAULT '' CHECK(boundary IN ('inbound','outbound',''))
);
CREATE TABLE hierarchy(
  parent_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  child_id TEXT NOT NULL REFERENCES intent(id) ON DELETE CASCADE,
  notes TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT '',
  PRIMARY KEY(parent_id, child_id), CHECK(parent_id <> child_id)
);
INSERT INTO intent(id,name,description,abstraction_level,source_refs,status,lifecycle,created_at,updated_at,tags)
  VALUES('p','parent','d','system','[]','confirmed','implemented','t','t','[]');
INSERT INTO intent(id,name,description,abstraction_level,source_refs,status,lifecycle,created_at,updated_at,tags)
  VALUES('c','child','d','feature','[]','confirmed','implemented','t','t','[]');
INSERT INTO hierarchy(parent_id, child_id) VALUES('p','c');
"#,
            )
            .unwrap();
    }

    // Opening with this binary runs create_schema → the migration.
    let store = SqliteGraphStore::open(&path).unwrap();

    // Intent rows preserved.
    assert_eq!(
        store.list_all_intents().unwrap().len(),
        2,
        "intents survived the rebuild"
    );
    // The inbound-FK edge survived (foreign_keys=OFF rebuild did not cascade).
    assert_eq!(
        store.list_hierarchy_pairs().unwrap().len(),
        1,
        "hierarchy edge must survive the intent-table rebuild (no FK cascade)"
    );
    // Version stamped only after the migration (v11 = v10's data-model
    // expansion + Validation.last_executed_run, the proven-axis discriminator).
    assert_eq!(store.graph_meta().unwrap().unwrap().version, "11");
    // The widened CHECK now admits to_be_removed.
    let check: String = store
        .conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='intent'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        check.contains("to_be_removed"),
        "lifecycle CHECK widened: {check}"
    );
    store
        .conn
        .execute(
            "UPDATE intent SET lifecycle = 'to_be_removed' WHERE id = 'c'",
            [],
        )
        .expect("to_be_removed is now a writable lifecycle");

    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
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

fn assert_find_hits_well_formed(query: &str, hits: &[crate::db::queries::FindHit], total: usize) {
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

fn sqlite_test_intent(id: &str, now: &str) -> Intent {
    Intent {
        id: id.into(),
        name: format!("intent {id}"),
        description: "Test intent for sqlite regression coverage.".into(),
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
    }
}

fn sqlite_test_hypothesis(id: &str, now: &str) -> Hypothesis {
    Hypothesis {
        id: id.into(),
        name: format!("hypothesis {id}"),
        claim: "The target status update should affect every target edge.".into(),
        proposal: "Stamp all TARGETS edges for the hypothesis.".into(),
        predicted_outcome: "The returned row count equals the number of target edges.".into(),
        status: "proposed".into(),
        author: "llm:analyzer".into(),
        evidence: String::new(),
        inspected_by: String::new(),
        last_inspected: String::new(),
        created_at: now.into(),
        updated_at: now.into(),
    }
}

fn sqlite_test_inbox_item(id: &str, now: &str) -> InboxItem {
    InboxItem {
        id: id.into(),
        raw_text: "Capture this imported inbox card.".into(),
        normalized_claim: "Capture this imported inbox card.".into(),
        kind: "observation".into(),
        status: "new".into(),
        source: "user".into(),
        author: "user".into(),
        tags: Vec::new(),
        links: Vec::new(),
        route_kind: String::new(),
        route_command: String::new(),
        route_target_kind: String::new(),
        route_target_id: String::new(),
        resolution: String::new(),
        created_at: now.into(),
        updated_at: now.into(),
    }
}

fn snapshot_signature(snapshot: &crate::db::queries::QuerySnapshot, notes: &[Note]) -> JsonValue {
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

// AUDIT 14541cf3: retire_intent used to set status=deprecated but leave
// the intent's IMPLEMENTS passing, and list_all_implements had no status
// filter — so retired code kept producing undeclared_coupling / tangled_file
// findings keyed by dead UUIDs. Two fixes: retire_intent now stales the
// IMPLEMENTS edge (defensive — un-retiring forces a re-inspection, matching
// the RELATES_TO/SERVES/TARGETS/GOVERNS handling), AND query_snapshot filters
// IMPLEMENTS whose intent_id isn't in the active intents set (so the active
// snapshot is self-consistent and smells never see dangling retired edges).
#[test]
fn sqlite_retire_stales_implements_and_filters_them_from_active_snapshot() {
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
            name: "to be retired".into(),
            description: "Grounding must not stay green after retire.".into(),
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
            symbols: vec!["fn foo".into()],
            symbol_facts: Vec::new(),
            content_hash: "hash-a".into(),
        })
        .unwrap();
    store
        .insert_implements("intent-a", "code-a", "fn foo", "", now)
        .unwrap();

    // Pre-retire: the grounding is passing and visible in the active snapshot.
    assert_eq!(
        store.list_implements_for_intent("intent-a").unwrap()[0].inspection_status,
        "passing"
    );
    let snap_before = store.query_snapshot().unwrap();
    assert!(snap_before
        .implements
        .iter()
        .any(|im| im.intent_id == "intent-a"));

    store
        .retire_intent("intent-a", "superseded", None, "2026-01-02T00:00:00Z")
        .unwrap();

    // 1. retire_intent stales the IMPLEMENTS edge (defensive: un-retiring
    //    forces a re-inspection, matching the other edge types).
    let after = store.list_implements_for_intent("intent-a").unwrap();
    assert_eq!(
        after.len(),
        1,
        "the grounding row is preserved (history kept, not hard-dropped)"
    );
    assert_eq!(
        after[0].inspection_status, "needs_reverification",
        "a passing grounding on now-dead code must not stay green"
    );

    // 2. The active snapshot excludes the retired intent's IMPLEMENTS — no
    //    dangling edges keyed by a dead UUID for smells to fire on.
    let snap_after = store.query_snapshot().unwrap();
    assert!(
        !snap_after.intents.iter().any(|i| i.id == "intent-a"),
        "retired intent is out of the active intents set"
    );
    assert!(
            snap_after.implements.iter().all(|im| im.intent_id != "intent-a"),
            "retired intent's IMPLEMENTS are filtered from the active snapshot (no dead-UUID findings): {:?}",
            snap_after.implements
        );
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
fn sqlite_import_clears_inbox() {
    let now = "2026-01-01T00:00:00Z";
    let mut store = SqliteGraphStore::in_memory().unwrap();
    store
        .initialize(
            crate::db::schema::SCHEMA_VERSION,
            "graph-clear-inbox",
            "test",
            "owned",
            now,
        )
        .unwrap();
    store
        .insert_inbox_item(&sqlite_test_inbox_item("inbox-a", now))
        .unwrap();

    let exported = store.export_json().unwrap();
    assert_eq!(exported["nodes"]["InboxItem"].as_array().unwrap().len(), 1);

    store.import_export_json(&exported).unwrap();

    let items = store.list_inbox_items(None, None).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "inbox-a");
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
    assert!(
        signature["implements"].as_array().unwrap().len()
            <= data["edges"]["IMPLEMENTS"].as_array().unwrap().len(),
        "QuerySnapshot keeps active IMPLEMENTS (a retired intent's groundings \
             are filtered so smells don't fire on dead code) while export carries history"
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
            last_executed_run: "".into(),
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
fn set_targets_status_returns_correct_count() {
    let now = "2026-01-01T00:00:00Z";
    let mut store = SqliteGraphStore::in_memory().unwrap();
    for id in ["intent-a", "intent-b", "intent-c"] {
        store.insert_intent(&sqlite_test_intent(id, now)).unwrap();
    }
    store
        .insert_hypothesis(&sqlite_test_hypothesis("hypothesis-count", now))
        .unwrap();
    for id in ["intent-a", "intent-b", "intent-c"] {
        store.insert_targets("hypothesis-count", id, now).unwrap();
    }

    let changed = store
        .set_targets_status_for_hypothesis(
            "hypothesis-count",
            "passing",
            "proof establishes target impact",
            "verified every target edge was stamped",
            0.91,
            "llm:analyzer",
            now,
        )
        .unwrap();

    assert_eq!(changed, 3);
}

#[test]
fn confirmed_hypothesis_targets_are_settled_not_staled() {
    let now = "2026-01-01T00:00:00Z";
    let mut store = SqliteGraphStore::in_memory().unwrap();
    store.insert_intent(&sqlite_test_intent("intent-a", now)).unwrap();
    store.insert_intent(&sqlite_test_intent("intent-b", now)).unwrap();

    // (A) a confirmed hypothesis's passing TARGETS must NOT be staled by the ripple:
    // prove is the only re-stamper and it is closed once confirmed, so staling would
    // strand the edge in needs_reverification forever.
    let mut h_pass = sqlite_test_hypothesis("hyp-pass", now);
    h_pass.status = "confirmed".into();
    store.insert_hypothesis(&h_pass).unwrap();
    store.insert_targets("hyp-pass", "intent-a", now).unwrap();
    store
        .set_targets_status_for_hypothesis("hyp-pass", "passing", "proof", "verified", 0.9, "llm", now)
        .unwrap();
    let edge = store.list_targets_for_hypothesis("hyp-pass").unwrap().remove(0);
    assert!(
        !store
            .flag_targets_needs_reverification(&edge, "spawned intent code changed", now)
            .unwrap(),
        "a confirmed hypothesis's TARGETS must not stale"
    );
    assert_eq!(
        store.list_targets_for_hypothesis("hyp-pass").unwrap()[0].inspection_status,
        "passing"
    );

    // (B) a confirmed hypothesis's already-stale TARGETS is reconciled back to passing.
    let mut h_stale = sqlite_test_hypothesis("hyp-stale", now);
    h_stale.status = "confirmed".into();
    store.insert_hypothesis(&h_stale).unwrap();
    store.insert_targets("hyp-stale", "intent-b", now).unwrap();
    store
        .set_targets_status_for_hypothesis(
            "hyp-stale",
            "needs_reverification",
            "staled before sync skipped decided hypotheses",
            "x",
            0.9,
            "llm",
            now,
        )
        .unwrap();
    let cleared = store.settle_confirmed_hypothesis_targets().unwrap();
    assert_eq!(cleared, 1, "settle returns the count of stale TARGETS cleared");
    assert_eq!(
        store.list_targets_for_hypothesis("hyp-stale").unwrap()[0].inspection_status,
        "passing"
    );
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
