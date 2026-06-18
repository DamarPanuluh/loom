use serde_json::Value;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

static SQLITE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn sqlite_test_lock() -> MutexGuard<'static, ()> {
    SQLITE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

struct ScratchGraph {
    root: PathBuf,
}

impl ScratchGraph {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let root = env::temp_dir().join(format!("loom-{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root).expect("create scratch graph directory");

        let export = Path::new(env!("CARGO_MANIFEST_DIR")).join("loom.graph.json");
        fs::copy(export, root.join("loom.graph.json")).expect("copy committed loom export");

        Self { root }
    }
}

impl Drop for ScratchGraph {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn loom_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_loom"))
}

fn run_json(cwd: &Path, args: &[&str]) -> Value {
    run_json_as(cwd, args, "llm:validator")
}

fn run_json_as(cwd: &Path, args: &[&str], agent: &str) -> Value {
    let output = Command::new(loom_bin())
        .args(args)
        .current_dir(cwd)
        .env("LOOM_AGENT", agent)
        .env_remove("LOOM_GRAPH")
        .env_remove("LOOM_DIAGNOSE_MISSING_BASE")
        .output()
        .unwrap_or_else(|err| panic!("failed to run loom {args:?}: {err}"));

    if !output.status.success() {
        panic!(
            "loom {:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "loom {:?} emitted invalid JSON: {err}\nstdout:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn run_json_failure_as(cwd: &Path, args: &[&str], agent: &str) -> Value {
    let output = Command::new(loom_bin())
        .args(args)
        .current_dir(cwd)
        .env("LOOM_AGENT", agent)
        .env_remove("LOOM_GRAPH")
        .output()
        .unwrap_or_else(|err| panic!("failed to run loom {args:?}: {err}"));

    if output.status.success() {
        panic!(
            "loom {:?} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "loom {:?} emitted invalid JSON after failure: {err}\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn run_text_as(cwd: &Path, args: &[&str], agent: &str) -> String {
    let output = Command::new(loom_bin())
        .args(args)
        .current_dir(cwd)
        .env("LOOM_AGENT", agent)
        .env_remove("LOOM_GRAPH")
        .output()
        .unwrap_or_else(|err| panic!("failed to run loom {args:?}: {err}"));

    if !output.status.success() {
        panic!(
            "loom {:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
            args,
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .unwrap_or_else(|err| panic!("loom {args:?} emitted non-UTF8 stdout: {err}"))
}

fn setup_imported_graph(prefix: &str) -> ScratchGraph {
    let graph = ScratchGraph::new(prefix);
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json(&graph.root, &["import", "loom.graph.json", "--json"]);
    graph
}

fn write_scratch_file(root: &Path, path: &str, contents: &str) {
    let file = root.join(path);
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).expect("create scratch file parent");
    }
    fs::write(file, contents).expect("write scratch file");
}

fn unsigned_jwt(claims: serde_json::Value) -> String {
    use base64::Engine as _;
    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_string(&claims).expect("serialize jwt claims"));
    format!("{header}.{payload}.")
}

fn one_shot_status_server(status: u16) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
    let addr = listener.local_addr().expect("test server addr");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept test request");
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf);
        let body = "{}";
        let reason = match status {
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            _ => "OK",
        };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write test response");
    });
    format!("http://{addr}")
}

fn assert_status_ok(value: &Value) {
    assert!(
        value.is_object(),
        "command returned non-object JSON: {value}"
    );
}

fn delete_interface_inventory(root: &Path) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute("DELETE FROM calls", [])
        .expect("delete scratch calls");
    conn.execute("DELETE FROM interface_surface", [])
        .expect("delete scratch interface surfaces");
}

fn insert_interface_surface(root: &Path, id: &str, name: &str, method: &str, target: &str) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute(
        "INSERT INTO interface_surface(
            id, name, description, surface_kind, method, target, created_at, updated_at
         ) VALUES(?1, ?2, 'scratch interface gap fixture', 'http_endpoint', ?3, ?4, 'now', 'now')",
        rusqlite::params![id, name, method, target],
    )
    .expect("insert scratch interface surface");
}

fn delete_validates_for_validation(root: &Path, validation_id: &str) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute(
        "DELETE FROM validates WHERE validation_id = ?1",
        rusqlite::params![validation_id],
    )
    .expect("delete scratch validates");
}

/// Empty the inbox so a test that asserts absolute intake/triage counts is
/// independent of how many cards the committed loom.graph.json fixture carries
/// (audit cards live in the graph and travel with it).
fn clear_inbox(root: &Path) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute("DELETE FROM inbox_item", [])
        .expect("clear scratch inbox");
}

/// Delete every note and seed `n` routine transition notes on one target, with
/// a low cap — a self-contained fixture for sync's transition-note compaction
/// that does not depend on how many notes the committed graph carries.
fn seed_transition_notes(root: &Path, target_id: &str, n: usize, cap: usize) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute("DELETE FROM note", [])
        .expect("clear scratch notes");
    for i in 0..n {
        conn.execute(
            "INSERT INTO note(id, kind, text, author, target_kind, target_id, created_at, audience)
             VALUES(?1, 'transition', ?2, 'llm', 'edge', ?3, ?4, '')",
            rusqlite::params![
                format!("note-{i}"),
                format!("routine churn {i}: uninspected -> passing"),
                target_id,
                format!("2026-01-01T00:00:{:02}Z", i % 60),
            ],
        )
        .expect("insert scratch transition note");
    }
    conn.execute(
        "UPDATE meta SET transition_cap = ?1 WHERE id = 1",
        rusqlite::params![cap.to_string()],
    )
    .expect("set scratch transition cap");
}

/// The first `n` active intent ids from the imported graph, for building edges.
fn first_n_intent_ids(root: &Path, n: usize) -> Vec<String> {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let mut stmt = conn
        .prepare("SELECT id FROM intent WHERE status != 'deprecated' LIMIT ?1")
        .expect("prepare intent query");
    stmt.query_map([n as i64], |r| r.get(0))
        .expect("query intents")
        .map(|r| r.expect("intent id"))
        .collect()
}

