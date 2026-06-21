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
    assert_eq!(migrated["version"], "11");
    assert_eq!(migrated["current"], true);
    assert_eq!(migrated["expected"], "11");
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

// audit #17 (lane-bypass): a bare `llm` agent (no LOOM_AGENT role) solo-passes
// every lane, so a multi-agent batch run with a forgotten LOOM_AGENT silently
// records every verdict as unguarded solo. The batch path must FLAG that at
// record time (advisory, never reject — solo batch by one driver is legit).
// Runs the honest edge-specific-evidence fixture (no copied-evidence warning)
// with a bare `llm` so the ONLY warning that can fire is the solo-mode one.
#[test]
fn sqlite_batch_flags_solo_mode_when_no_role_declared() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-batch-solo");
    let ids = first_n_intent_ids(&graph.root, 4);
    assert!(ids.len() >= 4, "need 4 intents for the solo-mode fixture");

    // Edge-specific evidence → no copied-evidence warning. Bare `llm` →
    // session_role() is None → the solo-mode advisory is the only warning.
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
    write_scratch_file(&graph.root, "scratch/solo.jsonl", &honest.join("\n"));
    let flagged = run_json_as(
        &graph.root,
        &["batch", "scratch/solo.jsonl", "--json"],
        "llm", // bare — no role, solo mode
    );
    assert_eq!(flagged["failed"], 0, "solo verdicts apply: {flagged}");
    assert_eq!(
        flagged["warnings_total"].as_i64().unwrap_or(-1),
        1,
        "exactly one warning — the solo-mode advisory (no copied evidence here): {flagged}"
    );
    let warnings = flagged["warnings"].as_array().expect("warnings array");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").starts_with("solo mode:")),
        "the solo-mode advisory must be present: {flagged}"
    );

    // Same fixture, role declared → no solo warning, no copied-evidence
    // warning → clean. Proves the warning is solo-mode-specific, not unconditional.
    let clean = run_json_as(
        &graph.root,
        &["batch", "scratch/solo.jsonl", "--json"],
        "llm:analyzer",
    );
    assert_eq!(
        clean["warnings_total"].as_i64().unwrap_or(-1),
        0,
        "a role-declared batch with edge-specific evidence stays clean: {clean}"
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

// loom-dx #2: the --take N batch template must NOT pre-fill a numeric
// confidence default (the blind re-ground anti-pattern). Confidence is a
// placeholder the batch gate rejects unedited, so a verbatim paste stamps
// zero verdicts, not N false 0.9 grounds. And loom-dx #1+#7 + #8: the template
// header carries a per-op required-fields legend (so `independent→notes` is
// visible before you fail) and surfaces the `--dry-run` guardrail inline.
#[test]
fn sqlite_take_template_confidence_placeholder_and_hints_json() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("take-tmpl-hints-json");
    // discovery --take is a deterministic non-empty template source (the
    // fixture carries thousands of unexplored pairs), and it routes through the
    // same run_take emitter as the fix queue.
    let take = run_json(
        &graph.root,
        &["next", "--take", "5", "--mode", "discovery", "--json"],
    );
    let tmpl = take["batch_template"]
        .as_array()
        .expect("batch_template array");
    assert!(
        !tmpl.is_empty(),
        "discovery --take must emit template lines on this fixture: {take}"
    );
    for l in tmpl {
        let s = l.as_str().expect("template line is a JSON string");
        assert!(
            s.contains("\"confidence\":\"<confidence>\""),
            "every template line must carry a confidence placeholder, not a \
             numeric default (no blind re-ground): {s}"
        );
        assert!(
            !s.contains("\"confidence\":0.9"),
            "the 0.9 confidence default must be gone from the template: {s}"
        );
    }
    // #1+#7 + #8: the hints block is emitted in JSON alongside the template.
    let hints = take["batch_template_hints"]
        .as_array()
        .expect("batch_template_hints array");
    let joined: String = hints
        .iter()
        .filter_map(|h| h.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("independent→a,b,notes"),
        "the field legend must name independent→notes (the field-name trap): {joined}"
    );
    assert!(
        joined.contains("issue→a,b,evidence"),
        "the field legend must name issue→evidence: {joined}"
    );
    assert!(
        joined.contains("--dry-run"),
        "the dry-run guardrail must be surfaced next to the template: {joined}"
    );

    // The committed graph may have all GOVERNS verdicts recorded (green),
    // leaving the quality queue empty. Sync to invalidate some verdicts
    // (the temp dir has no source files, so sync flags them as changed),
    // creating quality items for the template check.
    run_json(&graph.root, &["sync", "--json"]);

    // The QUALITY lane (GOVERNS verdicts — the highest-stakes green) routes
    // through a SEPARATE emitter (run_take_quality), which historically hard-coded
    // a paste-ready "confidence": 0.9 while promising a placeholder. Assert it now
    // carries the same placeholder, so a verbatim paste can't blind-stamp norms.
    let q = run_json(
        &graph.root,
        &["next", "--take", "5", "--mode", "quality", "--json"],
    );
    let qtmpl = q["batch_template"]
        .as_array()
        .expect("quality batch_template array");
    assert!(
        !qtmpl.is_empty(),
        "quality --take must emit rule_verdict template lines on this fixture: {q}"
    );
    for l in qtmpl {
        let s = l.as_str().expect("template line is a JSON string");
        assert!(
            s.contains("\"op\":\"rule_verdict\""),
            "quality lane emits rule_verdict ops: {s}"
        );
        assert!(
            s.contains("\"confidence\":\"<confidence>\"") && !s.contains("\"confidence\":0.9"),
            "quality verdict template must carry a confidence placeholder, not a 0.9 default: {s}"
        );
    }
}

#[test]
fn sqlite_take_template_human_prints_legend_and_dry_run() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("take-tmpl-hints-human");
    let out = run_text_as(
        &graph.root,
        &["next", "--take", "3", "--mode", "discovery"],
        "llm:analyzer",
    );
    assert!(
        out.contains("per-op required fields"),
        "the human template prints the per-op field legend: {out}"
    );
    assert!(
        out.contains("independent→a,b,notes"),
        "the human legend names independent→notes (the field-name trap): {out}"
    );
    assert!(
        out.contains("--dry-run"),
        "the human template surfaces the dry-run guardrail inline: {out}"
    );
    assert!(
        out.contains("\"confidence\":\"<confidence>\""),
        "the human template line carries a confidence placeholder: {out}"
    );
}

// loom-dx #2 forcing function: a template line pasted VERBATIM (confidence
// placeholder unedited) must be rejected by `loom batch` — a scout cannot paste
// the --take template and stamp N false grounds without filling a real
// confidence on each line.
#[test]
fn sqlite_batch_rejects_confidence_placeholder_verbatim_paste() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("batch-rejects-placeholder");
    let (a, b) = first_two_intent_ids(&graph.root);
    // A real criterion isolates the failure to the confidence placeholder —
    // the one field whose default the template used to pre-commit.
    let verbatim = format!(
        "{{\"op\":\"ground\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"these intents coexist without coupling\",\
         \"confidence\":\"<confidence>\"}}"
    );
    write_scratch_file(&graph.root, "scratch/verbatim.jsonl", &verbatim);
    let res = run_json_failure_as(
        &graph.root,
        &["batch", "scratch/verbatim.jsonl", "--dry-run", "--json"],
        "llm:analyzer",
    );
    assert_eq!(res["ok"], 0, "a verbatim paste stamps zero verdicts: {res}");
    assert_eq!(res["failed"], 1, "the placeholder line is rejected: {res}");
    let err = res["results"][0]["error"].as_str().expect("per-line error");
    assert!(
        err.contains("confidence"),
        "the rejection must name the confidence field: {err}"
    );
}

// loom-dx #6: bare `loom next` (no --mode) follows the compass phase instead
// of a hardcoded discovery. The phase is read from `loom status`; the default
// mode must serve that phase's lane. Mirrors phase_default_mode in next.rs.
fn phase_default_mode_for_test(phase: &str) -> &str {
    match phase {
        "build" => "build",
        "fix" => "fix",
        "validate" => "validate",
        "quality" => "quality",
        "discovery" => "discovery",
        _ => "discovery",
    }
}

#[test]
fn sqlite_next_default_follows_compass_phase() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("next-default-phase");
    let st = run_json(&graph.root, &["status", "--json"]);
    let phase = st["graph_state"]["phase"]
        .as_str()
        .expect("status carries graph_state.phase");
    let bare = run_json(&graph.root, &["next", "--json"]);
    let mode = bare["mode"]
        .as_str()
        .expect("bare `loom next` JSON carries a `mode` field");
    assert_eq!(
        mode,
        phase_default_mode_for_test(phase),
        "bare `loom next` must follow the compass phase ({phase}), not a hardcoded default"
    );
    // The regression: the old binary always returned discovery. On a fixture
    // in a mapped non-discovery phase, the default must NOT be discovery.
    if matches!(phase, "fix" | "build" | "validate" | "quality") {
        assert_ne!(
            mode, "discovery",
            "compass phase is {phase} but bare `loom next` defaulted to discovery"
        );
    }
}

#[test]
fn sqlite_next_explicit_mode_overrides_phase_default() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("next-explicit-mode");
    let st = run_json(&graph.root, &["status", "--json"]);
    let phase = st["graph_state"]["phase"]
        .as_str()
        .expect("status carries graph_state.phase");
    // Asking for discovery explicitly must win even when the phase is fix.
    let disc = run_json(&graph.root, &["next", "--mode", "discovery", "--json"]);
    let mode = disc["mode"].as_str().expect("next mode field");
    assert_eq!(mode, "discovery");
    if phase != "discovery" {
        assert_ne!(
            mode, phase,
            "explicit --mode must override the phase default (phase was {phase})"
        );
    }
}

// loom-dx #4: --take on a one-command-per-item mode (build/populate/validate/
// prove) used to hard-error. It now caps to 1 and ANNOUNCES the cap — a silent
// cap is the trap this closes. run_json panics on non-zero exit, so reaching
// the assertions proves the hard error is gone.
#[test]
fn sqlite_next_take_caps_to_one_on_non_bulk_mode() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("next-take-cap");
    let v = run_json(
        &graph.root,
        &["next", "--mode", "validate", "--take", "50", "--json"],
    );
    assert_eq!(v["mode"], "validate");
    assert_eq!(
        v["take_capped_to"], 1,
        "the cap must be visible in JSON: {v}"
    );
    let note = v["take_note"].as_str().expect("take_note field");
    assert!(
        note.contains("one command per item") && note.contains("--all"),
        "the note must explain the cap + point to `loom next --all`: {note}"
    );
}

// honesty-next #2: map-vs-territory is surfaced at EVERY phase, not only the
// audit gate. The fixture sits at phase=fix (a red graph); the old compass hid
// disk reconciliation behind near-green. Now status carries map_vs_territory
// regardless of phase. (The scratch fixture's disk isn't loom's real territory,
// so the COUNTS aren't representative — this test asserts the always-on WIRING:
// the field is present + structured at a non-audit phase, and the human render
// prints the 🗺 line. Count correctness is covered by integrity.rs's unit
// tests of disk_reconciliation_from_parts.)
#[test]
fn sqlite_status_always_surfaces_map_vs_territory() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("status-map-territory");
    let st = run_json(&graph.root, &["status", "--json"]);
    // The regression: map_vs_territory must ALWAYS be disclosed, whatever
    // the phase — including audit (disk issues surface correctly under the
    // post-fix cascade where the audit gate ranks above stale edges).
    let m = &st["map_vs_territory"];
    assert!(
        m.is_object(),
        "status JSON carries map_vs_territory always: {st}"
    );
    let unaccounted = m["unaccounted"].as_u64().expect("unaccounted count");
    let drifted = m["drifted"].as_u64().expect("drifted count");
    let missing = m["missing"].as_u64().expect("missing count");
    let total = m["total"].as_u64().expect("total count");
    assert_eq!(
        total,
        unaccounted + drifted + missing,
        "total decomposes into unaccounted + drifted + missing: {m}"
    );
    assert!(
        !m["message"].as_str().expect("message").is_empty(),
        "the disclosure carries a human message"
    );
    // Human parity: the 🗺 line appears in the plain render too.
    let human = run_text_as(&graph.root, &["status"], "llm:analyzer");
    assert!(
        human.contains('🗺'),
        "human status prints the map-vs-territory line: {human}"
    );
}

// loom-dx #? (rule-show-subcommand-missing): `loom rule show <identifier>`
// returns one rule's full record (detection_logic et al.) so a quality-lane
// agent doesn't list all 22 rules and grep. Matches by NAME first (the handle
// `loom rule list` prints), then by id — either works.
#[test]
fn sqlite_rule_show_by_name_and_id_and_unknown() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("rule-show");
    // by name — the handle a driver pastes from `loom rule list`.
    let by_name = run_json(
        &graph.root,
        &["rule", "show", "endpoint-matched-edges", "--json"],
    );
    assert_eq!(
        by_name["name"].as_str().expect("name"),
        "endpoint-matched-edges",
        "show-by-name returns the named rule: {by_name}"
    );
    assert!(
        by_name["detection_logic"].is_string(),
        "carries detection_logic (the field the card is about): {by_name}"
    );
    let id = by_name["id"].as_str().expect("id").to_string();
    // by id (UUID) resolves the SAME rule — name-first then id fallback.
    let by_id = run_json(&graph.root, &["rule", "show", &id, "--json"]);
    assert_eq!(
        by_id["name"].as_str().expect("name"),
        "endpoint-matched-edges",
        "show-by-id resolves the same rule: {by_id}"
    );
    // unknown -> non-zero + names known rules (a dead-end, not a silent empty).
    // The error bails to stderr with no JSON body, so assert on stderr directly
    // (run_json_failure_as requires JSON on stdout, which a bail doesn't emit).
    let out = std::process::Command::new(loom_bin())
        .args(["rule", "show", "nope-not-a-rule"])
        .current_dir(&graph.root)
        .env("LOOM_AGENT", "llm:quality")
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom rule show <unknown>");
    assert!(
        !out.status.success(),
        "an unknown rule identifier must exit non-zero: {:?}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no rule matches") && stderr.contains("Known rule names"),
        "a miss names known rules so the driver can recover: {stderr}"
    );
}

// loom-dx #? (orphaned-backend-files-survive-upgrade): `loom doctor
// --clean-orphans` reaps dead backend relics (graph.grafeo / db.sqlite /
// graph.db + WAL/SHM) left in .loom/ by past storage generations — the files
// loom once wrote but no longer reads. Dry-run by default; --yes removes.
// Never touches the live graph.sqlite.
#[test]
fn sqlite_doctor_clean_orphans_dry_run_then_yes_removes_relics() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("doctor-clean-orphans");
    let loom = graph.root.join(".loom");
    // plant dead relics from past storage generations
    for relic in ["graph.grafeo", "db.sqlite", "graph.db"] {
        std::fs::write(loom.join(relic), b"dead").unwrap();
    }
    let live = loom.join("graph.sqlite");
    assert!(live.is_file(), "fixture has a live graph.sqlite to protect");

    // dry-run: lists the relics, removes NOTHING.
    let dry = run_json(&graph.root, &["doctor", "--clean-orphans", "--json"]);
    assert_eq!(dry["dry_run"], true, "default is a dry-run preview: {dry}");
    let orphans = dry["orphaned_relics"]
        .as_array()
        .expect("orphaned_relics array");
    assert_eq!(orphans.len(), 3, "lists all three planted relics: {dry}");
    assert_eq!(
        dry["removed"].as_array().unwrap().len(),
        0,
        "dry-run removes nothing: {dry}"
    );
    for r in ["graph.grafeo", "db.sqlite", "graph.db"] {
        assert!(loom.join(r).is_file(), "dry-run did not touch {r}");
    }
    // the live graph.sqlite is NEVER on the reap list.
    assert!(
        orphans
            .iter()
            .all(|v| v.as_str().expect("relic name") != "graph.sqlite"),
        "the live backend is never targeted: {dry}"
    );

    // --yes: removes them.
    let gone = run_json(
        &graph.root,
        &["doctor", "--clean-orphans", "--yes", "--json"],
    );
    assert_eq!(gone["dry_run"], false, "--yes is not a dry-run: {gone}");
    let removed = gone["removed"].as_array().expect("removed array");
    assert_eq!(removed.len(), 3, "removed all three relics: {gone}");
    for r in ["graph.grafeo", "db.sqlite", "graph.db"] {
        assert!(!loom.join(r).exists(), "--yes removed {r}");
    }
    assert!(live.is_file(), "the live graph.sqlite survives the reap");

    // second pass: nothing left, and the graph still reads (reap didn't corrupt).
    let again = run_json(&graph.root, &["doctor", "--clean-orphans", "--json"]);
    assert_eq!(
        again["orphaned_relics"].as_array().unwrap().len(),
        0,
        "idempotent — nothing left to reap: {again}"
    );
    let st = run_json(&graph.root, &["status", "--json"]);
    assert!(
        st["graph_state"].is_object(),
        "graph still reads after the reap: {st}"
    );
}

// HONESTY-NEXT adjudication-drill-down (card 71d45ddf): `loom coverage
// --adjudicated` turns the "N adjudicated (bought green, not grounded)" COUNT
// into an auditable per-symbol trail. The JSON scope is `adjudicated`, the full
// archive ships under adjudicated_symbol_gaps, and the note is honest that
// decision notes carry no confidence field (staleness + author are the
// challenge handles). The human view carries the drill-down header.
#[test]
fn sqlite_coverage_adjudicated_drills_down_per_symbol() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("coverage-adjudicated");

    // JSON: scoped payload, full archive, honest no-confidence note, next_step.
    let json = run_json(&graph.root, &["coverage", "--adjudicated", "--json"]);
    assert_eq!(
        json["scope"].as_str().expect("scope"),
        "adjudicated",
        "adjudicated view is its own scope, not the full coverage dump: {json}"
    );
    assert!(
        json["adjudicated_total"].is_i64(),
        "carries an adjudicated_total count: {json}"
    );
    assert!(
        json["adjudicated_symbol_gaps"].is_array(),
        "ships the full per-symbol archive (not just a count): {json}"
    );
    let note = json["note"].as_str().expect("note");
    assert!(
        note.contains("confidence"),
        "the note is honest that decision notes carry no confidence field: {note}"
    );
    assert!(
        json["next_step"].is_string(),
        "carries a challenge next_step: {json}"
    );

    // Human: the drill-down header (not the regular coverage banner).
    let text = run_text_as(&graph.root, &["coverage", "--adjudicated"], "llm:quality");
    assert!(
        text.contains("adjudication drill-down"),
        "human view carries the drill-down header: {text}"
    );
    // Whether loom's own graph has 0 or N adjudicated symbols, the honest
    // framing is present either way (the empty-state teaches what adjudication
    // IS, the populated state audits each bought symbol).
    assert!(
        text.contains("adjudication") && text.contains("decision note"),
        "teaches that adjudication = green earned by a ruling, not a locator: {text}"
    );
}