/// Two distinct active intent ids from the imported graph, for building edges.
fn first_two_intent_ids(root: &Path) -> (String, String) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let mut stmt = conn
        .prepare("SELECT id FROM intent WHERE status != 'deprecated' LIMIT 2")
        .expect("prepare intent query");
    let ids: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .expect("query intents")
        .map(|r| r.expect("intent id"))
        .collect();
    (ids[0].clone(), ids[1].clone())
}

/// Insert a SERVES verdict that is `passing` with an empty (vacuous) criterion,
/// so `loom doctor` has a SERVES edge to catch — exercising that the audit
/// covers SERVES like every other inspectable edge type.
fn insert_passing_serves_with_vacuous_criterion(root: &Path) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let intent_id: String = conn
        .query_row("SELECT id FROM intent LIMIT 1", [], |r| r.get(0))
        .expect("at least one intent exists in the imported graph");
    conn.execute(
        "INSERT INTO persona(id, name, description, author, created_at, updated_at)
         VALUES('persona-test', 'Test Persona', 'scratch persona', 'llm', 'now', 'now')",
        [],
    )
    .expect("insert scratch persona");
    conn.execute(
        "INSERT INTO serves(persona_id, intent_id, inspection_status, criterion, confidence,
                            evidence, last_inspected, inspected_by, notes, created_at)
         VALUES('persona-test', ?1, 'passing', '', 0.9, 'present', '2026-01-01T00:00:00Z',
                'llm:analyzer', '', 'now')",
        rusqlite::params![intent_id],
    )
    .expect("insert scratch serves edge");
}

fn force_legacy_inbox_kind_constraint(root: &Path) {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.execute_batch(
        r#"
ALTER TABLE inbox_item RENAME TO inbox_item_old;
CREATE TABLE inbox_item(
  id TEXT PRIMARY KEY,
  raw_text TEXT NOT NULL,
  normalized_claim TEXT NOT NULL DEFAULT '',
  kind TEXT NOT NULL CHECK(kind IN (
    'observation','user_request','feature_proposal','bug_suspicion','refactor_suspicion',
    'missing_intent','missing_validation','missing_story','terminology',
    'rough_edge','external_blocker','question'
  )),
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
    )
    .expect("rewrite scratch inbox table with legacy kind constraint");
}

fn inbox_table_sql(root: &Path) -> String {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'inbox_item'",
        [],
        |row| row.get(0),
    )
    .expect("read inbox table sql")
}

#[test]
fn sqlite_imported_export_read_surface() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-read-surface");

    let status = run_json(&graph.root, &["status", "--json"]);
    assert!(status["total_intents"].as_i64().unwrap_or_default() > 0);

    let doctor = run_json(&graph.root, &["doctor", "--json"]);
    assert_eq!(
        doctor["healthy"], true,
        "doctor should be healthy: {doctor}"
    );

    for args in [
        vec!["report", "--json"],
        vec!["next", "--all", "--json"],
        vec!["find", "sqlite", "--limit", "10", "--json"],
        vec!["door", "sqlite storage", "--limit", "10", "--json"],
        vec!["coverage", "--json"],
        vec!["smells", "--limit", "10", "--json"],
        vec!["export", "-", "--json"],
    ] {
        let value = run_json(&graph.root, &args);
        assert!(value.is_object(), "loom {args:?} returned non-object JSON");
    }
}

#[test]
fn sqlite_migrate_reports_open_time_schema_contract() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-migrate-schema");

    let migrated = run_json(&graph.root, &["migrate", "--json"]);
    assert_eq!(migrated["status"], "ok");
    assert_eq!(migrated["backend"], "sqlite");
    assert_eq!(migrated["migrated"], false);
    assert_eq!(migrated["version"], "9");
    assert_eq!(migrated["current"], true);
    assert_eq!(migrated["expected"], "9");
    assert!(
        migrated["next_step"].is_null(),
        "current graph needs no rebuild: {migrated}"
    );
    assert!(
        migrated["message"]
            .as_str()
            .is_some_and(|message| message.contains("created on open")),
        "migrate should teach the current SQLite schema contract: {migrated}"
    );
}

#[test]
fn sqlite_batch_dry_run_validates_without_writing() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-batch-dryrun");
    let (a, b) = first_two_intent_ids(&graph.root);
    // Start the pair from a clean slate so "no row after" proves no write.
    let db = graph.root.join(".loom").join("graph.sqlite");
    {
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        conn.execute(
            "DELETE FROM relates_to WHERE (from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)",
            rusqlite::params![a, b],
        )
        .expect("clear pair");
    }
    // One valid line + one with a fabricated locator: dry-run still runs every gate.
    let valid = format!(
        "{{\"op\":\"ground\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"these coexist cleanly without coupling\",\
         \"evidence\":\"checked the boundary by hand\",\"confidence\":0.6}}"
    );
    let fabricated = format!(
        "{{\"op\":\"ground\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"these coexist cleanly without coupling\",\
         \"evidence\":\"checked\",\"evidence_locator\":\"src/made_up.rs:1-9\",\"confidence\":0.6}}"
    );
    write_scratch_file(
        &graph.root,
        "scratch/dry.jsonl",
        &format!("{valid}\n{fabricated}"),
    );
    let res = run_json_failure_as(
        &graph.root,
        &["batch", "scratch/dry.jsonl", "--dry-run", "--json"],
        "llm:analyzer",
    );
    assert_eq!(res["dry_run"], true, "dry_run flag must be set: {res}");
    assert_eq!(res["ok"], 1, "the valid line would apply: {res}");
    assert_eq!(
        res["failed"], 1,
        "dry-run still rejects the fabricated locator: {res}"
    );
    assert!(
        res["results"][0]["applied"]
            .as_str()
            .is_some_and(|s| s.contains("[dry-run] would ground")),
        "dry-run reports a would-apply, not an apply: {res}"
    );
    // Nothing was written.
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM relates_to WHERE (from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)",
            rusqlite::params![a, b],
            |r| r.get(0),
        )
        .expect("count pair edges");
    assert_eq!(n, 0, "dry-run must not write any edge");
}

#[test]
fn sqlite_batch_flags_copied_evidence() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-batch-copied");
    let ids = first_n_intent_ids(&graph.root, 4);
    assert!(
        ids.len() >= 4,
        "need 4 intents for the copied-evidence fixture"
    );

    // Three distinct edges, ONE pasted evidence body → flagged (not rejected).
    let copied = "these modules both reference the shared Foo type per the audit fixture";
    let lines: Vec<String> = (1..4)
        .map(|i| {
            format!(
                "{{\"op\":\"ground\",\"a\":\"{}\",\"b\":\"{}\",\
                 \"criterion\":\"these coexist cleanly without coupling\",\
                 \"evidence\":\"{copied}\",\"confidence\":0.6}}",
                ids[0], ids[i]
            )
        })
        .collect();
    write_scratch_file(&graph.root, "scratch/copied.jsonl", &lines.join("\n"));
    let flagged = run_json_as(
        &graph.root,
        &["batch", "scratch/copied.jsonl", "--json"],
        "llm:analyzer",
    );
    assert_eq!(
        flagged["failed"], 0,
        "copied evidence is FLAGGED, not rejected: {flagged}"
    );
    assert!(
        flagged["warnings_total"].as_i64().unwrap_or(0) >= 1,
        "one evidence body across 3 edges must raise an epistemic warning: {flagged}"
    );

    // Edge-specific evidence on the same edges → no warning (honest batch passes clean).
    let honest: Vec<String> = (1..4)
        .map(|i| {
            format!(
                "{{\"op\":\"ground\",\"a\":\"{}\",\"b\":\"{}\",\
                 \"criterion\":\"these coexist cleanly without coupling\",\
                 \"evidence\":\"edge {i}: a distinct, specific observation about this exact pair\",\"confidence\":0.6}}",
                ids[0], ids[i]
            )
        })
        .collect();
    write_scratch_file(&graph.root, "scratch/honest.jsonl", &honest.join("\n"));
    let clean = run_json_as(
        &graph.root,
        &["batch", "scratch/honest.jsonl", "--json"],
        "llm:analyzer",
    );
    assert_eq!(
        clean["warnings_total"].as_i64().unwrap_or(-1),
        0,
        "edge-specific evidence must NOT be flagged: {clean}"
    );
}

#[test]
fn sqlite_batch_resolves_evidence_locators() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-batch-locator");
    let (a, b) = first_two_intent_ids(&graph.root);
    write_scratch_file(
        &graph.root,
        "scratch/proof.rs",
        "fn one() {}\nfn two() {}\nfn three() {}\n",
    );
    // A locator pointing at a real file:line is accepted.
    let good = format!(
        "{{\"op\":\"ground\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"these intents coexist cleanly without coupling\",\
         \"evidence\":\"verified the boundary holds\",\
         \"evidence_locator\":\"scratch/proof.rs:1-3\",\"confidence\":0.6}}"
    );
    write_scratch_file(&graph.root, "scratch/good.jsonl", &good);
    let ok = run_json_as(
        &graph.root,
        &["batch", "scratch/good.jsonl", "--json"],
        "llm:analyzer",
    );
    assert_eq!(
        ok["failed"], 0,
        "a real evidence_locator must be accepted: {ok}"
    );

    // A fabricated anchor is rejected — it cannot be laundered into a verdict.
    let bad = format!(
        "{{\"op\":\"ground\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"these intents coexist cleanly without coupling\",\
         \"evidence\":\"verified the boundary holds\",\
         \"evidence_locator\":\"src/totally_made_up.rs:1-9\",\"confidence\":0.6}}"
    );
    write_scratch_file(&graph.root, "scratch/bad.jsonl", &bad);
    let res = run_json_failure_as(
        &graph.root,
        &["batch", "scratch/bad.jsonl", "--json"],
        "llm:analyzer",
    );
    assert_eq!(
        res["failed"], 1,
        "a fabricated evidence_locator must be rejected: {res}"
    );
}

#[test]
fn sqlite_fix_take_withholds_ground_template_from_failing_edges() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-fix-take");
    // Clear pre-existing stale edges so the fix queue contains only the failing
    // edge we create — keeps the test deterministic regardless of how many
    // needs_reverification edges the committed fixture carries.
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        conn.execute(
            "UPDATE relates_to SET inspection_status='passing' WHERE inspection_status='needs_reverification'",
            [],
        )
        .expect("reset stale relates_to");
    }
    let (a, b) = first_two_intent_ids(&graph.root);
    // Record a FAILING RELATES_TO edge between two intents (analyzer/fixer lane).
    let line = format!(
        "{{\"op\":\"issue\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"these intents must remain decoupled\",\
         \"evidence\":\"a now references b directly per the audit fixture\",\"confidence\":0.9}}"
    );
    write_scratch_file(&graph.root, "scratch/fail.jsonl", &line);
    run_json_as(
        &graph.root,
        &["batch", "scratch/fail.jsonl", "--json"],
        "llm:fixer",
    );

    let take = run_json_as(
        &graph.root,
        &["next", "--mode", "fix", "--take", "50", "--json"],
        "llm:fixer",
    );
    let template: Vec<&str> = take["batch_template"]
        .as_array()
        .expect("batch_template array")
        .iter()
        .filter_map(|l| l.as_str())
        .collect();
    let items: Vec<&Value> = take["groups"]
        .as_array()
        .expect("groups array")
        .iter()
        .flat_map(|g| g["items"].as_array().expect("items array").iter())
        .collect();
    let failing: Vec<&Value> = items
        .iter()
        .copied()
        .filter(|it| it["inspection_status"] == "failing")
        .collect();
    assert!(
        !failing.is_empty(),
        "the failing edge should appear in the fix take: {take}"
    );
    for it in failing {
        assert_eq!(
            it["owner_role"], "fixer",
            "a failing edge is fixer work: {it}"
        );
        let ia = it["a"]["id"].as_str().unwrap_or_default();
        let ib = it["b"]["id"].as_str().unwrap_or_default();
        assert!(
            !template.iter().any(|l| l.contains(ia) && l.contains(ib)),
            "a failing edge must NOT get an op:ground template line (it would invite \
             marking a known-failing edge passing with no code fix): {ia} x {ib}"
        );
    }
}