// HONESTY-NEXT staleness-severity (card 6171c646): `loom smells --stale` turns
// the undifferentiated "N stale" wall of red into a triaged queue. The JSON
// scope is `stale`; broken + drift + no_grounding partition the total; the note
// is honest that severity ranks by re-inspection cost (blast radius), NOT
// retrospective drift magnitude (sync overwrites the prior symbol set). The
// human view carries the stale-severity header + the broken/drift split.
#[test]
fn sqlite_smells_stale_triages_the_wall_of_red() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("smells-stale");

    // The committed fixture is fully green by design (no stale edges), so
    // manufacture one deterministically: ground a code-kind RELATES_TO on a
    // real file, then change that file and sync — the edge flips to
    // needs_reverification. This keeps the test independent of whatever the
    // committed graph happens to contain.
    write_scratch_file(
        &graph.root,
        "scratch/stale_src.rs",
        "pub fn anchor() -> u8 { 1 }\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "scratch/stale_src.rs", "--json"],
        "llm:builder",
    );
    for nm in ["stale owner one", "stale owner two"] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                nm,
                "--description",
                "owns the stale-severity anchor symbol for the wall-of-red test",
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
                "scratch/stale_src.rs",
                "--locator",
                "fn anchor",
                "--json",
            ],
            "llm:builder",
        );
    }
    let sa = intent_id_by_name(&graph.root, "stale owner one");
    let sb = intent_id_by_name(&graph.root, "stale owner two");
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &sa,
            &sb,
            "ground",
            "--criterion",
            "both realize the shared anchor symbol",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    );
    write_scratch_file(
        &graph.root,
        "scratch/stale_src.rs",
        "pub fn anchor() -> u8 { 2 }\n",
    );
    run_json_as(&graph.root, &["sync", "--json"], "llm:analyzer");

    let json = run_json(&graph.root, &["smells", "--stale", "--json"]);
    assert!(
        json["stale_total"].as_i64().expect("stale_total") >= 1,
        "the manufactured code change must stale at least one edge: {json}"
    );
    assert_eq!(
        json["scope"].as_str().expect("scope"),
        "stale",
        "stale view is its own scope: {json}"
    );
    let total = json["stale_total"].as_i64().expect("stale_total");
    let broken = json["broken"].as_i64().expect("broken");
    let drift = json["drift"].as_i64().expect("drift");
    let no_grounding = json["no_grounding"].as_i64().expect("no_grounding");
    assert!(total >= 0, "stale_total is a non-negative count: {json}");
    assert_eq!(
        broken + drift + no_grounding,
        total,
        "broken + drift + no_grounding partitions the stale total: {json}"
    );
    let edges = json["edges"].as_array().expect("edges array");
    for edge in edges {
        let tier = edge["tier"].as_str().expect("tier");
        assert!(
            matches!(tier, "broken" | "drift" | "no_grounding"),
            "every edge has a known tier: {edge}"
        );
        assert!(
            edge["files"].is_array(),
            "every edge carries its files: {edge}"
        );
        assert!(
            edge["note"].is_string(),
            "every edge carries a note: {edge}"
        );
    }
    let note = json["note"].as_str().expect("note");
    assert!(
        note.contains("drift magnitude"),
        "the note is honest that this is NOT retrospective drift magnitude: {note}"
    );

    // Human: the stale-severity header + the broken/drift split line.
    let text = run_text_as(&graph.root, &["smells", "--stale"], "llm:quality");
    assert!(
        text.contains("stale severity"),
        "human view carries the stale-severity header: {text}"
    );
    assert!(
        text.contains("broken") && text.contains("drift"),
        "human view splits broken vs drift: {text}"
    );
}

// R4/R5/R6 (intake): a conjunction-joined intent/utterance is flagged against the
// granularity contract at intake (intent add + door), the door's standing
// granularity cue is always present, the `--why` rationale is preserved as a
// linked triageable card, and the door's orientation is condition-aware
// (greenfield with no source, brownfield once code exists).
#[test]
fn sqlite_intake_flags_granularity_preserves_why_and_is_condition_aware() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("intake");

    // R4: `intent add` flags a name that joins responsibilities with a conjunction.
    let coarse = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "Undo and Recovery",
            "--description",
            "users can undo actions and recover stale state",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
        "llm:builder",
    );
    assert!(
        coarse["granularity_advisory"]
            .as_str()
            .unwrap_or("")
            .contains("granularity"),
        "a name joining responsibilities with 'and' is flagged: {coarse}"
    );
    // An atomic name is NOT flagged (no false positive).
    let atomic = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "Undo the last action",
            "--description",
            "the user reverses their most recent action",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
        "llm:builder",
    );
    assert!(
        atomic.get("granularity_advisory").is_none(),
        "an atomic intent is not flagged: {atomic}"
    );

    // R4 + R5 + R6 at the door.
    let door = run_json(
        &graph.root,
        &[
            "door",
            "I want undo and redo for every editor action",
            "--why",
            "users lose work constantly and ask for this weekly",
            "--json",
        ],
    );
    assert!(
        door["granularity_cue"]
            .as_str()
            .unwrap_or("")
            .contains("GRANULARITY"),
        "the door carries a standing granularity cue: {door}"
    );
    assert!(
        door["granularity_advisory"]
            .as_str()
            .unwrap_or("")
            .contains("granularity"),
        "the door flags the conjunction in the utterance: {door}"
    );
    // R6: no source in the scratch graph → greenfield orientation.
    assert_eq!(
        door["mode"].as_str(),
        Some("greenfield"),
        "no source on disk → greenfield orientation: {door}"
    );
    // R5: the --why rationale is preserved as a linked, triageable card.
    let rationale = door["rationale_card"].as_str().unwrap_or("");
    assert!(
        !rationale.is_empty(),
        "the --why rationale becomes a card: {door}"
    );
    let utt_id = door["inbox_item"]["id"].as_str().expect("utterance id");
    let backlink = format!("inbox:{utt_id}");
    let show = run_json(&graph.root, &["inbox", "show", rationale, "--json"]);
    let links = show["item"]["links"].as_array().expect("links array");
    assert!(
        links.iter().any(|l| l.as_str() == Some(backlink.as_str())),
        "the rationale card links back to the utterance: {show}"
    );

    // R5 resilience: a too-thin --why must NOT abort capture (CAPTURE FIRST) —
    // the utterance is still captured, no rationale card, and a warning explains.
    let thin = run_json(
        &graph.root,
        &[
            "door",
            "a perfectly capturable utterance here",
            "--why",
            "short",
            "--json",
        ],
    );
    assert!(
        thin["inbox_item"]["id"].as_str().is_some(),
        "a thin --why must still capture the utterance: {thin}"
    );
    assert!(
        thin["rationale_card"].is_null(),
        "a thin --why creates no rationale card: {thin}"
    );
    assert!(
        thin["why_warning"]
            .as_str()
            .unwrap_or("")
            .contains("too thin"),
        "a thin --why is explained, not silently dropped: {thin}"
    );

    // R6: once source exists on disk, the same door flips to brownfield.
    write_scratch_file(&graph.root, "src/real.rs", "pub fn handler() {}\n");
    let door2 = run_json(
        &graph.root,
        &["door", "undo should be reversible everywhere", "--json"],
    );
    assert_eq!(
        door2["mode"].as_str(),
        Some("brownfield"),
        "source on disk → brownfield orientation: {door2}"
    );
}

// R2 (BUILD wires PROVE): the build action carries an explicit prove-the-criterion
// step, teaches verify-first grounding, surfaces the criterion itself, and cues
// build-time relationship capture; marking a realized leaf with no proof flags it
// implemented-but-UNPROVEN instead of reading as silently done.
#[test]
fn sqlite_build_loop_wires_prove_and_flags_unproven() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("build-prove");
    // The committed fixture is fully implemented, so a single planned leaf is the
    // sole build candidate — deterministic.
    run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "r2 build target",
            "--description",
            "the worker must realize then prove this leaf",
            "--criterion",
            "users can undo the last action within the session",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
        "llm:builder",
    );
    let item = run_json(&graph.root, &["next", "--mode", "build", "--json"]);
    let action = item["suggested_action"].as_str().expect("suggested_action");
    assert!(
        action.contains("PROVE") && action.contains("loom validation add"),
        "the build action wires an explicit prove step: {action}"
    );
    assert!(
        action.contains("verified against the file"),
        "the build action teaches verify-first grounding: {action}"
    );
    assert!(
        action.contains("relates to")
            && action.contains("--for analyzer")
            && action.contains("note add --intent")
            && action.contains("--text "),
        "the build action cues a RUNNABLE relationship-capture command (note add needs --text): {action}"
    );
    // R2b: the criterion (THE acceptance test) rides into the work item.
    assert_eq!(
        item["intent_a"]["criterion"].as_str().unwrap_or(""),
        "users can undo the last action within the session",
        "the criterion is surfaced in the build item: {item}"
    );

    // R2c / R11: marking a realized leaf with no proof flags it unproven.
    let id = item["intent_a"]["id"]
        .as_str()
        .expect("intent id")
        .to_string();
    let marked = run_json_as(
        &graph.root,
        &[
            "intent",
            "mark",
            &id,
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    assert!(
        marked["advisory"]
            .as_str()
            .unwrap_or("")
            .contains("UNPROVEN"),
        "an unproven realized leaf is flagged on mark: {marked}"
    );
}

// R1 (verify-first grounding): `loom edge implement` rejects a locator that does
// NOT occur in the file at ground time — a grounding can no longer be born stale
// and surface only at the next sync. A real symbol grounds; a file-level (empty)
// locator grounds; a ghost symbol bails non-zero, naming the miss.
#[test]
fn sqlite_edge_implement_verifies_locator_at_ground_time() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("verify-ground");
    write_scratch_file(
        &graph.root,
        "scratch/r1.rs",
        "pub fn real_sym() -> u8 { 1 }\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "scratch/r1.rs", "--json"],
        "llm:builder",
    );
    for nm in ["r1 real owner", "r1 file owner"] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                nm,
                "--description",
                "owns the verify-first grounding anchor symbol",
                "--level",
                "feature",
                "--lifecycle",
                "implemented",
                "--json",
            ],
            "llm:builder",
        );
    }
    // A real symbol grounds cleanly.
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "r1 real owner",
            "scratch/r1.rs",
            "--locator",
            "fn real_sym",
            "--json",
        ],
        "llm:builder",
    );
    // A file-level (empty) locator grounds cleanly.
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "r1 file owner",
            "scratch/r1.rs",
            "--json",
        ],
        "llm:builder",
    );
    // A ghost symbol is rejected AT GROUND TIME, naming the miss.
    let out = std::process::Command::new(loom_bin())
        .args([
            "edge",
            "implement",
            "r1 real owner",
            "scratch/r1.rs",
            "--locator",
            "fn ghost_symbol",
        ])
        .current_dir(&graph.root)
        .env("LOOM_AGENT", "llm:builder")
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom edge implement with a ghost locator");
    assert!(
        !out.status.success(),
        "grounding a non-existent locator must exit non-zero: {:?}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not occur in") && stderr.contains("stale on arrival"),
        "the rejection names the missing symbol + why: {stderr}"
    );
    // A SUB-TOKEN must not masquerade as the symbol: "real" appears inside
    // "real_sym" but is not it — word-boundary matching rejects the grounding.
    let subtoken = std::process::Command::new(loom_bin())
        .args([
            "edge",
            "implement",
            "r1 real owner",
            "scratch/r1.rs",
            "--locator",
            "real",
        ])
        .current_dir(&graph.root)
        .env("LOOM_AGENT", "llm:builder")
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom edge implement with a sub-token locator");
    assert!(
        !subtoken.status.success(),
        "a sub-token locator ('real' inside 'real_sym') must be rejected: {:?}",
        subtoken.status
    );
}