#[test]
fn sqlite_review_take_drains_low_confidence_in_bulk() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-review-take");
    // Clear the existing review queue so the item we seed is deterministic.
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        conn.execute(
            "UPDATE relates_to SET confidence=0.9 WHERE confidence < 0.7",
            [],
        )
        .expect("bump relates confidence");
        conn.execute(
            "UPDATE governs SET confidence=0.9 WHERE confidence < 0.7",
            [],
        )
        .expect("bump governs confidence");
    }
    let (a, b) = first_two_intent_ids(&graph.root);
    // A low-confidence passing verdict → exactly one review candidate.
    let line = format!(
        "{{\"op\":\"ground\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"these coexist cleanly without coupling\",\"confidence\":0.5}}"
    );
    write_scratch_file(&graph.root, "scratch/lowconf.jsonl", &line);
    run_json_as(
        &graph.root,
        &["batch", "scratch/lowconf.jsonl", "--json"],
        "llm:analyzer",
    );

    let take = run_json(
        &graph.root,
        &["next", "--mode", "review", "--take", "50", "--json"],
    );
    assert_eq!(
        take["status"], "ok",
        "review --take should serve a bulk read: {take}"
    );
    let items = take["items"].as_array().expect("items array");
    assert_eq!(
        items.len(),
        1,
        "exactly the one seeded low-confidence verdict: {take}"
    );
    assert_eq!(items[0]["a"]["id"], serde_json::json!(a));
    assert_eq!(items[0]["owner_role"], "analyzer");
    assert_eq!(items[0]["effort"], "high");
    assert_eq!(
        take["batch_template"].as_array().map(|a| a.len()),
        Some(1),
        "a re-record template line per item: {take}"
    );
    assert_eq!(take["dispatch"]["effort"], "high");
}

fn intent_id_by_name(root: &Path, name: &str) -> String {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.query_row(
        "SELECT id FROM intent WHERE name = ?1",
        rusqlite::params![name],
        |r| r.get(0),
    )
    .expect("intent by name")
}

#[test]
fn sqlite_populate_kinds_backfills_mechanical_tier() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-populate-kinds");
    write_scratch_file(&graph.root, "scratch/shared.rs", "pub fn shared() {}\n");
    run_json_as(
        &graph.root,
        &["codefile", "add", "scratch/shared.rs", "--json"],
        "llm:builder",
    );
    for nm in ["alpha shared owner", "beta shared owner"] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                nm,
                "--description",
                "owns the shared scratch helper for the taxonomy test",
                "--level",
                "feature",
                "--lifecycle",
                "implemented",
                "--json",
            ],
            "llm:builder",
        );
        run_json_as(
            &graph.root,
            &[
                "edge",
                "implement",
                nm,
                "scratch/shared.rs",
                "--locator",
                "fn shared",
                "--json",
            ],
            "llm:builder",
        );
    }
    let a = intent_id_by_name(&graph.root, "alpha shared owner");
    let b = intent_id_by_name(&graph.root, "beta shared owner");
    // Ground a relationship between the two co-owners.
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "ground",
            "--criterion",
            "they coexist around the shared helper",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    );
    // Backfill mechanical kinds — both own scratch/shared.rs → shares_file.
    let pop = run_json_as(&graph.root, &["populate", "kinds", "--json"], "llm:builder");
    assert!(
        pop["edges_updated"].as_i64().unwrap_or(0) >= 1,
        "populate kinds should backfill the co-ownership edge: {pop}"
    );
    let db = graph.root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let kinds: String = conn
        .query_row(
            "SELECT kinds FROM relates_to WHERE (from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)",
            rusqlite::params![a, b],
            |r| r.get(0),
        )
        .expect("the grounded edge exists");
    assert!(
        kinds.contains("shares_file"),
        "co-owners of one file must get the shares_file kind: {kinds}"
    );
}

#[test]
fn sqlite_taxonomy_kinds_persist_and_doctor_validates() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-taxonomy");
    let db = graph.root.join(".loom").join("graph.sqlite");
    {
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        // A valid relationship kind round-trips (JSON list column).
        conn.execute(
            "UPDATE relates_to SET kinds='[\"imports\",\"shares_file\"]' \
             WHERE rowid=(SELECT rowid FROM relates_to LIMIT 1)",
            [],
        )
        .expect("set valid kinds");
        // An invalid relationship kind must be caught by doctor.
        conn.execute(
            "UPDATE relates_to SET kinds='[\"bogus_kind\"]' \
             WHERE rowid=(SELECT rowid FROM relates_to LIMIT 1 OFFSET 1)",
            [],
        )
        .expect("set bogus kind");
        // An invalid governs category too.
        conn.execute(
            "UPDATE quality_rule SET kind='not_a_category' \
             WHERE rowid=(SELECT rowid FROM quality_rule LIMIT 1)",
            [],
        )
        .expect("set bogus rule kind");
    }
    let doctor = run_json_failure_as(&graph.root, &["doctor", "--json"], "llm:validator");
    assert_eq!(
        doctor["healthy"], false,
        "bogus kinds make the graph unhealthy: {doctor}"
    );
    let issues = doctor["issues"].to_string();
    assert!(
        issues.contains("unknown kind 'bogus_kind'"),
        "doctor must flag an unknown relation kind: {issues}"
    );
    assert!(
        issues.contains("unknown kind 'not_a_category'"),
        "doctor must flag an unknown governs kind: {issues}"
    );
    // The valid kinds did NOT produce an issue.
    assert!(
        !issues.contains("'imports'") && !issues.contains("'shares_file'"),
        "valid kinds must not be flagged: {issues}"
    );
}

#[test]
fn sqlite_doctor_audits_serves_edges() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-doctor-serves");
    insert_passing_serves_with_vacuous_criterion(&graph.root);
    // doctor exits non-zero when unhealthy but still prints its JSON report.
    let doctor = run_json_failure_as(&graph.root, &["doctor", "--json"], "llm:validator");
    assert_eq!(
        doctor["healthy"], false,
        "a vacuous SERVES verdict is an issue: {doctor}"
    );
    assert!(
        doctor["issues"]
            .as_array()
            .is_some_and(|issues| issues.iter().any(|i| {
                let s = i.as_str().unwrap_or("");
                s.contains("SERVES") && s.contains("criterion")
            })),
        "doctor must audit SERVES verdicts like every inspectable edge: {}",
        doctor["issues"]
    );
}