// The review/prove lanes are AUTONOMOUS (an agent drains them), not human-gated,
// and were previously invisible in `loom status` — a status-driven driver was
// blind to them. The compass must now surface them honestly (visible, autonomous,
// not required-for-green) so they are never mistaken for human work or nonexistent.
#[test]
fn sqlite_status_surfaces_optional_autonomous_lanes() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("optional-autonomous");
    // Manufacture one low-confidence verdict → it lands in the review lane.
    let (a, b) = first_two_intent_ids(&graph.root);
    let line = format!(
        "{{\"op\":\"ground\",\"a\":\"{a}\",\"b\":\"{b}\",\
         \"criterion\":\"they coexist but the coupling is uncertain\",\"confidence\":0.5}}"
    );
    write_scratch_file(&graph.root, "scratch/lc.jsonl", &line);
    run_json_as(
        &graph.root,
        &["batch", "scratch/lc.jsonl", "--json"],
        "llm:analyzer",
    );

    // JSON: the review lane is surfaced, labeled autonomous, and NOT required for green.
    let st = run_json(&graph.root, &["status", "--json"]);
    let opt = &st["optional_autonomous"];
    assert!(
        opt["review"].as_i64().unwrap_or(0) >= 1,
        "the review lane is surfaced in the compass: {st}"
    );
    assert_eq!(
        opt["gate"].as_str(),
        Some("autonomous"),
        "review is labeled autonomous, not human-gated: {opt}"
    );
    assert_eq!(
        opt["required_for_green"].as_bool(),
        Some(false),
        "review is optional, not required for green: {opt}"
    );
    // Human parity: the line names it and says an AGENT (not a human) drains it.
    let text = run_text_as(&graph.root, &["status"], "llm");
    assert!(
        text.contains("optional autonomous") && text.contains("review") && text.contains("AGENT"),
        "the human compass surfaces review as agent-drained autonomous work: {text}"
    );
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
fn sqlite_sync_skips_meaning_only_edges_on_code_change() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-kind-sync");
    write_scratch_file(
        &graph.root,
        "scratch/shared.rs",
        "pub fn shared() -> u8 { 1 }\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "scratch/shared.rs", "--json"],
        "llm:builder",
    );
    for nm in ["alpha kind owner", "beta kind owner"] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                nm,
                "--description",
                "owns the shared helper for the kind-aware sync test",
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
    let a = intent_id_by_name(&graph.root, "alpha kind owner");
    let b = intent_id_by_name(&graph.root, "beta kind owner");
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "ground",
            "--criterion",
            "they sit in the same area of the system",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    );
    let db = graph.root.join(".loom").join("graph.sqlite");
    {
        // Mark the edge as a MEANING-ONLY coupling (concept, not code).
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        conn.execute(
            "UPDATE relates_to SET kinds='[\"same_domain\"]' WHERE (from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)",
            rusqlite::params![a, b],
        )
        .expect("set meaning-only kind");
    }
    // Change the grounded file's code, then sync.
    write_scratch_file(
        &graph.root,
        "scratch/shared.rs",
        "pub fn shared() -> u8 { 2 }\n",
    );
    run_json_as(&graph.root, &["sync", "--json"], "llm:analyzer");
    // The meaning-only edge must NOT be staled by a code change.
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let status: String = conn
        .query_row(
            "SELECT inspection_status FROM relates_to WHERE (from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)",
            rusqlite::params![a, b],
            |r| r.get(0),
        )
        .expect("the edge exists");
    assert_eq!(
        status, "passing",
        "a same_domain (meaning-only) edge must survive a code change, got {status}"
    );
}

#[test]
fn sqlite_wiki_generates_and_checks_freshness() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-wiki");

    // Generate the document projection.
    let gen = run_json(&graph.root, &["wiki", "--json"]);
    assert_eq!(
        gen["status"],
        serde_json::json!("ok"),
        "wiki generates: {gen}"
    );
    let md = std::fs::read_to_string(graph.root.join("loom.wiki.md")).expect("wiki written");
    assert!(
        md.contains("# ") && md.contains("## Architecture") && md.contains("## Overview"),
        "wiki carries the expected sections"
    );

    // Freshly generated → --check is clean (the export-shaped freshness contract).
    let fresh = run_json(&graph.root, &["wiki", "--check", "--json"]);
    assert_eq!(
        fresh["status"],
        serde_json::json!("ok"),
        "fresh wiki passes --check: {fresh}"
    );

    // A graph change makes it stale, and --check fails (non-zero) so CI can catch drift.
    run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "wiki drift probe",
            "--description",
            "a new intent that drifts the wiki projection for this test",
            "--level",
            "feature",
            "--json",
        ],
        "llm:builder",
    );
    let stale = run_json_failure_as(&graph.root, &["wiki", "--check", "--json"], "llm:validator");
    assert_eq!(
        stale["status"],
        serde_json::json!("stale"),
        "a graph change staled the wiki: {stale}"
    );
}

#[test]
fn sqlite_explain_synthesizes_by_intent_and_file() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-explain");

    // By intent id: a synthesized answer — identity + groundings + typed
    // couplings + governance + impact, plus the contract next_step.
    let (a, b) = first_two_intent_ids(&graph.root);
    let by_intent = run_json(&graph.root, &["explain", &a, "--json"]);
    let i = &by_intent["intents"][0];
    assert_eq!(
        i["id"],
        serde_json::json!(a),
        "explains the requested intent: {by_intent}"
    );
    for key in ["grounded_in", "coupled_to", "governed_by", "impact"] {
        assert!(i.get(key).is_some(), "explain must synthesize '{key}': {i}");
    }
    assert!(
        i["impact"].get("ripples_to").is_some(),
        "impact must report what ripples on a code change: {i}"
    );
    assert!(
        by_intent["next_step"]
            .as_str()
            .unwrap_or("")
            .contains("loom"),
        "explain carries a runnable next_step: {by_intent}"
    );

    // By file path: resolves to the intents grounded on that file.
    let path: String = {
        let db = graph.root.join(".loom").join("graph.sqlite");
        rusqlite::Connection::open(&db)
            .expect("open scratch sqlite graph")
            .query_row("SELECT path FROM codefile LIMIT 1", [], |r| r.get(0))
            .expect("a registered codefile exists")
    };
    let by_file = run_json(&graph.root, &["explain", &path, "--json"]);
    assert_eq!(
        by_file["target_file"],
        serde_json::json!(path),
        "a file path resolves as a file target: {by_file}"
    );

    // --impact: the blast-radius mode (pre-change preview of sync's ripple).
    let impact = run_json(&graph.root, &["explain", &path, "--impact", "--json"]);
    assert_eq!(impact["target"], serde_json::json!(path));
    for key in [
        "directly_affected",
        "reopens_relationships",
        "rerun_validations",
        "summary",
    ] {
        assert!(
            impact.get(key).is_some(),
            "--impact must report '{key}': {impact}"
        );
    }
    assert!(
        impact["next_step"]
            .as_str()
            .unwrap_or("")
            .contains("loom sync"),
        "--impact points at the post-change reconcile: {impact}"
    );

    // Regression (stress-test high-sev): query_snapshot filters deprecated
    // intents, but `intent show`/`list` keep them — explain must resolve them too
    // (it reads the unfiltered set) and flag them, not report "Nothing matches".
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        rusqlite::Connection::open(&db)
            .expect("open scratch sqlite graph")
            .execute(
                "UPDATE intent SET status='deprecated' WHERE id=?1",
                rusqlite::params![b],
            )
            .expect("deprecate an intent");
    }
    let dep = run_json(&graph.root, &["explain", &b, "--json"]);
    assert_eq!(
        dep["intents"][0]["deprecated"],
        serde_json::json!(true),
        "a deprecated intent must still resolve in explain and be flagged: {dep}"
    );
}