#[test]
fn sqlite_sync_ignores_mtime_only_change() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-sync-mtime");
    // A registered file that actually exists in the scratch root.
    write_scratch_file(
        &graph.root,
        "scratch/widget.rs",
        "fn widget() -> u8 { 1 }\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "scratch/widget.rs", "--json"],
        "llm:builder",
    );
    // First sync stamps content_hash + last_modified for the new file.
    run_json(&graph.root, &["sync", "--json"]);
    // Rewrite byte-identical content — bumps filesystem mtime, content unchanged.
    write_scratch_file(
        &graph.root,
        "scratch/widget.rs",
        "fn widget() -> u8 { 1 }\n",
    );
    let resync = run_json(&graph.root, &["sync", "--json"]);
    assert_eq!(
        resync["files_changed"], 0,
        "content_hash is the authority — an mtime-only change must not count as changed: {resync}"
    );
    assert!(
        resync["changes"]
            .as_array()
            .is_none_or(|a| a.iter().all(|c| c != "scratch/widget.rs")),
        "a byte-identical file must not drift the graph: {resync}"
    );
}

#[test]
fn sqlite_sync_flags_edges_of_missing_files() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-sync-missing");
    // The scratch root holds only loom.graph.json, so every registered source
    // file is missing on disk: sync must flag their grounded edges, not leave
    // an intent reading green while its code is gone.
    let sync = run_json(&graph.root, &["sync", "--json"]);
    assert!(
        sync["missing_files_total"].as_i64().unwrap_or(0) > 0,
        "expected registered files missing on disk: {sync}"
    );
    assert!(
        sync["relates_to_edges_flagged"].as_i64().unwrap_or(0) > 0,
        "a missing grounded file must flag its RELATES_TO edges to needs_reverification: {sync}"
    );
}

#[test]
fn sqlite_sync_compacts_transition_notes() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-sync-compact");
    // 30 routine transition notes on one target, cap 5 → sync must drop 25.
    seed_transition_notes(&graph.root, "rt:compact-test:compact-test", 30, 5);
    let sync = run_json(&graph.root, &["sync", "--json"]);
    assert!(
        sync["transitions_compacted"].as_i64().unwrap_or(0) >= 25,
        "sync must enforce the transition cap it advertises (drop routine churn beyond cap): {sync}"
    );
    // The cap holds: a second sync has nothing left to compact on that target.
    let again = run_json(&graph.root, &["sync", "--json"]);
    assert!(
        again["transitions_compacted"].as_i64().unwrap_or(99) < 25,
        "the cap holds — a second sync should not re-compact the same target: {again}"
    );
}

#[test]
fn sqlite_audit_summary_surfaces_stay_bounded() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-audit-summary");

    let smells = run_json(&graph.root, &["smells", "--summary", "--json"]);
    assert_eq!(smells["summary"], true);
    assert!(
        smells.get("open_by_kind").is_some(),
        "summary keeps smell counts by kind: {smells}"
    );
    assert!(
        smells.get("smells").is_none(),
        "summary must not dump full smell evidence bodies: {smells}"
    );
    assert!(
        smells.get("adjudicated").is_none(),
        "summary must not dump adjudication bodies: {smells}"
    );

    let coverage = run_json(&graph.root, &["coverage", "--summary", "--json"]);
    assert_eq!(coverage["summary"], true);
    assert!(
        coverage.get("symbol_accountability").is_some(),
        "summary keeps actionable coverage counts: {coverage}"
    );
    assert!(
        coverage.get("raw_actionable_symbol_gaps").is_none(),
        "summary must not dump raw symbol-gap archives: {coverage}"
    );
    assert!(
        coverage.get("adjudicated_symbol_gaps").is_none(),
        "summary must not dump adjudicated symbol-gap archives: {coverage}"
    );
}

#[test]
fn sqlite_primary_mutation_surface_on_fresh_graph() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-mutation-surface");
    run_json(&graph.root, &["init", ".", "--json"]);

    let parent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "checkout flow",
            "--description",
            "customer can submit a cart and receive an order confirmation",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
        "llm:builder",
    );
    let child = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "checkout validation",
            "--description",
            "checkout rejects invalid carts before creating an order",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    let parent_id = parent["id"].as_str().expect("parent id");
    let child_id = child["id"].as_str().expect("child id");

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "edge",
            "hierarchy",
            parent_id,
            child_id,
            "--notes",
            "checkout validation is a child behavior of checkout flow",
            "--json",
        ],
        "llm:builder",
    ));

    write_scratch_file(
        &graph.root,
        "src/checkout.rs",
        "pub fn validate_checkout() -> bool {\n    true\n}\n",
    );
    assert_status_ok(&run_json_as(
        &graph.root,
        &["codefile", "add", "src/checkout.rs", "--json"],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            child_id,
            "src/checkout.rs",
            "--locator",
            "fn validate_checkout",
            "--notes",
            "scratch implementation for SQLite regression coverage",
            "--json",
        ],
        "llm:builder",
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            parent_id,
            child_id,
            "ground",
            "--criterion",
            "checkout validation directly constrains checkout flow behavior",
            "--evidence",
            "the validation child blocks invalid carts before order creation",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    ));

    assert_status_ok(&run_json(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "checkout validation smoke",
            "--type",
            "test",
            "--command",
            "true",
            "--intent",
            child_id,
            "--json",
        ],
    ));
    assert_status_ok(&run_json(
        &graph.root,
        &[
            "validation",
            "mark",
            "checkout validation smoke",
            "--result",
            "passed",
            "--evidence",
            "scratch command returns success",
            "--json",
        ],
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "rule",
            "add",
            "--name",
            "checkout validation rule",
            "--description",
            "checkout behavior has a validation before it is considered complete",
            "--severity",
            "warning",
            "--json",
        ],
        "llm:quality",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "rule",
            "apply",
            "checkout validation rule",
            child_id,
            "--criterion",
            "child checkout behavior has an attached passing validation",
            "--json",
        ],
        "llm:quality",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "rule",
            "verdict",
            "checkout validation rule",
            child_id,
            "--status",
            "passing",
            "--criterion",
            "child checkout behavior has an attached passing validation",
            "--evidence",
            "checkout validation smoke is marked passed",
            "--json",
        ],
        "llm:quality",
    ));

    let hypothesis = run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "add",
            "--name",
            "checkout validation split",
            "--claim",
            "checkout validation may need its own module as rules grow",
            "--proposal",
            "split validation helpers from the checkout flow orchestration",
            "--predicted-outcome",
            "validation helpers become independently testable",
            "--target",
            child_id,
            "--json",
        ],
        "llm:builder",
    );
    assert_status_ok(&hypothesis);
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "prove",
            "checkout validation split",
            "--verdict",
            "refuted",
            "--evidence",
            "scratch fixture is intentionally small and does not justify a split",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "ignore",
            "add",
            "target/sqlite-regression/**",
            "--reason",
            "scratch regression output",
            "--json",
        ],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "delegate",
            "add",
            "target/delegated/**",
            "--to",
            "target/delegated/loom.graph.json",
            "--json",
        ],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &["delegate", "remove", "target/delegated/**", "--json"],
        "llm:builder",
    ));

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "persona",
            "add",
            "--name",
            "checkout-operator",
            "--description",
            "operator verifying checkout behavior in a scratch regression graph",
            "--json",
        ],
        "llm:builder",
    ));
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "persona",
            "serve",
            "checkout-operator",
            child_id,
            "ground",
            "--criterion",
            "operator needs the validation behavior to trust checkout changes",
            "--evidence",
            "the scratch validation smoke is the operator's regression proof",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    ));

    let status = run_json(&graph.root, &["status", "--json"]);
    assert!(status["total_intents"].as_i64().unwrap_or_default() >= 2);

    let export = run_json(&graph.root, &["export", "-", "--json"]);
    assert!(export["nodes"]["Intent"].as_array().unwrap().len() >= 2);
}

#[test]
fn sqlite_populate_backfills_interface_calls_from_existing_sagas() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-populate-interfaces");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "cart creation",
            "--description",
            "customer can create a cart through the HTTP checkout API",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    let intent_id = intent["id"].as_str().expect("intent id");
    write_scratch_file(
        &graph.root,
        "journeys/cart.yaml",
        &format!(
            "saga: cart-flow\nsteps:\n  - name: create cart\n    intent: {intent_id}\n    request:\n      method: POST\n      url: /carts\n    expect:\n      status: 201\n"
        ),
    );
    assert_status_ok(&run_json_as(
        &graph.root,
        &["saga", "add", "journeys/cart.yaml", "--json"],
        "llm:builder",
    ));

    let initial = run_json(&graph.root, &["interface", "list", "--json"]);
    assert_eq!(initial["total"], 1);
    delete_interface_inventory(&graph.root);

    let pending = run_json_as(
        &graph.root,
        &["next", "--mode", "populate", "--json"],
        "llm:builder",
    );
    assert_eq!(pending["mode"], "populate");
    assert_eq!(pending["kind"], "interface_from_sagas");
    assert_eq!(pending["missing_surfaces"], 1);

    let applied = run_json_as(
        &graph.root,
        &["populate", "interfaces", "--from-sagas", "--json"],
        "llm:builder",
    );
    assert_eq!(applied["status"], "ok");
    assert_eq!(applied["interface_surfaces_created"], 1);
    assert_eq!(applied["calls_written"], 1);

    let populated = run_json(&graph.root, &["interface", "list", "--json"]);
    assert_eq!(populated["total"], 1);
    assert_eq!(populated["interfaces"][0]["target"], "/carts");
    assert_eq!(populated["interfaces"][0]["calls"], 1);

    let plan = run_json_as(&graph.root, &["populate", "plan", "--json"], "llm:builder");
    assert_eq!(
        plan["populate"]["interface_from_sagas"]["pending"], false,
        "populate should be idempotent after backfill: {plan}"
    );
}

#[test]
fn sqlite_interface_gaps_detect_boundary_intent_without_calls() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-interface-gap-boundary");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "public cart endpoint",
            "--description",
            "customer can call the public cart endpoint through the service boundary",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    assert_status_ok(&intent);

    let gaps = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(gaps["interface_gaps"]["boundary_intent_without_calls"], 1);
    assert_eq!(gaps["interface_gaps"]["total"], 1);

    let plan = run_json(&graph.root, &["populate", "plan", "--json"]);
    assert_eq!(
        plan["populate"]["interface_gaps"]["boundary_intent_without_calls"],
        gaps["interface_gaps"]["boundary_intent_without_calls"]
    );
}

#[test]
fn sqlite_status_surfaces_populate_gap_lane() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-status-populate");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "status visible endpoint",
            "--description",
            "operator can see that this boundary endpoint still needs interface population",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    assert_status_ok(&intent);

    let status = run_json(&graph.root, &["status", "--json"]);
    assert_eq!(status["other_lanes"]["populate"], 1);
    assert_eq!(status["populate"]["total"], 1);
    assert_eq!(status["populate"]["interface_gaps"], 1);
    assert_eq!(
        status["populate"]["next_command"],
        "loom next --mode populate"
    );

    let human = run_text_as(&graph.root, &["status"], "llm:validator");
    assert!(
        human.contains("populate: 1 gap(s) waiting"),
        "human status should teach the populate gap: {human}"
    );
    assert!(
        human.contains("other open lanes: populate 1"),
        "human status should include populate in other lanes: {human}"
    );
}

#[test]
fn sqlite_saga_diagnose_reports_missing_env_without_stamping() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-saga-diagnose-env");
    write_scratch_file(
        &graph.root,
        "journeys/diagnose-env.yaml",
        r#"
saga: diagnose-env
base: "{{ env.LOOM_DIAGNOSE_MISSING_BASE }}"
steps:
  - name: call target
    intent: saga runner halt-on-failure semantics
    request: { method: GET, url: /health }
    expect: { status: 200 }
"#,
    );

    let diagnosed = run_json_failure_as(
        &graph.root,
        &["saga", "diagnose", "journeys/diagnose-env.yaml", "--json"],
        "llm:validator",
    );
    assert_eq!(diagnosed["status"], "failed");
    assert_eq!(
        diagnosed["diagnosis"]["steps"][0]["root_cause"]["kind"],
        "env_var_missing"
    );
    assert!(diagnosed["diagnosis"]["steps"][0]["root_cause"]["fix"]
        .as_str()
        .unwrap()
        .contains("LOOM_DIAGNOSE_MISSING_BASE=<value> loom saga run diagnose-env"));
}