#[test]
fn sqlite_next_carries_next_step_and_bulk_context() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-next-context");

    // The output contract: a single work item carries a runnable next_step
    // (field-driven driving, not parsing the suggested_action prose).
    let single = run_json(&graph.root, &["next", "--mode", "discovery", "--json"]);
    let next_step = single
        .get("next_step")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        next_step.contains("loom edge explore"),
        "single `next` must carry a runnable next_step: {single}"
    );

    // Bulk read parity: --take items carry the SAME inspection context the single
    // item does (descriptions + groundings), so the agent fills the batch
    // template's criteria without scripting a per-pair `loom intent show` loop.
    let take = run_json(
        &graph.root,
        &["next", "--take", "3", "--mode", "discovery", "--json"],
    );
    let item = take["groups"][0]["items"][0].clone();
    assert!(
        item["a"]["description"].is_string(),
        "bulk discovery item must carry intent A's description: {item}"
    );
    assert!(
        item["a"]["groundings"].is_array(),
        "bulk discovery item must carry intent A's code groundings: {item}"
    );
    assert!(
        item["b"]["description"].is_string(),
        "bulk discovery item must carry intent B's description: {item}"
    );

    // The bulk guidance must match what discovery actually hands back — NOT the
    // re-verification ("staling file") text that misled the LLM into scripting.
    let guidance = take["guidance"].as_str().unwrap_or("");
    assert!(
        guidance.contains("UNEXPLORED"),
        "discovery --take guidance must be discovery-specific: {guidance}"
    );
    assert!(
        !guidance.contains("staling file"),
        "discovery --take must NOT emit the re-verification guidance: {guidance}"
    );
}

#[test]
fn sqlite_export_import_round_trips_relationship_kinds() {
    let _guard = sqlite_test_lock();
    let edges_with_kinds = |root: &Path| -> i64 {
        let db = root.join(".loom").join("graph.sqlite");
        rusqlite::Connection::open(&db)
            .expect("open scratch sqlite graph")
            .query_row(
                "SELECT count(*) FROM relates_to WHERE kinds != '[]'",
                [],
                |r| r.get(0),
            )
            .expect("count kinded edges")
    };
    // The committed fixture carries the taxonomy; import must READ the kinds.
    let g1 = setup_imported_graph("sqlite-kinds-rt-1");
    let imported = edges_with_kinds(&g1.root);
    assert!(
        imported > 0,
        "committed loom.graph.json carries relationship kinds; import must materialize them"
    );
    // Export g1's DB, then import that export into a fresh graph — the kinds
    // must survive the full DB → JSON → DB round-trip (the portability contract).
    run_text_as(&g1.root, &["export"], "llm:validator");
    let g2 = ScratchGraph::new("sqlite-kinds-rt-2");
    fs::copy(
        g1.root.join("loom.graph.json"),
        g2.root.join("loom.graph.json"),
    )
    .expect("carry g1's export to g2");
    run_json(&g2.root, &["init", ".", "--json"]);
    run_json(&g2.root, &["import", "loom.graph.json", "--json"]);
    assert_eq!(
        edges_with_kinds(&g2.root),
        imported,
        "export → import must preserve every relationship kind (taxonomy travels with the repo)"
    );
}

#[test]
fn sqlite_doctor_flags_weak_only_grounding() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-weak-grounding");
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        conn.execute(
            "UPDATE relates_to SET inspection_status='passing', kinds='[\"same_domain\"]' \
             WHERE rowid=(SELECT rowid FROM relates_to LIMIT 1)",
            [],
        )
        .expect("set weak-only passing edge");
    }
    // hints never fail the exit code, so doctor stays healthy → plain run_json.
    let doctor = run_json(&graph.root, &["doctor", "--json"]);
    let hints = doctor["hints"].to_string();
    assert!(
        hints.contains("only by weak") || hints.contains("weak kinds"),
        "doctor must flag a passing edge grounded only by weak kinds: {hints}"
    );
}