#[test]
fn sqlite_saga_diagnose_reports_missing_jwt_scope() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-saga-diagnose-scope");
    let base_url = one_shot_status_server(403);
    env::set_var("LOOM_DIAGNOSE_SCOPE_BASE", base_url);
    env::set_var(
        "LOOM_DIAGNOSE_APP_TOKEN",
        unsigned_jwt(serde_json::json!({
            "sub": "app_admin",
            "scope": "signals.emit standing.read"
        })),
    );
    write_scratch_file(
        &graph.root,
        "journeys/diagnose-scope.yaml",
        r#"
saga: diagnose-scope
base: "{{ env.LOOM_DIAGNOSE_SCOPE_BASE }}"
steps:
  - name: write app
    intent: saga runner halt-on-failure semantics
    request:
      method: POST
      url: /apps
      headers:
        Authorization: "Bearer {{ env.LOOM_DIAGNOSE_APP_TOKEN }}"
    auth:
      requires_scopes: [developer.apps.write, standing.read]
    expect: { status: 201 }
"#,
    );

    let diagnosed = run_json_failure_as(
        &graph.root,
        &["saga", "diagnose", "journeys/diagnose-scope.yaml", "--json"],
        "llm:validator",
    );
    assert_eq!(diagnosed["status"], "failed");
    let root = &diagnosed["diagnosis"]["steps"][0]["root_cause"];
    assert_eq!(root["kind"], "token_scope_missing");
    assert_eq!(root["title"], "Token scope missing");
    assert!(root["fields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["name"] == "token_missing" && f["value"] == "developer.apps.write"));
    assert!(root["fix"]
        .as_str()
        .unwrap()
        .contains("developer.apps.write"));
}

#[test]
fn sqlite_inbox_add_normalize_mark_and_export() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-inbox-flow");
    // The committed fixture now carries triaged audit cards; this test asserts
    // absolute intake counts, so start from a known-empty inbox.
    clear_inbox(&graph.root);
    let initial_status = run_json(&graph.root, &["status", "--json"]);
    let initial_required_debt = initial_status["completion"]["required_autonomous_debt"]["total"]
        .as_i64()
        .expect("required autonomous debt total");

    let added = run_json(
        &graph.root,
        &[
            "inbox",
            "add",
            "status debt feels scarier than reality",
            "--source",
            "chat",
            "--json",
        ],
    );
    let id = added["item"]["id"].as_str().expect("inbox id").to_string();
    assert_eq!(added["item"]["status"], "new");

    let status = run_json(&graph.root, &["status", "--json"]);
    assert_eq!(status["intake"]["untriaged"], 1);
    assert_eq!(
        status["completion"]["required_autonomous_debt"]["total"]
            .as_i64()
            .expect("required autonomous debt total"),
        initial_required_debt
    );

    let triage = run_json(&graph.root, &["inbox", "triage", "--take", "5", "--json"]);
    assert_eq!(triage["count"], 1);
    assert_eq!(triage["taken"], 1);
    assert_eq!(triage["queue_total"], 1);
    assert!(triage["normalize_templates"][0]
        .as_str()
        .unwrap()
        .contains(&id));

    let next = run_json(&graph.root, &["next", "--all", "--json"]);
    assert!(next["queues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|queue| queue["queue"] == "inbox" && queue["optional"] == true));

    let normalized = run_json(
        &graph.root,
        &[
            "inbox",
            "normalize",
            &id,
            "--kind",
            "rough_edge",
            "--claim",
            "status should separate required debt from optional enrichment",
            "--route",
            "note",
            "--command",
            "loom note add --kind decision --text \"status taxonomy accepted\"",
            "--json",
        ],
    );
    assert_eq!(normalized["item"]["status"], "triaged");
    assert_eq!(normalized["item"]["route_kind"], "note");

    let proposed = run_json(
        &graph.root,
        &[
            "inbox",
            "add",
            "add saga diagnose so failed HTTP proofs explain root causes",
            "--source",
            "chat",
            "--json",
        ],
    );
    let proposal_id = proposed["item"]["id"]
        .as_str()
        .expect("proposal inbox id")
        .to_string();
    let proposal = run_json(
        &graph.root,
        &[
            "inbox",
            "normalize",
            &proposal_id,
            "--kind",
            "feature_proposal",
            "--claim",
            "saga failures should produce structured diagnosis",
            "--route",
            "intent",
            "--command",
            "loom intent add --name 'saga failure diagnosis' --description 'diagnose failed saga runs' --level feature --lifecycle planned",
            "--json",
        ],
    );
    assert_eq!(proposal["item"]["kind"], "feature_proposal");
    assert_eq!(proposal["item"]["route_kind"], "intent");

    for (kind, route, command) in [
        (
            "decision_capture",
            "note",
            "loom note add --kind decision --text \"use inbox as the single input boundary\"",
        ),
        (
            "constraint",
            "quality_rule",
            "loom rule add --name inbox-boundary --description \"free text enters through inbox\"",
        ),
        (
            "acceptance_criterion",
            "validation",
            "loom validation add --name inbox-vocab-proof --type assertion --command \"cargo test sqlite_inbox\"",
        ),
        (
            "interface_gap",
            "validation",
            "loom saga add journeys/interface-gap.yaml --spawn-missing",
        ),
        (
            "evidence",
            "note",
            "loom note add --kind justification --text \"triage output showed the route menu\"",
        ),
        (
            "risk",
            "hypothesis",
            "loom hypothesis add --name inbox-risk --claim \"intake terms are ambiguous\" --proposal \"expand inbox kind vocabulary\" --predicted-outcome \"routing is clearer\"",
        ),
        (
            "follow_up",
            "intent",
            "loom intent add --name inbox follow-up --description \"handle later inbox work\" --level feature --lifecycle planned",
        ),
        (
            "duplicate_candidate",
            "note",
            "loom note add --kind decision --text \"these inbox cards are duplicates\"",
        ),
        (
            "docs_gap",
            "intent",
            "loom intent mark self-teaching --lifecycle needs_change --reason \"document inbox vocabulary\"",
        ),
        (
            "migration_need",
            "validation",
            "loom validation add --name inbox-check-widening --type assertion --command \"cargo test sqlite_inbox_kind_constraint\"",
        ),
    ] {
        let added = run_json(
            &graph.root,
            &[
                "inbox",
                "add",
                &format!("inbox vocabulary fixture for {kind}"),
                "--source",
                "llm",
                "--json",
            ],
        );
        let id = added["item"]["id"].as_str().expect("new inbox id");
        let normalized = run_json(
            &graph.root,
            &[
                "inbox",
                "normalize",
                id,
                "--kind",
                kind,
                "--claim",
                &format!("{kind} should be accepted as inbox semantic vocabulary"),
                "--route",
                route,
                "--command",
                command,
                "--json",
            ],
        );
        assert_eq!(normalized["item"]["kind"], kind);
        assert_eq!(normalized["item"]["route_kind"], route);
    }

    let marked = run_json(
        &graph.root,
        &[
            "inbox",
            "mark",
            &id,
            "--status",
            "routed",
            "--reason",
            "route command reviewed and no graph mutation was needed for this fixture",
            "--json",
        ],
    );
    assert_eq!(marked["item"]["status"], "routed");

    let exported = run_json(&graph.root, &["export", "-", "--json"]);
    let inbox = exported["nodes"]["InboxItem"].as_array().unwrap();
    assert!(inbox.iter().any(|item| item["id"] == id));
    assert!(inbox.iter().any(|item| item["kind"] == "migration_need"));
}

#[test]
fn sqlite_inbox_kind_constraint_is_widened_on_open() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-inbox-kind-upgrade");
    force_legacy_inbox_kind_constraint(&graph.root);
    assert!(!inbox_table_sql(&graph.root).contains("decision_capture"));

    let added = run_json(
        &graph.root,
        &[
            "inbox",
            "add",
            "we decided to track constraints through inbox",
            "--source",
            "chat",
            "--json",
        ],
    );
    let id = added["item"]["id"].as_str().expect("inbox id");
    let normalized = run_json(
        &graph.root,
        &[
            "inbox",
            "normalize",
            id,
            "--kind",
            "decision_capture",
            "--claim",
            "inbox check constraint should accept expanded semantic vocabulary",
            "--route",
            "note",
            "--command",
            "loom note add --kind decision --text \"expanded inbox vocabulary is accepted\"",
            "--json",
        ],
    );
    assert_eq!(normalized["item"]["kind"], "decision_capture");

    let table_sql = inbox_table_sql(&graph.root);
    assert!(table_sql.contains("decision_capture"));
    assert!(table_sql.contains("migration_need"));
}

#[test]
fn sqlite_door_captures_inbox_item_before_routing() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-door-inbox");

    let door = run_json(
        &graph.root,
        &[
            "door",
            "users need a better intake boundary",
            "--limit",
            "3",
            "--json",
        ],
    );
    let id = door["inbox_item"]["id"]
        .as_str()
        .expect("door inbox id")
        .to_string();
    assert_eq!(door["inbox_item"]["status"], "new");
    assert!(door["next_step"].as_str().unwrap().contains(&id));

    let listed = run_json(&graph.root, &["inbox", "list", "--status", "new", "--json"]);
    assert_eq!(listed["count"], 1);
    assert!(listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == id));
}