#[test]
fn sqlite_judgment_kind_assignment() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-judgment-kind");
    let db = graph.root.join(".loom").join("graph.sqlite");

    // rule add --kind sets the category AND derives the default effort.
    run_json_as(
        &graph.root,
        &[
            "rule",
            "add",
            "--name",
            "no-secrets-in-source",
            "--description",
            "secrets never enter source code; use a secret store",
            "--severity",
            "error",
            "--kind",
            "security",
            "--json",
        ],
        "llm:quality",
    );
    {
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        let (kind, effort): (String, String) = conn
            .query_row(
                "SELECT kind, inspection_effort FROM quality_rule WHERE name='no-secrets-in-source'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("rule exists");
        assert_eq!(kind, "security", "rule kind stored");
        assert_eq!(effort, "high", "security defaults to high effort");
    }

    // edge ground --kind asserts a judgment relationship kind.
    let (a, b) = first_two_intent_ids(&graph.root);
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "ground",
            "--criterion",
            "a invokes a function grounded by b",
            "--kind",
            "calls",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    );
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let kinds: String = conn
        .query_row(
            "SELECT kinds FROM relates_to WHERE (from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)",
            rusqlite::params![a, b],
            |r| r.get(0),
        )
        .expect("the grounded edge exists");
    assert!(
        kinds.contains("calls"),
        "judgment kind asserted on the edge: {kinds}"
    );
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
fn sqlite_hypothesis_adoption_outcome_validation_confirms_then_settles_targets() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("hypothesis-outcome-flow");
    run_json(&graph.root, &["init", ".", "--json"]);

    write_scratch_file(
        &graph.root,
        "src/outcome_flow.rs",
        "pub fn adoption_outcome_flow() -> bool {\n    false\n}\n",
    );
    assert_status_ok(&run_json_as(
        &graph.root,
        &["codefile", "add", "src/outcome_flow.rs", "--json"],
        "llm:builder",
    ));

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "adoption outcome target",
            "--description",
            "target behavior for proving hypothesis adoption outcomes",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    let target_id = intent["id"].as_str().expect("target intent id");
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            target_id,
            "src/outcome_flow.rs",
            "--locator",
            "fn adoption_outcome_flow",
            "--json",
        ],
        "llm:builder",
    ));

    let predicted_outcome = "adopted hypothesis outcome is captured as a passable proof";
    let hypothesis = run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "add",
            "--name",
            "adoption outcome proof",
            "--claim",
            "adoption needs to preserve the promised outcome as follow-up evidence",
            "--proposal",
            "turn the promised outcome into a validation when adopting",
            "--predicted-outcome",
            predicted_outcome,
            "--target",
            target_id,
            "--json",
        ],
        "llm:builder",
    );
    let hypothesis_id = hypothesis["id"].as_str().expect("hypothesis id");
    assert_eq!(
        hypothesis["status"], "proposed",
        "adding a hypothesis should start it in the pre-decision state: {hypothesis}"
    );
    assert!(
        hypothesis["targets"]
            .as_array()
            .expect("hypothesis targets")
            .iter()
            .any(|target| target == target_id),
        "hypothesis add must TARGET the requested intent: {hypothesis}"
    );

    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "prove",
            hypothesis_id,
            "--verdict",
            "supported",
            "--evidence",
            "the scratch target needs an adoption outcome proof",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    ));
    let supported = run_json_as(
        &graph.root,
        &["hypothesis", "show", hypothesis_id, "--json"],
        "llm:builder",
    );
    assert_eq!(
        supported["hypothesis"]["status"], "supported",
        "supported proof should move the hypothesis to the adoptable state: {supported}"
    );
    assert!(
        supported["targets"]
            .as_array()
            .expect("supported targets")
            .iter()
            .any(|target| {
                target["intent_id"] == target_id && target["inspection_status"] == "passing"
            }),
        "supporting the hypothesis must stamp its TARGETS evidence passing: {supported}"
    );

    let adopted = run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "adopt",
            hypothesis_id,
            "--spawned",
            target_id,
            "--json",
        ],
        "llm:builder",
    );
    assert_eq!(
        adopted["adopted"], true,
        "adopt should report the hypothesis decision: {adopted}"
    );
    assert!(
        adopted["spawned"]
            .as_array()
            .expect("spawned intents")
            .iter()
            .any(|spawned| spawned == target_id),
        "adopt should attach the spawned/target intent to the decision: {adopted}"
    );

    let outcome_validation = run_json(
        &graph.root,
        &[
            "validation",
            "show",
            "hypothesis outcome: adoption outcome proof",
            "--json",
        ],
    );
    let outcome_validation_id = outcome_validation["id"]
        .as_str()
        .expect("outcome validation id");
    assert_eq!(
        outcome_validation["validation_type"], "manual_check",
        "adoption outcome proof should be a manual validation: {outcome_validation}"
    );
    assert_eq!(
        outcome_validation["last_result"], "not_run",
        "adoption should create the predicted outcome proof as not_run: {outcome_validation}"
    );
    assert!(
        outcome_validation["description"]
            .as_str()
            .expect("outcome validation description")
            .contains(predicted_outcome),
        "the outcome validation should carry the predicted_outcome text: {outcome_validation}"
    );
    assert!(
        outcome_validation["description"]
            .as_str()
            .expect("outcome validation description")
            .contains(hypothesis_id),
        "the outcome validation should retain the source hypothesis id for confirmation: {outcome_validation}"
    );

    let db = graph.root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let (validates_status, validates_notes): (String, String) = conn
        .query_row(
            "SELECT inspection_status, notes FROM validates WHERE validation_id = ?1 AND intent_id = ?2",
            rusqlite::params![outcome_validation_id, target_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("outcome validation is VALIDATES-linked to the spawned target");
    assert_eq!(
        validates_status, "uninspected",
        "the new not_run outcome proof should not claim validation evidence yet"
    );
    assert_eq!(
        validates_notes, "hypothesis outcome proof",
        "the VALIDATES edge should identify that it came from hypothesis adoption"
    );

    // While the hypothesis is in-flight (proven + adopted, not yet confirmed), a
    // target code change DOES stale its TARGETS evidence — the sync ripple at work.
    write_scratch_file(
        &graph.root,
        "src/outcome_flow.rs",
        "pub fn adoption_outcome_flow() -> bool {\n    true\n}\n",
    );
    let in_flight_sync = run_json(&graph.root, &["sync", "--json"]);
    assert_eq!(
        in_flight_sync["targets_edges_flagged"], 1,
        "an in-flight (pre-confirm) hypothesis's TARGETS stales on a target code change: {in_flight_sync}"
    );

    let marked = run_json(
        &graph.root,
        &[
            "validation",
            "mark",
            outcome_validation_id,
            "--result",
            "passed",
            "--evidence",
            "manual acceptance confirms the predicted outcome",
            "--json",
        ],
    );
    assert_eq!(
        marked["result"], "passed",
        "marking the outcome validation should record a pass: {marked}"
    );
    assert_eq!(
        marked["intents_updated"], 1,
        "the passed outcome validation should update the one linked target intent: {marked}"
    );
    let confirmed = run_json_as(
        &graph.root,
        &["hypothesis", "show", hypothesis_id, "--json"],
        "llm:builder",
    );
    assert_eq!(
        confirmed["hypothesis"]["status"], "confirmed",
        "passing the adopted outcome validation should confirm the hypothesis: {confirmed}"
    );

    // Once confirmed, the hypothesis is settled: the next sync reconciles its staled
    // TARGETS back to passing (prove is closed — the live proof is the spawned
    // intents' validations), and further target code changes never re-stale it.
    let settle_sync = run_json(&graph.root, &["sync", "--json"]);
    let settled = run_json_as(
        &graph.root,
        &["hypothesis", "show", hypothesis_id, "--json"],
        "llm:builder",
    );
    assert!(
        settled["targets"]
            .as_array()
            .expect("settled targets")
            .iter()
            .any(|target| {
                target["intent_id"] == target_id && target["inspection_status"] == "passing"
            }),
        "sync settles a confirmed hypothesis's staled TARGETS back to passing: {settled} (sync: {settle_sync})"
    );
    write_scratch_file(
        &graph.root,
        "src/outcome_flow.rs",
        "pub fn adoption_outcome_flow() -> bool {\n    !false\n}\n",
    );
    let post_confirm_sync = run_json(&graph.root, &["sync", "--json"]);
    assert_eq!(
        post_confirm_sync["targets_edges_flagged"], 0,
        "a confirmed hypothesis's TARGETS is never re-staled by sync: {post_confirm_sync}"
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

// FALSE-GREEN [compass-must-not-overstate]: coverage's "✓ No open actionable
// symbol gaps" must not ride over adjudication-bought green. loom's own graph
// resolves most symbols by adjudication (a decision note), not by a grounding
// locator — "no OPEN gaps" is true, but green earned by adjudication rather
// than grounding is the shape the false-green cluster hunts. The headline ✓
// must disclose the co-located negative (adjudicated count) next to itself.
#[test]
fn sqlite_coverage_qualifies_no_gaps_with_adjudicated_green() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("coverage-qualify");
    // Human output (not --json): the ✓ headline + its qualifier are a
    // presentation fix, so assert on the text surface an agent reads.
    let text = run_text_as(&graph.root, &["coverage"], "llm:validator");
    // symbol_accountability is graph-derived (symbol_facts + locators), so it
    // renders even on a tree-less scratch checkout. loom's graph carries
    // adjudicated symbols, so the bare ✓ must be qualified.
    assert!(
        text.contains("No open actionable symbol gaps (but")
            && text.contains("adjudicated (bought green, not grounded)"),
        "the ✓ 'no open gaps' headline must be bounded by the adjudicated-bought-green \
         negative instead of riding over it: {text}"
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
    // The door-captured card must appear as a `new` item. (Assert presence, not
    // a global count: the imported graph may already carry other intake cards.)
    assert!(listed["count"].as_i64().unwrap() >= 1);
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

#[test]
fn sqlite_status_json_top_level_keys_are_frozen() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-status-keys");
    let status = run_json(&graph.root, &["status", "--json"]);
    let mut keys: Vec<String> = status
        .as_object()
        .expect("status --json is an object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    // The --json contract is the agent-facing driving interface. This frozen set
    // makes a dropped/renamed top-level key a TEST failure, not a silent break.
    // Changing it must be DELIBERATE: update this list in the same commit.
    let expected = [
        "advisories",
        "audit",
        "blocked_validations",
        "committed_export",
        "completion",
        "failing_edges",
        "graph_state",
        "human_gated",
        "independent_edges",
        "intake",
        "intents_without_validations",
        "map_vs_territory",
        "needs_reverification",
        "open_issues",
        "optional_autonomous",
        "other_lanes",
        "passing_edges",
        "populate",
        "total_codefiles",
        "total_edges",
        "total_intents",
        "total_validations",
        "uninspected_edges",
        "uninspected_outside_queues",
        "validation_health",
        "validation_pass_rate",
        "validation_pass_rate_runnable",
    ];
    assert_eq!(
        keys, expected,
        "status --json top-level key set changed — update the frozen set DELIBERATELY"
    );
}

#[test]
fn sqlite_to_be_removed_lifecycle_round_trips() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-to-be-removed");
    let added = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "legacy shim slated for deletion",
            "--description",
            "a compatibility shim that should be removed once callers migrate",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    let id = added["id"].as_str().expect("new intent id").to_string();
    run_json_as(
        &graph.root,
        &[
            "intent",
            "mark",
            &id,
            "--lifecycle",
            "to_be_removed",
            "--reason",
            "superseded by the v2 API; delete after callers migrate",
            "--json",
        ],
        "llm:fixer",
    );
    let shown = run_json(&graph.root, &["intent", "show", &id, "--json"]);
    assert_eq!(
        shown["intent"]["lifecycle"], "to_be_removed",
        "to_be_removed must survive the schema CHECK + read path: {shown}"
    );
}

/// inspection_status of the RELATES_TO edge between `intent show` subject and
/// `other_id`, from the show JSON's `edges` array.
fn relates_status_to(shown: &Value, other_id: &str) -> String {
    shown["edges"]
        .as_array()
        .expect("edges array")
        .iter()
        .find(|e| e["from_id"] == other_id || e["to_id"] == other_id)
        .and_then(|e| e["inspection_status"].as_str())
        .unwrap_or("(none)")
        .to_string()
}

#[test]
fn sqlite_federation_child_export_change_ripples_to_seam_intents() {
    let _guard = sqlite_test_lock();
    // A fresh, codefile-free graph: the ONLY possible staleness source is the
    // cross-service federation ripple (no files to flag, no import).
    let graph = ScratchGraph::new("federation-ripple");
    run_json(&graph.root, &["init", ".", "--json"]);

    let a = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "grid query gateway",
            "--description",
            "parent seam that consumes the grid child service contract",
            "--level",
            "feature",
            "--json",
        ],
        "llm:builder",
    );
    let a_id = a["id"].as_str().expect("intent a id").to_string();
    let b = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "result cache",
            "--description",
            "caches grid responses for the gateway",
            "--level",
            "feature",
            "--json",
        ],
        "llm:builder",
    );
    let b_id = b["id"].as_str().expect("intent b id").to_string();
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a_id,
            &b_id,
            "ground",
            "--criterion",
            "the gateway feeds the cache after each grid call; verified the call order",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    );

    // A child export artifact + a delegation whose seam is intent A.
    write_scratch_file(&graph.root, "child/loom.graph.json", "child-export-v1");
    run_json_as(
        &graph.root,
        &[
            "delegate",
            "add",
            "child/**",
            "--to",
            "child/loom.graph.json",
            "--json",
        ],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["delegate", "seam", "child/**", &a_id, "--json"],
        "llm:builder",
    );

    // First sync only baselines the child-export hash — no ripple.
    run_json(&graph.root, &["sync", ".", "--json"]);
    let before = run_json(&graph.root, &["intent", "show", &a_id, "--json"]);
    assert_eq!(
        relates_status_to(&before, &b_id),
        "passing",
        "baseline sync must not ripple: {before}"
    );

    // The child's committed contract changes → the next sync re-opens the seam.
    write_scratch_file(
        &graph.root,
        "child/loom.graph.json",
        "child-export-v2-CHANGED",
    );
    run_json(&graph.root, &["sync", ".", "--json"]);
    let after = run_json(&graph.root, &["intent", "show", &a_id, "--json"]);
    assert_eq!(
        relates_status_to(&after, &b_id),
        "needs_reverification",
        "a child export change must ripple to the seam intent: {after}"
    );
}

#[test]
fn sqlite_populate_interfaces_prune_removes_orphan_surfaces() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-prune-surface");
    run_json(&graph.root, &["init", ".", "--json"]);
    insert_interface_surface(&graph.root, "orphan", "GET /old", "GET", "/old");
    let before = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(before["interface_gaps"]["surface_without_calls"], 1);
    // No sagas → nothing recreated; --prune removes the call-less orphan.
    let res = run_json_as(
        &graph.root,
        &[
            "populate",
            "interfaces",
            "--from-sagas",
            "--prune",
            "--json",
        ],
        "llm:builder",
    );
    assert_eq!(
        res["surfaces_pruned"], 1,
        "the orphan surface was pruned: {res}"
    );
    let after = run_json(&graph.root, &["interface", "gaps", "--json"]);
    assert_eq!(after["interface_gaps"]["surface_without_calls"], 0);
}

#[test]
fn sqlite_persona_list_flags_orphans() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-persona-orphan");
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "persona",
            "add",
            "--name",
            "ghost",
            "--description",
            "an audience segment with no serves or journeys whatsoever",
            "--json",
        ],
        "llm:builder",
    );
    let listed = run_json(&graph.root, &["persona", "list", "--json"]);
    assert_eq!(
        listed["orphans"].as_array().unwrap().len(),
        1,
        "a persona with no SERVES/JOURNEYS is flagged orphan: {listed}"
    );
}

#[test]
fn loom_write_blocks_with_named_error_while_lock_held() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("lock-race");
    run_json(&graph.root, &["init", ".", "--json"]);
    let a = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "alpha",
            "--description",
            "first intent for the lock race test",
            "--level",
            "feature",
            "--json",
        ],
        "llm:builder",
    );
    let b = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "beta",
            "--description",
            "second intent for the lock race test",
            "--level",
            "feature",
            "--json",
        ],
        "llm:builder",
    );
    let a_id = a["id"].as_str().unwrap().to_string();
    let b_id = b["id"].as_str().unwrap().to_string();

    // Hold the cross-process write lock from THIS process — the SAME flock file
    // the binary locks. A competing TRANSACTIONAL write must then fail (after a
    // short deadline) with the NAMED error, not a raw rusqlite "database locked".
    let lock_path = graph.root.join(".loom/graph.lock");
    let held = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .unwrap();
    fs2::FileExt::lock_exclusive(&held).unwrap();

    let out = Command::new(loom_bin())
        .args([
            "edge",
            "explore",
            &a_id,
            &b_id,
            "ground",
            "--criterion",
            "alpha relates to beta; verified by the lock-race test",
            "--confidence",
            "0.9",
            "--json",
        ])
        .current_dir(&graph.root)
        .env("LOOM_AGENT", "llm:analyzer")
        .env("LOOM_LOCK_DEADLINE_MS", "300")
        .env_remove("LOOM_GRAPH")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a write must fail while another session holds the lock"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("write lock is held by another loom session"),
        "expected the named lock error, got: {combined}"
    );

    // After the holder releases, the same write succeeds.
    fs2::FileExt::unlock(&held).unwrap();
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a_id,
            &b_id,
            "ground",
            "--criterion",
            "alpha relates to beta; verified after lock release",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    );
}