#[test]
fn sqlite_interface_gaps_detect_surface_without_calls() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-interface-gap-surface");
    run_json(&graph.root, &["init", ".", "--json"]);
    insert_interface_surface(
        &graph.root,
        "surface-without-calls",
        "GET /health",
        "GET",
        "/health",
    );

    let gaps = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(gaps["interface_gaps"]["surface_without_calls"], 1);
    assert_eq!(gaps["interface_gaps"]["total"], 1);
    assert_eq!(
        gaps["interface_gaps"]["examples"][0]["kind"],
        "surface_without_calls"
    );
}

#[test]
fn sqlite_interface_gaps_detect_call_without_validates() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-interface-gap-validates");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "payment capture endpoint",
            "--description",
            "customer payment can be captured through the HTTP checkout boundary",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    let intent_id = intent["id"].as_str().expect("intent id");
    write_scratch_file(
        &graph.root,
        "journeys/payment.yaml",
        &format!(
            "saga: payment-flow\nsteps:\n  - name: capture payment\n    intent: {intent_id}\n    request:\n      method: POST\n      url: /payments/capture\n    expect:\n      status: 200\n"
        ),
    );
    let saga = run_json_as(
        &graph.root,
        &["saga", "add", "journeys/payment.yaml", "--json"],
        "llm:builder",
    );
    let validation_id = saga["validation_id"].as_str().expect("validation id");
    delete_validates_for_validation(&graph.root, validation_id);

    let gaps = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(gaps["interface_gaps"]["call_without_validates"], 1);
    assert_eq!(gaps["interface_gaps"]["total"], 1);
    assert_eq!(
        gaps["interface_gaps"]["examples"][0]["kind"],
        "call_without_validates"
    );
}

#[test]
fn sqlite_populate_next_prioritizes_deterministic_backfill_before_interface_gaps() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-populate-priority");
    run_json(&graph.root, &["init", ".", "--json"]);

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "cart priority endpoint",
            "--description",
            "customer can create a cart through the HTTP priority endpoint",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--visibility",
            "user_visible",
            "--boundary",
            "inbound",
            "--json",
        ],
        "llm:builder",
    );
    let intent_id = intent["id"].as_str().expect("intent id");
    write_scratch_file(
        &graph.root,
        "journeys/priority.yaml",
        &format!(
            "saga: priority-flow\nsteps:\n  - name: create priority cart\n    intent: {intent_id}\n    request:\n      method: POST\n      url: /priority-carts\n    expect:\n      status: 201\n"
        ),
    );
    assert_status_ok(&run_json_as(
        &graph.root,
        &["saga", "add", "journeys/priority.yaml", "--json"],
        "llm:builder",
    ));
    insert_interface_surface(
        &graph.root,
        "unbound-extra-surface",
        "GET /unbound",
        "GET",
        "/unbound",
    );
    delete_interface_inventory(&graph.root);
    insert_interface_surface(
        &graph.root,
        "unbound-extra-surface",
        "GET /unbound",
        "GET",
        "/unbound",
    );

    let next = run_json_as(
        &graph.root,
        &["next", "--mode", "populate", "--json"],
        "llm:builder",
    );
    assert_eq!(next["kind"], "interface_from_sagas");
    assert_eq!(next["missing_surfaces"], 1);
}