#[test]
fn sqlite_federation_ripple_edge_cases() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("federation-edges");
    run_json(&graph.root, &["init", ".", "--json"]);
    // Three intents, two passing RELATES_TO into a shared target; A and C are seams.
    let mk = |name: &str, desc: &str| -> String {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                desc,
                "--level",
                "feature",
                "--json",
            ],
            "llm:builder",
        )["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let a = mk(
        "gateway one",
        "first parent seam onto the grid child contract",
    );
    let c = mk(
        "gateway two",
        "second parent seam onto the grid child contract",
    );
    let b = mk("shared cache", "shared downstream both gateways feed");
    for from in [&a, &c] {
        run_json_as(
            &graph.root,
            &[
                "edge",
                "explore",
                from,
                &b,
                "ground",
                "--criterion",
                "gateway feeds the shared cache; verified call order",
                "--confidence",
                "0.9",
                "--json",
            ],
            "llm:analyzer",
        );
    }

    write_scratch_file(&graph.root, "child/loom.graph.json", "child-v1");
    run_json_as(
        &graph.root,
        &[
            "delegate",
            "add",
            "child/**",
            "--to",
            "child/loom.graph.json",
            "--json",
        ],
        "llm:builder",
    );
    // MULTIPLE seams on one delegation.
    run_json_as(
        &graph.root,
        &["delegate", "seam", "child/**", &a, "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["delegate", "seam", "child/**", &c, "--json"],
        "llm:builder",
    );
    run_json(&graph.root, &["sync", ".", "--json"]); // baseline

    // Child changes → BOTH seams' edges re-open.
    write_scratch_file(&graph.root, "child/loom.graph.json", "child-v2-changed");
    run_json(&graph.root, &["sync", ".", "--json"]);
    let sa = run_json(&graph.root, &["intent", "show", &a, "--json"]);
    let sc = run_json(&graph.root, &["intent", "show", &c, "--json"]);
    assert_eq!(
        relates_status_to(&sa, &b),
        "needs_reverification",
        "seam A rippled: {sa}"
    );
    assert_eq!(
        relates_status_to(&sc, &b),
        "needs_reverification",
        "seam C rippled: {sc}"
    );

    // EDGE CASE: a DELETED child export must not crash sync and must not ripple.
    // Re-ground A→B to passing, delete the child export, sync.
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "ground",
            "--criterion",
            "re-grounded after the ripple; verified again",
            "--confidence",
            "0.9",
            "--json",
        ],
        "llm:analyzer",
    );
    std::fs::remove_file(graph.root.join("child/loom.graph.json")).unwrap();
    run_json(&graph.root, &["sync", ".", "--json"]); // must succeed (missing child export skipped)
    let sa2 = run_json(&graph.root, &["intent", "show", &a, "--json"]);
    assert_eq!(
        relates_status_to(&sa2, &b),
        "passing",
        "a missing child export must not ripple (skipped, no crash): {sa2}"
    );
}

// FALSE-GREEN [status-audit-open-findings-null-not-zero]: when the audit scan
// is deferred (any phase other than audit|complete), `loom status --json` must
// emit `audit.open_findings: null` — never a literal `0` — so a programmatic
// consumer keying on that field cannot mistake "no scan ran" for "scan ran and
// found zero" (the false-green remnant). `computed:false` already says so, but
// a consumer reading only `open_findings` saw a clean 0.
#[test]
fn sqlite_status_audit_open_findings_null_when_deferred() {
    let _g = sqlite_test_lock();
    let graph = setup_imported_graph("audit-null");
    // Post phase-cascade-fix: the audit gate now ranks above stale edges, so
    // this fixture (temp dir with no source files → disk issues) may reach
    // phase=audit and the scan IS computed. The test verifies the null-vs-0
    // contract CONDITIONALLY — when deferred, open_findings must be null; the
    // shape (top_kinds as array) must be stable regardless.
    let status = run_json(&graph.root, &["status", "--json"]);
    let audit = status
        .get("audit")
        .expect("status --json carries an audit pulse");
    let computed = audit.get("computed").and_then(|v| v.as_bool());
    if computed == Some(false) {
        assert!(
            audit.get("open_findings").is_none() || audit["open_findings"].is_null(),
            "audit.open_findings must be null (not 0) when computed:false — a programmatic \
             consumer keying on this field must not read 'audit clean' when no scan ran. Got: {audit}"
        );
    }
    assert!(
        audit
            .get("top_kinds")
            .map(|v| v.is_array())
            .unwrap_or(false),
        "audit.top_kinds must be an array regardless of computed state: {audit}"
    );
}

// FALSE-GREEN [oversized-file-loc-detector]: a god-file's physical size must
// surface as a single file-level finding that survives the per-symbol
// adjudication path — not as N adjudicable per-symbol findings that each get
// ruled away. Adjudication is keyed on <kind>:<scope>, so a ruling about one
// symbol cannot launder the file finding; only its own oversized_file:<path>
// ruling answers it.
#[cfg(feature = "treesitter")]
fn smell_open_for(value: &Value, kind: &str, path_prefix: &str) -> bool {
    value["smells"]
        .as_array()
        .map(|a| {
            a.iter().any(|s| {
                s["kind"] == kind
                    && s["summary"]
                        .as_str()
                        .map(|t| t.starts_with(path_prefix))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(feature = "treesitter")]
fn smell_adjudicated_for(value: &Value, kind: &str, path_prefix: &str) -> bool {
    value["adjudicated"]
        .as_array()
        .map(|a| {
            a.iter().any(|s| {
                s["kind"] == kind
                    && s["summary"]
                        .as_str()
                        .map(|t| t.starts_with(path_prefix))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// Re-opening the oversized_file finding requires re-extracting a ≥2000-line
// symbol span via `loom sync`, which is tree-sitter-only; under the heuristic
// (no-default-features) path sync produces no symbol extent, so this test is
// gated to the `treesitter` feature. The adjudication logic it exercises
// (a per-symbol ruling cannot launder the file-level finding) is
// feature-independent and lives in `compute_smells_from_parts`.
#[cfg(feature = "treesitter")]
#[test]
fn sqlite_oversized_file_survives_per_symbol_adjudication() {
    let _g = sqlite_test_lock();
    let graph = setup_imported_graph("oversized-file");
    let smells = || run_json(&graph.root, &["smells", "--limit", "500", "--json"]);
    let path = "src/db/sqlite.rs";
    // The committed graph may carry a pre-existing decision note that
    // adjudicates this finding. Write a LARGE file to disk and sync to reset
    // last_modified, making any prior decision note stale and re-opening the
    // finding. Use many TOP-LEVEL `fn` declarations (not one big body) so the
    // last symbol sits near line 2100 — its line number alone yields extent
    // ≥ 2000 under BOTH the tree-sitter and the heuristic symbol extractors.
    let mut big = String::new();
    for i in 0..2100 {
        big.push_str(&format!("fn _f{i}() {{ let _ = {i}; }}\n"));
    }
    write_scratch_file(&graph.root, path, &big);
    run_json(&graph.root, &["sync", "--json"]);

    // 1. The god-file (~5960 physical lines) surfaces as an open oversized_file
    //    finding — physical size measured as such, independent of impl/test.
    let s = smells();
    assert!(
        smell_open_for(&s, "oversized_file", path),
        "oversized_file must fire on the god-file {path}: {s}"
    );

    // 2. A PER-SYMBOL ruling (large_behavioral_symbol:<path>:<label>) must NOT
    //    launder the file-level finding — adjudication is keyed on <kind>:<scope>,
    //    so a ruling about one symbol cannot clear the file. This is the crux of
    //    the card: the god-file must not be hidden behind per-symbol rulings.
    run_json(
        &graph.root,
        &[
            "note",
            "add",
            "--smell",
            "large_behavioral_symbol:src/db/sqlite.rs:impl SqliteGraphStore",
            "--kind",
            "decision",
            "--text",
            "the impl is deliberately large; this ruling is about a SYMBOL, not the file",
            "--json",
        ],
    );
    let s = smells();
    assert!(
        smell_open_for(&s, "oversized_file", path),
        "a per-symbol large_behavioral_symbol ruling must NOT launder the file-level \
         oversized_file finding: {s}"
    );
    assert!(
        !smell_adjudicated_for(&s, "oversized_file", path),
        "the file finding must not appear adjudicated from a per-symbol ruling: {s}"
    );

    // 3. The finding IS answerable by its OWN identity — a decision note keyed
    //    on oversized_file:<path> suppresses it (and surfaces in adjudicated
    //    with its ruling), so a deliberate god-file can be recorded honestly.
    run_json(
        &graph.root,
        &[
            "note",
            "add",
            "--smell",
            "oversized_file:src/db/sqlite.rs",
            "--kind",
            "decision",
            "--text",
            "god-file is deliberate for now; this ruling is about the FILE",
            "--json",
        ],
    );
    let s = smells();
    assert!(
        !smell_open_for(&s, "oversized_file", path),
        "the own oversized_file:<path> ruling must suppress the file finding: {s}"
    );
    assert!(
        smell_adjudicated_for(&s, "oversized_file", path),
        "the suppressed file finding must surface in adjudicated with its ruling: {s}"
    );
}

// RUBBER-STAMP BAR: a smell decision note must be a real per-finding
// inspection, not a pasted template. The write-time gate rejects (a) a
// vacuous/too-short ruling and (b) a ruling that reuses the wording of one
// already recorded on ANOTHER finding — so an agent cannot batch-stamp the
// audit gate green with one rationale. The first ruling of a template lands;
// the second bounces. Uses a bare graph so the assertion is independent of any
// fixture's existing notes.
#[test]
fn sqlite_smell_adjudication_rejects_batch_rubber_stamp() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("smell-rubber-stamp");
    run_json(&graph.root, &["init", ".", "--json"]);

    // Run `loom note add --smell …`; return (succeeded, combined stdout+stderr).
    let add_ruling = |finding: &str, text: &str| -> (bool, String) {
        let out = Command::new(loom_bin())
            .args([
                "note", "add", "--smell", finding, "--kind", "decision", "--text", text,
            ])
            .current_dir(&graph.root)
            .env("LOOM_AGENT", "llm")
            .env_remove("LOOM_GRAPH")
            .output()
            .expect("run loom note add");
        let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
        combined.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), combined)
    };

    // 1. A substantive, finding-specific first ruling lands.
    let (ok, _) = add_ruling(
        "large_behavioral_symbol:src/commands/intent.rs:fn run_with_sqlite",
        "intent.rs runs 8 distinct subcommand handlers in one match; each arm threads the same \
         db+printer setup and extracting them would only relocate that shared header — measured, not split",
    );
    assert!(ok, "a substantive first ruling must be accepted");

    // 2. The SAME rationale reworded onto ANOTHER finding is rejected — the
    //    batch rubber-stamp the bar exists to stop.
    let (ok, err) = add_ruling(
        "large_behavioral_symbol:src/commands/hypothesis.rs:fn run_with_sqlite",
        "hypothesis.rs runs 5 distinct subcommand handlers in one match; each arm threads the same \
         db+printer setup and extracting them would only relocate that shared header — measured, not split",
    );
    assert!(
        !ok,
        "a reworded copy of an existing ruling must be rejected"
    );
    assert!(
        err.contains("reuses the wording") && err.contains("intent.rs"),
        "the bounce must name the finding it echoes: {err}"
    );

    // 3. A genuinely different, finding-specific ruling on a third finding lands.
    let (ok, _) = add_ruling(
        "oversized_file:src/cli.rs",
        "cli.rs is the clap-derive declaration surface: every command is a struct with no behavioral \
         body to extract, only flags to read; there is nothing here to decompose into modules",
    );
    assert!(ok, "a genuinely distinct ruling must be accepted");

    // 4. A vacuous ruling never suppresses a finding.
    let (ok, err) = add_ruling("tangled_file:src/db/sqlite.rs", "deliberate");
    assert!(!ok, "a vacuous ruling must be rejected");
    assert!(
        err.contains("substantive inspection"),
        "the vacuous bounce must teach the bar: {err}"
    );
}

// FALSE-GREEN [proven-axis-proof-quality-ceiling]: `proven` must NOT conflate
// executed test-proof with hand-marked acceptance. The graph records a pass
// identically whether a command RAN and passed or `loom validation mark
// --result passed` stamped it (mark_validation_result sets last_run too), so
// the discriminator is the validation's SHAPE — a runnable validation_type
// (test/assertion/benchmark/saga) WITH a command is EXECUTED; a manual_check or
// an empty command is ASSERTED. The axis splits proven into executed vs
// asserted-only and the compass discloses both inline.
fn coverage_proven(status: &Value) -> &Value {
    status
        .get("coverage")
        .or_else(|| status.get("graph_state").and_then(|g| g.get("coverage")))
        .expect("status json has a coverage object")
}

#[test]
fn sqlite_proven_axis_invariant_executed_plus_asserted_equals_proven() {
    let _g = sqlite_test_lock();
    let graph = setup_imported_graph("sqlite-proven-invariant");
    let status = run_json(&graph.root, &["status", "--json"]);
    let cov = coverage_proven(&status);
    let proven = cov["proven_leaves"]["covered"]
        .as_i64()
        .expect("proven covered");
    let exec = cov["proven_executed_leaves"]["covered"]
        .as_i64()
        .expect("proven_executed covered");
    let asserted = cov["proven_asserted_leaves"]["covered"]
        .as_i64()
        .expect("proven_asserted covered");
    assert_eq!(
        exec + asserted,
        proven,
        "proven must partition cleanly into executed + asserted-only: proven={proven} exec={exec} assert={asserted}"
    );
    // The committed graph has manual_check/empty-command passes, so the
    // discriminator must land some leaves in asserted (proves manual_check is
    // NOT counted as executed). If this breaks, the discriminator regressed.
    assert!(
        asserted > 0,
        "the committed graph has hand-marked proofs — asserted must be > 0, got {asserted} (proven={proven} exec={exec})"
    );
    // The compass discloses the split inline whenever there is proven to inspect.
    let human = run_text_as(&graph.root, &["status"], "llm:validator");
    assert!(
        human.contains("exec ") && human.contains("assert "),
        "the compass must disclose executed-vs-asserted inline: {human}"
    );
}

#[test]
fn sqlite_proven_axis_discriminates_manual_check_from_executed_test() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-proven-discriminator");
    run_json(&graph.root, &["init", ".", "--json"]);

    // Root with two implemented leaves: A proven ONLY by a manual_check
    // hand-mark (empty command, never run → ASSERTED), B proven by a test the
    // EXECUTOR actually runs (last_executed_run stamped → EXECUTED). proven must
    // be 2, executed 1 (B), asserted 1 (A). A hand-mark on a command-bearing
    // proof is ASSERTED, not EXECUTED — the executor must have run it.
    let root = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "root",
            "--description",
            "root",
            "--level",
            "system",
            "--json",
        ],
        "llm:builder",
    );
    let root_id = root["id"].as_str().expect("root id");
    let leaf_a = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "leaf A manual proof",
            "--description",
            "leaf proven only by a manual check",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    let leaf_a_id = leaf_a["id"].as_str().expect("leaf A id");
    let leaf_b = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "leaf B executed test",
            "--description",
            "leaf proven by a runnable test",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    let leaf_b_id = leaf_b["id"].as_str().expect("leaf B id");
    run_json_as(
        &graph.root,
        &["edge", "hierarchy", root_id, leaf_a_id, "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["edge", "hierarchy", root_id, leaf_b_id, "--json"],
        "llm:builder",
    );

    // Codefiles must exist on disk (codefile add checks mtime); register + ground.
    write_scratch_file(&graph.root, "src/a.rs", "pub fn a() {}\n");
    write_scratch_file(&graph.root, "src/b.rs", "pub fn b() {}\n");
    run_json_as(
        &graph.root,
        &["codefile", "add", "src/a.rs", "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "src/b.rs", "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["edge", "implement", leaf_a_id, "src/a.rs", "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["edge", "implement", leaf_b_id, "src/b.rs", "--json"],
        "llm:builder",
    );

    // A: manual_check with an EMPTY command — hand-marked acceptance. Asserted.
    let vmanual = run_json(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "manual acceptance for A",
            "--type",
            "manual_check",
            "--command",
            "",
            "--json",
        ],
    );
    let vmanual_id = vmanual["id"].as_str().expect("manual validation id");
    run_json(
        &graph.root,
        &[
            "validation",
            "mark",
            vmanual_id,
            "--result",
            "passed",
            "--evidence",
            "manual sign-off recorded by reviewer",
            "--json",
        ],
    );
    run_json(
        &graph.root,
        &["edge", "validates", vmanual_id, leaf_a_id, "--json"],
    );

    // B: a runnable test — EXECUTED proof. We actually RUN it via the validator
    // lane (`loom validate` runs the command and stamps last_executed_run), NOT
    // a hand-mark. This is the crux of the proven-axis honesty fix: a hand-mark
    // on a command-bearing proof is ASSERTED, not EXECUTED — you cannot buy
    // 'executed' by typing a command and marking it passed; the executor must
    // have run it. (A trivially-passing command keeps the test hermetic.)
    let vtest = run_json(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "automated test for B",
            "--type",
            "test",
            "--command",
            "test -f src/b.rs",
            "--json",
        ],
    );
    let vtest_id = vtest["id"].as_str().expect("test validation id");
    run_json(
        &graph.root,
        &["edge", "validates", vtest_id, leaf_b_id, "--json"],
    );
    // The executor runs it — last_executed_run is stamped, so B reads EXECUTED.
    run_json(&graph.root, &["validate", leaf_b_id, "--json"]);

    let status = run_json(&graph.root, &["status", "--json"]);
    let cov = coverage_proven(&status);
    let proven = cov["proven_leaves"]["covered"].as_i64().expect("proven");
    let exec = cov["proven_executed_leaves"]["covered"]
        .as_i64()
        .expect("executed");
    let asserted = cov["proven_asserted_leaves"]["covered"]
        .as_i64()
        .expect("asserted");
    assert_eq!(proven, 2, "both leaves are proven: {status}");
    assert_eq!(
        exec, 1,
        "only leaf B (runnable test) is executed-proven: {status}"
    );
    assert_eq!(
        asserted, 1,
        "only leaf A (manual_check, empty command) is asserted-only: {status}"
    );
    assert_eq!(
        exec + asserted,
        proven,
        "invariant: executed + asserted-only == proven"
    );
}
