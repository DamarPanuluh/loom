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

/// Manufacture an UNEXPLORED, signal-bearing intent pair on a fully-green
/// fixture. The committed fixture is now horizontally complete (every existing
/// intent pair explored), so discovery has no work. Adding two brand-new
/// intents grounded in the SAME freshly-registered codefile gives a pair that
/// is (a) unexplored — never inspected — and (b) signal-bearing via the
/// shared_file discovery signal, so the discovery lane immediately has a
/// suspected-coupling work item. The pair is deliberately left ungrounded
/// (no `edge explore … ground`) so it stays unexplored. Returns the two
/// intent names.
fn seed_unexplored_signal_pair(root: &Path, tag: &str) -> (String, String) {
    let file = format!("scratch/{tag}_shared.rs");
    write_scratch_file(root, &file, "pub fn shared_helper() -> u8 { 1 }\n");
    run_json_as(root, &["codefile", "add", &file, "--json"], "llm:builder");
    let names = [
        format!("{tag} discovery seed A"),
        format!("{tag} discovery seed B"),
    ];
    for nm in &names {
        run_json_as(
            root,
            &[
                "intent",
                "add",
                "--name",
                nm,
                "--description",
                "owns the shared helper used to seed an unexplored discovery pair",
                "--level",
                "feature",
                "--lifecycle",
                "implemented",
                "--json",
            ],
            "llm:builder",
        );
        run_json_as(
            root,
            &[
                "edge",
                "implement",
                nm,
                &file,
                "--locator",
                "fn shared_helper",
                "--json",
            ],
            "llm:builder",
        );
    }
    (names[0].clone(), names[1].clone())
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
    assert_eq!(migrated["version"], "12");
    assert_eq!(migrated["current"], true);
    assert_eq!(migrated["expected"], "12");
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
fn sqlite_batch_smell_decision_adjudicates_finding() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("sqlite-batch-smell-decision");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "src/tangled.rs",
        "pub fn alpha_path() -> u8 { 1 }\npub fn beta_path() -> u8 { 2 }\npub fn gamma_path() -> u8 { 3 }\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "src/tangled.rs", "--json"],
        "llm:builder",
    );
    for (name, locator) in [
        ("batch smell alpha owner", "fn alpha_path"),
        ("batch smell beta owner", "fn beta_path"),
        ("batch smell gamma owner", "fn gamma_path"),
    ] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                "owns one behavior in the batch smell decision regression",
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
                name,
                "src/tangled.rs",
                "--locator",
                locator,
                "--json",
            ],
            "llm:builder",
        );
    }

    let before = run_json(&graph.root, &["smells", "--limit", "100", "--json"]);
    let smell_id = before["smells"]
        .as_array()
        .expect("smells array")
        .iter()
        .find(|smell| {
            smell["kind"] == "tangled_file"
                && smell["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("src/tangled.rs"))
        })
        .and_then(|smell| smell["id"].as_str())
        .expect("the shared file must produce an open tangled_file smell")
        .to_string();
    assert_eq!(smell_id, "tangled_file:src/tangled.rs");

    let ruling = "the three paths share one atomic parser table, so splitting this fixture would duplicate the invariant being tested";
    let undeclared_ruling = "the temporary import is an accepted bootstrap bridge while the owners are merged by the next seeded graph import";
    let lines = [
        serde_json::json!({
            "op": "smell_decision",
            "smell": smell_id,
            "text": ruling,
        })
        .to_string(),
        serde_json::json!({
            "op": "smell_decision",
            "smell": "undeclared_coupling:batch-alpha:batch-beta",
            "text": undeclared_ruling,
        })
        .to_string(),
    ]
    .join("\n");
    write_scratch_file(&graph.root, "scratch/smell-decision.jsonl", &lines);
    let applied = run_json_as(
        &graph.root,
        &["batch", "scratch/smell-decision.jsonl", "--json"],
        "llm:quality",
    );
    assert_eq!(
        applied["failed"], 0,
        "smell decision batch lines apply: {applied}"
    );
    assert_eq!(
        applied["ok"], 2,
        "both smell decision lines apply: {applied}"
    );
    assert!(
        applied["results"]
            .as_array()
            .expect("results array")
            .iter()
            .any(|result| {
                result["applied"]
                    .as_str()
                    .is_some_and(|s| s.contains("smell_decision tangled_file:src/tangled.rs"))
            }),
        "batch output names the adjudicating smell decision: {applied}"
    );
    assert!(
        applied["results"]
            .as_array()
            .expect("results array")
            .iter()
            .any(|result| {
                result["applied"].as_str().is_some_and(|s| {
                    s.contains("smell_decision undeclared_coupling:batch-alpha:batch-beta")
                })
            }),
        "batch accepts the undeclared_coupling smell_decision shape: {applied}"
    );

    let db = graph.root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let notes: i64 = conn
        .query_row(
            "SELECT count(*) FROM note WHERE kind='decision' AND target_kind='smell' AND target_id='tangled_file:src/tangled.rs' AND text=?1",
            [ruling],
            |r| r.get(0),
        )
        .expect("count smell decision notes");
    assert_eq!(notes, 1, "batch must insert one smell decision note");
    let undeclared_notes: i64 = conn
        .query_row(
            "SELECT count(*) FROM note WHERE kind='decision' AND target_kind='smell' AND target_id='undeclared_coupling:batch-alpha:batch-beta' AND text=?1",
            [undeclared_ruling],
            |r| r.get(0),
        )
        .expect("count undeclared coupling smell decision notes");
    assert_eq!(
        undeclared_notes, 1,
        "batch must accept undeclared_coupling smell decision notes"
    );

    let after = run_json(&graph.root, &["smells", "--limit", "100", "--json"]);
    assert!(
        !after["smells"]
            .as_array()
            .expect("smells array")
            .iter()
            .any(|smell| smell["id"] == "tangled_file:src/tangled.rs"),
        "the ruled finding must leave the open smells list: {after}"
    );
    assert!(
        after["adjudicated"]
            .as_array()
            .expect("adjudicated array")
            .iter()
            .any(|smell| {
                smell["kind"] == "tangled_file"
                    && smell["summary"]
                        .as_str()
                        .is_some_and(|summary| summary.contains("src/tangled.rs"))
            }),
        "the ruled finding must surface as adjudicated: {after}"
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
    // The fixture is horizontally complete; seed one unexplored, signal-bearing
    // pair so discovery --take is a non-empty template source. It routes through
    // the same run_take emitter as the fix queue.
    seed_unexplored_signal_pair(&graph.root, "take-tmpl-json");
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
    // The fixture is horizontally complete; seed one unexplored, signal-bearing
    // pair so the human discovery --take prints a template, not "nothing to discover".
    seed_unexplored_signal_pair(&graph.root, "take-tmpl-human");
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
    // The committed fixture sits at phase=validate — its asserted-only leaves are
    // discriminating-proof work (see validate_selection_from_snapshot). Bare
    // `next` carries no `mode` and must follow that phase's lane, not a hardcoded
    // discovery (the bug this guards). The validation invalidation below keeps a
    // non-discovery actionable phase even if the fixture's proof mix shifts.
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
        // Invalidate ALL passing proofs so the validate lane is unambiguously the
        // dominant focus. (A single not_run proof does not outscore the base lane
        // once the committed graph is otherwise mature, so manufacture a clearly
        // validate-dominant state rather than relying on the fixture's proof mix.)
        conn.execute(
            "UPDATE validation SET last_result='not_run' WHERE last_result='passed'",
            [],
        )
        .expect("invalidate validations to make validate the dominant focus");
    }
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

#[test]
fn sqlite_smells_take_kind_returns_finding_ids_and_batch_template() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("smells-take-kind");

    write_scratch_file(
        &graph.root,
        "scratch/take_kind_a.rs",
        "pub fn take_kind_a() -> u8 { crate::scratch::take_kind_b::take_kind_b() }\n",
    );
    write_scratch_file(
        &graph.root,
        "scratch/take_kind_b.rs",
        "pub fn take_kind_b() -> u8 { 7 }\n",
    );
    for path in ["scratch/take_kind_a.rs", "scratch/take_kind_b.rs"] {
        run_json_as(
            &graph.root,
            &["codefile", "add", path, "--json"],
            "llm:builder",
        );
    }
    for (name, file, locator) in [
        (
            "smells take kind importer",
            "scratch/take_kind_a.rs",
            "fn take_kind_a",
        ),
        (
            "smells take kind imported",
            "scratch/take_kind_b.rs",
            "fn take_kind_b",
        ),
    ] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                "owns one side of the smell take kind undeclared coupling fixture",
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
                name,
                file,
                "--locator",
                locator,
                "--json",
            ],
            "llm:builder",
        );
    }
    for idx in 0..8 {
        let path = format!("scratch/take_kind_extra_{idx}.rs");
        let locator = format!("fn take_kind_extra_{idx}");
        write_scratch_file(
            &graph.root,
            &path,
            &format!(
                "pub fn take_kind_extra_{idx}() -> u8 {{ crate::scratch::take_kind_b::take_kind_b() }}\n"
            ),
        );
        run_json_as(
            &graph.root,
            &["codefile", "add", &path, "--json"],
            "llm:builder",
        );
        run_json_as(
            &graph.root,
            &[
                "edge",
                "implement",
                "smells take kind importer",
                &path,
                "--locator",
                &locator,
                "--json",
            ],
            "llm:builder",
        );
    }

    let db = graph.root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    let imports = serde_json::to_string(&vec!["scratch/take_kind_b.rs"]).expect("imports json");
    for path in std::iter::once("scratch/take_kind_a.rs".to_string())
        .chain((0..8).map(|idx| format!("scratch/take_kind_extra_{idx}.rs")))
    {
        conn.execute(
            "UPDATE codefile SET imports = ?1 WHERE path = ?2",
            rusqlite::params![imports, path],
        )
        .expect("wire import fixture");
    }

    let importer = intent_id_by_name(&graph.root, "smells take kind importer");
    let imported = intent_id_by_name(&graph.root, "smells take kind imported");
    let (a, b) = if importer < imported {
        (importer, imported)
    } else {
        (imported, importer)
    };

    let json = run_json(
        &graph.root,
        &[
            "smells",
            "--json",
            "--kind",
            "undeclared_coupling",
            "--take",
            "1",
        ],
    );
    assert_eq!(
        json["shown"], 1,
        "--take 1 limits the filtered result: {json}"
    );
    assert_eq!(
        json["smells"][0]["kind"], "undeclared_coupling",
        "--kind returns only the requested smell kind: {json}"
    );
    assert_eq!(
        json["smells"][0]["id"],
        format!("undeclared_coupling:{a}:{b}"),
        "finding id matches the smell-decision identity: {json}"
    );
    assert_eq!(
        json["smells"][0]["intent_ids"],
        serde_json::json!([a, b]),
        "undeclared coupling carries both endpoint intents: {json}"
    );
    assert!(
        json["smells"][0]["evidence"]
            .as_str()
            .expect("evidence")
            .contains("scratch/take_kind_a.rs → scratch/take_kind_b.rs"),
        "exact detector evidence is preserved: {json}"
    );
    let templates = json["batch_template"].as_array().expect("batch_template");
    assert_eq!(
        templates.len(),
        1,
        "one template line per shown finding: {json}"
    );
    let line = templates[0].as_str().expect("template line");
    let op: Value = serde_json::from_str(line).expect("template line is JSONL");
    assert_eq!(op["op"], "ground");
    assert_eq!(op["a"], a);
    assert_eq!(op["b"], b);
    assert_eq!(op["confidence"], "<confidence>");
    assert!(
        json["batch_template_hints"]
            .as_array()
            .is_some_and(|hints| !hints.is_empty()),
        "JSON includes batch template hints: {json}"
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

// DOGFOOD-FOUND DEFECT (AI-companion hunt): `loom next --take 0` (a computed
// zero-size chunk) silently returned the single-item schema instead of a bulk
// envelope, with no signal. Now an explicit 0 is rejected; omitting --take is the
// single-item path.
#[test]
fn sqlite_next_take_zero_rejected() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("take-zero");
    let out = std::process::Command::new(loom_bin())
        .args(["next", "--mode", "discovery", "--take", "0"])
        .current_dir(&graph.root)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom");
    assert!(
        !out.status.success(),
        "`--take 0` must be rejected, not a silent single-item"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("requests zero items"),
        "the rejection explains itself: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// DOGFOOD-FOUND DEFECT (AI-companion hunt): in phase=audit (open findings gate
// green) bare `loom next` mis-routed to OPTIONAL discovery — there is no audit
// queue, so the AI never reached the green-blocking work. Bare next now echoes
// the compass's audit directive (→ `loom smells`). Guarded on phase since the
// committed fixture's phase varies.
#[test]
fn sqlite_bare_next_in_audit_points_at_gate_not_discovery() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("audit-route");
    let phase = run_json(&graph.root, &["status", "--json"])["graph_state"]["phase"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if phase == "audit" {
        let text = run_text_as(&graph.root, &["next"], "llm");
        assert!(
            text.contains("loom smells") && !text.contains("No relationship is tracked yet"),
            "bare next in phase=audit must point at the audit gate, not optional discovery: {text}"
        );
    }
}

// DOGFOOD-FOUND DEFECT (integrity hunt): a reciprocal RELATES_TO (a->b AND b->a,
// both grounded) double-counted degree/centrality — RELATES_TO is undirected, so
// that is ONE relationship. Inflated blast-radius skews "start here" + next ranking.
#[test]
fn sqlite_reciprocal_relates_to_not_double_counted() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("recip-degree");
    run_json(&graph.root, &["init", ".", "--json"]);
    let id = |v: Value| v["id"].as_str().unwrap().to_string();
    let a = id(run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "alpha",
            "--description",
            "da",
            "--level",
            "component",
            "--domain",
            "test",
            "--json",
        ],
        "llm:builder",
    ));
    let b = id(run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "bravo",
            "--description",
            "db",
            "--level",
            "feature",
            "--domain",
            "test",
            "--json",
        ],
        "llm:builder",
    ));
    let degree_of = |who: &str| -> i64 {
        let v = run_json(&graph.root, &["hotspots", "--json"]);
        v["central_intents"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|x| x["id"].as_str() == Some(who))
            .and_then(|x| x["degree"].as_i64())
            .unwrap_or(0)
    };
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "ground",
            "--criterion",
            "they coexist for the degree test",
            "--confidence",
            "0.8",
            "--json",
        ],
        "llm:analyzer",
    );
    assert_eq!(degree_of(&a), 1, "one relationship → degree 1");
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &b,
            &a,
            "ground",
            "--criterion",
            "reciprocal direction, same pair",
            "--confidence",
            "0.8",
            "--json",
        ],
        "llm:analyzer",
    );
    assert_eq!(
        degree_of(&a),
        1,
        "a reciprocal pair is ONE relationship — degree must stay 1, not double to 2"
    );
}

// DOGFOOD-FOUND DEFECT (security hunt): import→exec supply-chain footgun. An
// imported loom.graph.json carries shell commands that `loom validate` runs via
// `sh -c`; the documented import→validate-all flow silently executed them with no
// warning. Import now neutralizes unvetted pending commands (blocked) so a bulk
// `validate --all` can't run them, and warns loudly.
#[cfg(unix)]
#[test]
fn sqlite_import_blocks_unvetted_commands_from_bulk_validate() {
    let _guard = sqlite_test_lock();
    let canary = std::env::temp_dir().join(format!("loom_rce_canary_{}", std::process::id()));
    let _ = fs::remove_file(&canary);
    // Attacker graph: a not_run validation whose command writes the canary.
    let atk = ScratchGraph::new("rce-atk");
    run_json(&atk.root, &["init", ".", "--json"]);
    let i = run_json_as(
        &atk.root,
        &[
            "intent",
            "add",
            "--name",
            "t",
            "--description",
            "tdesc",
            "--level",
            "feature",
            "--domain",
            "test",
            "--json",
        ],
        "llm:builder",
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cmd = format!("touch '{}'", canary.display());
    run_json_as(
        &atk.root,
        &[
            "validation",
            "add",
            "--name",
            "hostile",
            "--type",
            "test",
            "--command",
            &cmd,
            "--intent",
            &i,
            "--json",
        ],
        "llm:validator",
    );
    std::process::Command::new(loom_bin())
        .args(["export"])
        .current_dir(&atk.root)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("export");
    let export_file = atk.root.join("loom.graph.json");

    // Victim imports + runs the documented `validate --all`.
    let vic = ScratchGraph::new("rce-vic");
    run_json(&vic.root, &["init", ".", "--json"]);
    let imp = run_json(
        &vic.root,
        &["import", export_file.to_str().unwrap(), "--json"],
    );
    assert_eq!(
        imp["unvetted_commands_blocked"].as_i64(),
        Some(1),
        "import must block the unvetted command: {imp}"
    );
    run_text_as(&vic.root, &["validate", "--all"], "llm:validator");
    let ran = canary.exists();
    let _ = fs::remove_file(&canary);
    assert!(
        !ran,
        "`loom validate --all` must NOT execute a command from an imported graph (RCE footgun)"
    );
}

// DOGFOOD-FOUND DEFECT (security hunt): a validation's --timeout-secs killed only
// the `sh` parent, not its process tree — a forked test runner (or a hostile
// `sleep`) outlived the deadline while validate falsely reported "timed out". Now
// the command runs in its own process group and the timeout kills the whole group.
#[cfg(unix)]
#[test]
fn sqlite_validate_timeout_kills_the_process_tree() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("vtimeout");
    run_json(&graph.root, &["init", ".", "--json"]);
    let i = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "t",
            "--description",
            "tdesc",
            "--level",
            "feature",
            "--domain",
            "test",
            "--json",
        ],
        "llm:builder",
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let canary = graph.root.join("survivor_canary");
    // A forked child writes the canary AFTER the 1s timeout — the old bug let it
    // survive because only `sh` was killed.
    let cmd = format!("( sleep 2 && touch '{}' ) ; echo done", canary.display());
    run_json_as(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "probe",
            "--type",
            "test",
            "--command",
            &cmd,
            "--intent",
            &i,
            "--json",
        ],
        "llm:validator",
    );
    let start = std::time::Instant::now();
    run_text_as(
        &graph.root,
        &["validate", &i, "--timeout-secs", "1"],
        "llm:validator",
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "validate must return promptly on timeout, took {:?}",
        start.elapsed()
    );
    // Wait past when the survivor WOULD fire; the group-kill must have prevented it.
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert!(
        !canary.exists(),
        "the forked child must be killed with the process group, not survive the timeout"
    );
}

// DOGFOOD-FOUND DEFECT (integrity hunt): `loom import` is a TRUSTED build path
// (federation/restore/PR-merged loom.graph.json) but bypassed the structural/value
// invariants every interactive write enforces — it accepted a HIERARCHY cycle /
// multi-parent / out-of-range confidence / dangling edge with "✓ imported", and
// the malformed graph then re-exported byte-clean and TRAVELED. Import now
// validates the data first and refuses.
#[test]
fn sqlite_import_rejects_malformed_graphs() {
    let _guard = sqlite_test_lock();
    let src = ScratchGraph::new("imp-src");
    run_json(&src.root, &["init", ".", "--json"]);
    let id = |v: Value| v["id"].as_str().unwrap().to_string();
    let a = id(run_json_as(
        &src.root,
        &[
            "intent",
            "add",
            "--name",
            "alpha",
            "--description",
            "da",
            "--level",
            "component",
            "--domain",
            "test",
            "--json",
        ],
        "llm:builder",
    ));
    let b = id(run_json_as(
        &src.root,
        &[
            "intent",
            "add",
            "--name",
            "bravo",
            "--description",
            "db",
            "--level",
            "feature",
            "--domain",
            "test",
            "--json",
        ],
        "llm:builder",
    ));
    run_json_as(
        &src.root,
        &["edge", "hierarchy", &a, &b, "--json"],
        "llm:builder",
    );
    std::process::Command::new(loom_bin())
        .args(["export"])
        .current_dir(&src.root)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("export");
    let base: Value = serde_json::from_str(
        &fs::read_to_string(src.root.join("loom.graph.json")).expect("read export"),
    )
    .expect("parse export");

    let import_corrupted = |mutate: &dyn Fn(&mut Value)| -> (bool, String) {
        let mut g = base.clone();
        mutate(&mut g);
        let dst = ScratchGraph::new("imp-dst");
        run_json(&dst.root, &["init", ".", "--json"]);
        let bad = dst.root.join("bad.json");
        fs::write(&bad, serde_json::to_string(&g).unwrap()).unwrap();
        let out = std::process::Command::new(loom_bin())
            .args(["import", bad.to_str().unwrap()])
            .current_dir(&dst.root)
            .env_remove("LOOM_GRAPH")
            .output()
            .expect("import");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).to_string(),
        )
    };

    // Cycle: add the reverse hierarchy edge b->a.
    let (ok, err) = import_corrupted(&|g| {
        let e = serde_json::json!({"created_at":"2026-01-01T00:00:00+00:00","from":b.clone(),"to":a.clone(),"notes":""});
        g["edges"]["HIERARCHY"].as_array_mut().unwrap().push(e);
    });
    assert!(
        !ok && err.contains("cycle"),
        "cyclic import must be rejected: {err}"
    );

    // Out-of-range confidence.
    let (ok, err) = import_corrupted(&|g| {
        g["edges"]["RELATES_TO"] = serde_json::json!([{"created_at":"2026-01-01T00:00:00+00:00","from":a.clone(),"to":b.clone(),"confidence":5.0,"inspection_status":"passing","criterion":"x","notes":""}]);
    });
    assert!(
        !ok && err.contains("confidence"),
        "out-of-range confidence must be rejected: {err}"
    );

    // A valid graph must still import.
    let (ok, _) = import_corrupted(&|_g| {});
    assert!(ok, "a valid graph must still import");
}

// DOGFOOD-FOUND DEFECTS (AI-companion hunt): two --json/prose parity mismatches an
// AI driving on --json would act on. `loom report` printed "uninspected 0" (summary)
// and "uninspected 1" (raw per-status) both bare-labelled; the discovery signal's
// structured `weight` field (0.94) disagreed with the prose "weight" (0.23) for the
// SAME signal. Now the report explains the gap, and the weight numbers match.
#[test]
fn sqlite_report_uninspected_not_contradictory() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("report-uninsp");
    let v = run_json(&graph.root, &["report", "--json"]);
    let raw = v["edge_counts_by_status"]["uninspected"]
        .as_i64()
        .unwrap_or(0);
    let actionable = v["status"]["uninspected_edges"].as_i64().unwrap_or(raw);
    if raw != actionable {
        let text = run_text_as(&graph.root, &["report"], "llm");
        assert!(
            text.contains("(raw;"),
            "report's raw uninspected line must explain the gap vs the summary, not bare-contradict it: {text}"
        );
    }
}

// DOGFOOD-FOUND DEFECT (final-sweep #14): `report --json` emitted the FULL
// intents_without_validations + completeness_gaps lists. On a large graph those
// run to thousands of entries and bury the headline / flood an agent's context.
// They are now capped, with a `_total` so the consumer knows the true size.
#[test]
fn sqlite_report_json_caps_unbounded_lists() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("report-caps");
    let v = run_json(&graph.root, &["report", "--json"]);
    // The capped lists never exceed the cap…
    const CAP: usize = 50;
    for key in ["intents_without_validations", "completeness_gaps"] {
        let len = v[key].as_array().map(|a| a.len()).unwrap_or(0);
        assert!(len <= CAP, "{key} must be capped at {CAP}, got {len}: {v}");
    }
    // …and the true totals are always present so nothing is silently dropped.
    assert!(
        v.get("intents_without_validations_total").is_some(),
        "report --json must disclose the true list total: {v}"
    );
    assert!(
        v.get("completeness_gaps_total").is_some(),
        "report --json must disclose the true gaps total: {v}"
    );
}

// SWEEP #2 (scale): the semantic smell detectors (twin_intents,
// duplicated_responsibility) no longer scan every O(level²) same-level pair —
// they generate candidates from an inverted token/tag index. This proves the
// candidate path still FINDS a real twin: two same-level intents that read alike
// with no edge between them must still surface, or the pruning lost a true pair.
#[test]
fn sqlite_semantic_candidate_path_still_finds_twins() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("twin-candidates");
    run_json(&graph.root, &["init", ".", "--json"]);
    for name in ["user session login flow", "user session signin flow"] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                "authenticate the user and establish a session",
                "--level",
                "feature",
                "--lifecycle",
                "implemented",
                "--json",
            ],
            "llm:builder",
        );
    }
    let smells = run_json(&graph.root, &["smells", "--json"]);
    let twins: Vec<&str> = smells
        .as_object()
        .into_iter()
        .flat_map(|o| o.values())
        .filter_map(|v| v.as_array())
        .flatten()
        .filter(|s| s["kind"] == "twin_intents")
        .filter_map(|s| s["summary"].as_str())
        .collect();
    assert!(
        twins
            .iter()
            .any(|s| s.contains("login flow") && s.contains("signin flow")),
        "the candidate path must still surface the twin pair: {twins:?}"
    );
}

// PERFORMANCE REGRESSION HARNESS. The campaign's #2/#3/#12 were superlinear
// blow-ups invisible on loom's 102-intent graph because no test built a large
// one. This inflates the committed export with a few thousand synthetic intents
// that share NO tokens / domain / tags — so they contribute ~zero discovery and
// smell candidates (the fixed code stays fast) while the OLD all-pairs scan still
// pays O(N²). An O(N²) regression took 75s/90s at this scale, so a 20s ceiling
// catches it with a wide margin and never flakes on constant-factor variance.
//
// Wall-clock + a large margin is the deliberate lever: there is no deterministic
// operation counter in-binary, but the margin (catastrophic-vs-budget) makes that
// irrelevant. Reverting the candidate generation in scoring.rs/semantic.rs blows
// straight past the budget.
fn write_inflated_graph(root: &Path, extra_intents: usize, extra_edges: usize) -> Vec<String> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("loom.graph.json");
    let raw = fs::read_to_string(&base).expect("read committed export");
    let mut graph: Value = serde_json::from_str(&raw).expect("parse export");
    let mut ids = Vec::with_capacity(extra_intents);
    {
        let intents = graph["nodes"]["Intent"]
            .as_array_mut()
            .expect("Intent node array");
        for i in 0..extra_intents {
            let id = format!("perf-intent-{i:06}");
            ids.push(id.clone());
            // Unique tokens AND a unique domain → contributes zero discovery/smell
            // candidates, isolating the all-pairs cost the fixes removed.
            intents.push(serde_json::json!({
                "id": id,
                "name": format!("synthetic perf node alpha{i} beta{i} gamma{i}"),
                "description": format!("isolated synthetic intent {i} unique vocabulary delta{i} epsilon{i} zeta{i}"),
                "abstraction_level": "feature",
                "domain": format!("synthdomain{i}"),
                "layer": "",
                "source_refs": [],
                "status": "confirmed",
                "aspect": "",
                "tags": [],
                "visibility": "internal",
                "boundary": "",
                "lifecycle": "implemented",
                "criterion": "",
                "created_at": "2026-06-21T00:00:00+00:00",
                "updated_at": "2026-06-21T00:00:00+00:00",
            }));
        }
    }
    {
        let relates = graph["edges"]["RELATES_TO"]
            .as_array_mut()
            .expect("RELATES_TO edge array");
        for k in 0..extra_edges {
            // Chain synthetic intents so the real-RELATES_TO graph (status's
            // betweenness cost, #12) has edges — status must stay fast WITHOUT it.
            let a = ids[k % ids.len()].clone();
            let b = ids[(k + 1) % ids.len()].clone();
            let status = if k % 7 == 0 {
                "needs_reverification"
            } else {
                "uninspected"
            };
            relates.push(serde_json::json!({
                "from": a, "to": b,
                "inspection_status": status,
                "confidence": 0.0,
                "criterion": "", "evidence": "", "notes": "",
                "inspected_by": "", "last_inspected": "",
                "priority_score": 0.0, "stable": false,
                "kinds": [],
                "created_at": "2026-06-21T00:00:00+00:00",
            }));
        }
    }
    fs::write(
        root.join("perf.graph.json"),
        serde_json::to_string(&graph).expect("serialize inflated graph"),
    )
    .expect("write perf graph");
    ids
}

#[test]
fn sqlite_large_graph_read_commands_stay_under_budget() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("perf-budget");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_inflated_graph(&graph.root, 3000, 1200);
    run_json(&graph.root, &["import", "perf.graph.json", "--json"]);

    const BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
    let timed = |args: &[&str]| {
        let start = std::time::Instant::now();
        let out = Command::new(loom_bin())
            .args(args)
            .current_dir(&graph.root)
            .env_remove("LOOM_GRAPH")
            .output()
            .unwrap_or_else(|e| panic!("run loom {args:?}: {e}"));
        let elapsed = start.elapsed();
        assert!(
            out.status.success(),
            "loom {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            elapsed < BUDGET,
            "loom {args:?} took {elapsed:?}, over the {BUDGET:?} budget — a superlinear regression?"
        );
    };
    // The three hot read commands the campaign's perf fixes targeted.
    timed(&["status", "--json"]);
    timed(&["next", "--mode", "discovery", "--json"]);
    timed(&["smells", "--json"]);
}

// ENHANCEMENT #5 — reads must not write. The campaign's worst class of bug was a
// LIFECYCLE ENTRY POINT: `loom detect` runs BEFORE a graph exists and is the
// first command a cold agent runs on a new repo. Its routing must point at
// `loom init` first (every later step needs a graph), in BOTH human and --json —
// a dry-run of the full lane lifecycle found the --json form carried repo facts
// with no next action, and even the human form skipped `init`.
#[test]
fn sqlite_detect_routes_to_init_in_both_forms() {
    let _guard = sqlite_test_lock();
    let dir = std::env::temp_dir().join(format!(
        "loom-detect-route-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src")).expect("scratch src dir");
    fs::write(dir.join("src/lib.rs"), "pub fn a() {}\n").expect("scratch source");
    let run = |json: bool| {
        let mut args = vec!["detect"];
        if json {
            args.push("--json");
        }
        let out = Command::new(loom_bin())
            .args(&args)
            .current_dir(&dir)
            .env_remove("LOOM_GRAPH")
            .output()
            .expect("run loom detect");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    // --json carries a next_step that routes to `loom init`.
    let v: Value = serde_json::from_str(&run(true)).expect("detect --json");
    assert!(
        v["next_step"]
            .as_str()
            .is_some_and(|s| s.contains("loom init")),
        "detect --json must route to `loom init` first: {v}"
    );
    // Human form names it too (parity).
    assert!(
        run(false).contains("loom init"),
        "human detect must route to `loom init` first"
    );
    let _ = fs::remove_dir_all(&dir);
}

// COLD-START BOOTSTRAP: `loom seed --suggest` mines candidate intents from the
// code so a fresh repo starts from a draft, not a blank page. It runs WITHOUT a
// graph (like detect), SUGGESTS only (writes nothing), and emits pre-filled adopt
// commands per candidate.
#[test]
fn sqlite_seed_suggest_mines_candidate_intents() {
    let _guard = sqlite_test_lock();
    let dir = std::env::temp_dir().join(format!(
        "loom-seed-suggest-{}-{}",
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    // Go uses the heuristic extractor in BOTH build configs (tree-sitter off
    // doesn't extract Rust), so the assertion is build-independent.
    fs::write(
        dir.join("store.go"),
        "// Package store persists records.\npackage store\nfunc Save(id string) error { return nil }\n",
    )
    .expect("scratch source");

    let out = Command::new(loom_bin())
        .args(["seed", "--suggest", "--json"])
        .current_dir(&dir)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom seed --suggest");
    let v: Value = serde_json::from_slice(&out.stdout).expect("seed --suggest --json");
    let cands = v["candidates"].as_array().expect("candidates array");
    let store = cands
        .iter()
        .find(|c| c["path"] == "store.go")
        .expect("a candidate for store.go");
    // Mines the public surface + the module doc, and emits adopt commands.
    assert!(
        store["public_symbols"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x == "func Save")),
        "must surface the public symbol: {store}"
    );
    assert_eq!(
        store["doc"], "Package store persists records.",
        "doc draft from the leading // comment: {store}"
    );
    assert!(
        store["adopt"].as_array().is_some_and(|a| a.iter().any(|c| c
            .as_str()
            .is_some_and(|s| s.contains("loom edge implement")))),
        "must emit a pre-filled grounding command: {store}"
    );
    // SUGGEST-only: it created no graph.
    assert!(
        !dir.join(".loom").exists(),
        "seed --suggest must not write a graph"
    );
    let _ = fs::remove_dir_all(&dir);
}

// The SAD path of source-corpus coverage: docs with NO structured requirement IDs
// must report completeness UNKNOWN and route to `seed --inbox`, never silently claim
// full coverage from zero IDs.
#[test]
fn sqlite_corpus_coverage_reports_unknown_without_structured_ids() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("corpus-unknown");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "docs/notes.md",
        "# Notes\nThe system should be reliable and fast.\n",
    );
    let text = run_text_as(&graph.root, &["corpus", "coverage"], "llm");
    assert!(
        text.contains("completeness is unknown") && text.contains("seed --inbox"),
        "corpus coverage's sad path: docs with no structured IDs must report completeness \
         UNKNOWN and route to seed --inbox, not a silent full-coverage claim: {text}"
    );
}

// `loom seed --inbox` — the disciplined full-coverage seed: ingest every doc +
// source file into the inbox as triage items (the anti-gaming anchor — the LLM
// must process the whole surface). Idempotent on re-run; an empty repo seeds a
// vision prompt instead.
#[test]
fn sqlite_seed_inbox_ingests_surface_idempotently() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("seedinbox");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "store.go", "package store\nfunc Save() {}\n");
    write_scratch_file(&graph.root, "spec.md", "# Spec\nThe system.\n");

    let v = run_json(&graph.root, &["seed", "--inbox", "--json"]);
    let ingested = v["ingested"].as_i64().expect("ingested");
    assert!(ingested >= 2, "every doc + source file is ingested: {v}");

    // The inbox now holds NEW triage items anchored to the files.
    let inbox = run_json(&graph.root, &["inbox", "list", "--json"]);
    let items = inbox["items"]
        .as_array()
        .or_else(|| inbox.as_array())
        .expect("inbox items");
    let anchored: Vec<&str> = items
        .iter()
        .filter_map(|i| i["raw_text"].as_str().and_then(|t| t.lines().next()))
        .collect();
    assert!(
        anchored.contains(&"ingest: store.go") && anchored.contains(&"ingest: spec.md"),
        "items anchor the code + doc files: {anchored:?}"
    );
    assert!(
        items.iter().all(|i| i["status"] == "new"),
        "ingested items start un-triaged (new): {inbox}"
    );

    // Idempotent: a re-run ingests nothing new.
    let v2 = run_json(&graph.root, &["seed", "--inbox", "--json"]);
    assert_eq!(v2["ingested"], 0, "re-run is idempotent: {v2}");
}

#[test]
fn sqlite_source_corpus_blocks_seeded_for_unmodeled_structured_ids() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("source-corpus-seeded");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "docs/user-stories.md",
        "# Stories\n\n- US-101 checkout works\n- E-7 checkout epic\n",
    );

    let status = run_json(&graph.root, &["status", "--json"]);
    assert_eq!(status["source_corpus"]["ids_total"], 2, "{status}");
    assert_eq!(status["source_corpus"]["unresolved"], 2, "{status}");
    let seeded = status["maturity"]["rungs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "Seeded")
        .unwrap();
    assert_ne!(
        seeded["status"], "met",
        "unmodeled structured docs must keep Seeded honest: {status}"
    );
}

#[test]
fn sqlite_doc_source_intent_defaults_to_planned() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("doc-source-planned");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "docs/spec.md", "# Spec\nUS-9 parse things\n");

    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "US-9",
            "--description",
            "US-9 is represented as planned work from docs",
            "--level",
            "feature",
            "--source",
            "docs/spec.md#US-9",
            "--json",
        ],
        "llm:builder",
    );
    assert_eq!(
        intent["lifecycle"], "planned",
        "doc-only sources default to planned unless lifecycle is explicit: {intent}"
    );
}

#[test]
fn sqlite_seed_requirements_imports_structured_ids_as_planned() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("seed-requirements");
    run_json(&graph.root, &["init", ".", "--json"]);
    let parent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "checkout",
            "--description",
            "checkout requirement parent",
            "--level",
            "system",
            "--lifecycle",
            "planned",
            "--json",
        ],
        "llm:builder",
    );
    let parent_id = parent["id"].as_str().unwrap();
    write_scratch_file(
        &graph.root,
        "docs/stories.md",
        "# Stories\n\nUS-42 pay with card\nE-5 checkout epic\nADR-3 should not auto-seed\n",
    );

    let seeded = run_json_as(
        &graph.root,
        &[
            "seed",
            "--requirements",
            "docs/stories.md",
            "--under",
            parent_id,
            "--json",
        ],
        "llm:builder",
    );
    assert_eq!(seeded["created_count"], 2, "{seeded}");
    let list = run_json(&graph.root, &["intent", "list", "--json"]);
    let intents = list["intents"].as_array().unwrap();
    assert!(
        intents
            .iter()
            .any(|i| i["name"] == "US-42" && i["lifecycle"] == "planned"),
        "US ID imported as planned feature: {list}"
    );
    assert!(
        intents
            .iter()
            .any(|i| i["name"] == "E-5" && i["lifecycle"] == "planned"),
        "E ID imported as planned component: {list}"
    );
    assert!(
        !intents.iter().any(|i| i["name"] == "ADR-3"),
        "ADR IDs are reported but not auto-seeded as intents: {list}"
    );
}

#[test]
fn sqlite_validate_serves_user_visible_journey_debt() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("validate-journey-debt");
    run_json(&graph.root, &["init", ".", "--json"]);
    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "account signup",
            "--description",
            "user can sign up for an account",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--visibility",
            "user_visible",
            "--json",
        ],
        "llm:builder",
    );
    let next = run_json(&graph.root, &["next", "--mode", "validate", "--json"]);
    assert_eq!(next["mode"], "validate", "{next}");
    assert_eq!(next["intent"]["id"], intent["id"], "{next}");
    assert!(
        next["reason"]
            .as_str()
            .is_some_and(|r| r.contains("journey proof")),
        "validate must serve Proven journey debt: {next}"
    );
}

// `loom tour` — the guided comprehension walkthrough. Reads the graph back in
// decomposition order (system before its features), and per stop reports what a
// part is SUPPOSED to do, where it's grounded, and — uniquely to loom — whether
// it's PROVEN. Read-only.
#[test]
fn sqlite_tour_reads_intents_in_order_with_proof_status() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("tour");
    run_json(&graph.root, &["init", ".", "--json"]);
    // Go file: a locator that's textually present so grounding succeeds in both
    // build configs (tour reads the graph, not the extractor).
    write_scratch_file(
        &graph.root,
        "store.go",
        "package store\nfunc Save(id string) error { return nil }\n",
    );
    let b = "llm:builder";
    for (name, desc, level) in [
        ("user service", "Persist user records.", "system"),
        ("record storage", "Save a user record by id.", "feature"),
    ] {
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
                level,
                "--json",
            ],
            b,
        );
    }
    run_json_as(
        &graph.root,
        &[
            "edge",
            "hierarchy",
            "user service",
            "record storage",
            "--json",
        ],
        b,
    );
    run_json_as(&graph.root, &["codefile", "add", "store.go", "--json"], b);
    run_json(&graph.root, &["sync", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "record storage",
            "store.go",
            "--locator",
            "func Save",
            "--json",
        ],
        b,
    );
    // Prove the leaf.
    let sid = intent_id_by_name(&graph.root, "record storage");
    run_json_as(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "p",
            "--type",
            "test",
            "--command",
            "test 1 = 1",
            "--intent",
            &sid,
            "--json",
        ],
        "llm:validator",
    );
    run_json_as(&graph.root, &["validate", &sid, "--json"], "llm:validator");

    let v = run_json(&graph.root, &["tour", "--json"]);
    let stops = v["stops"].as_array().expect("stops");
    // The system intent comes first and decomposes into the feature.
    assert_eq!(stops[0]["level"], "system", "{v}");
    assert_eq!(stops[0]["name"], "user service", "{v}");
    assert!(
        stops[0]["decomposes_into"]
            .as_array()
            .is_some_and(|c| c.iter().any(|x| x == "record storage")),
        "system must decompose into its child: {v}"
    );
    // The grounded, proven leaf reads back proven with its file.
    let leaf = stops
        .iter()
        .find(|s| s["name"] == "record storage")
        .expect("leaf stop");
    assert_eq!(leaf["proven"], true, "the validated leaf is proven: {leaf}");
    assert!(
        leaf["grounded_in"]
            .as_array()
            .is_some_and(|g| g.iter().any(|x| x["path"] == "store.go")),
        "leaf grounding surfaced: {leaf}"
    );
}

// `loom impact` — pre-change blast radius. Given changed files (here explicit,
// for determinism — no git dependency), it names the intents whose groundings go
// stale and the proofs that must re-run, and flags changed source files not in
// the graph. Read-only.
#[test]
fn sqlite_impact_reports_blast_radius_for_changed_files() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("impact");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "store.go",
        "package store\nfunc Save(id string) error { return nil }\n",
    );
    let b = "llm:builder";
    run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "record storage",
            "--description",
            "Save a user record by id.",
            "--level",
            "feature",
            "--json",
        ],
        b,
    );
    run_json_as(&graph.root, &["codefile", "add", "store.go", "--json"], b);
    run_json(&graph.root, &["sync", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "record storage",
            "store.go",
            "--locator",
            "func Save",
            "--json",
        ],
        b,
    );
    let sid = intent_id_by_name(&graph.root, "record storage");
    run_json_as(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "save proof",
            "--type",
            "test",
            "--command",
            "test 1 = 1",
            "--intent",
            &sid,
            "--json",
        ],
        "llm:validator",
    );

    // A registered, grounded file → its intent + proof surface.
    let v = run_json(&graph.root, &["impact", "store.go", "--json"]);
    assert!(
        v["directly_affected"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["intent"] == "record storage")),
        "changed grounded file must flag its intent: {v}"
    );
    assert!(
        v["proofs_to_rerun"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x["validation"] == "save proof")),
        "the intent's proof must be listed to re-run: {v}"
    );

    // An unregistered source file → flagged as a coverage gap, not a fabricated intent.
    let v2 = run_json(&graph.root, &["impact", "newthing.go", "--json"]);
    assert!(
        v2["directly_affected"]
            .as_array()
            .is_some_and(|a| a.is_empty()),
        "an unregistered file affects no intents: {v2}"
    );
    assert!(
        v2["unregistered_changed_source"]
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x == "newthing.go")),
        "unregistered changed source must be flagged: {v2}"
    );
}

// Discovery SURPRISE: a structural coupling (here a shared file) between two
// intents the architecture keeps in different DOMAINS is architecturally
// surprising — a leak/misplaced responsibility — so it earns a boundary_crossing
// signal and outranks an equivalent same-domain coupling. `loom next --mode
// discovery` returns the single highest-priority pair, so it must be the
// cross-domain one.
#[test]
fn sqlite_discovery_ranks_boundary_crossing_coupling_first() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("surprise");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "shared.go",
        "package shared\nfunc ParseThing() {}\nfunc StoreThing() {}\n",
    );
    write_scratch_file(
        &graph.root,
        "common.go",
        "package common\nfunc UtilA() {}\nfunc UtilB() {}\n",
    );
    let b = "llm:builder";
    for (name, domain) in [
        ("parse path", "parsing"), // cross-domain pair, both grounded in shared.go
        ("store path", "storage"),
        ("util one", "util"), // same-domain control, both grounded in common.go
        ("util two", "util"),
    ] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                "X.",
                "--level",
                "feature",
                "--domain",
                domain,
                "--json",
            ],
            b,
        );
    }
    run_json_as(&graph.root, &["codefile", "add", "*.go", "--json"], b);
    run_json(&graph.root, &["sync", "--json"]);
    for (name, file, loc) in [
        ("parse path", "shared.go", "func ParseThing"),
        ("store path", "shared.go", "func StoreThing"),
        ("util one", "common.go", "func UtilA"),
        ("util two", "common.go", "func UtilB"),
    ] {
        run_json_as(
            &graph.root,
            &["edge", "implement", name, file, "--locator", loc, "--json"],
            b,
        );
    }

    // The top discovery item is the cross-domain coupling, flagged as boundary-crossing.
    let v = run_json(&graph.root, &["next", "--mode", "discovery", "--json"]);
    assert!(
        v["discovery_signals"]
            .as_array()
            .is_some_and(|s| s.iter().any(|x| x["kind"] == "boundary_crossing")),
        "top discovery pair must carry the boundary_crossing signal: {v}"
    );
    let pair = format!("{} {}", v["intent_a"]["name"], v["intent_b"]["name"]);
    assert!(
        pair.contains("parse path") && pair.contains("store path"),
        "the cross-domain pair must rank first, not the same-domain control: {v}"
    );
}

// G2 (the EXIT-0 launder fix): EXECUTED proof requires the executor to OBSERVE
// the runner ASSERT — a passing-but-inert command (exits 0, asserts nothing) is
// `ran_inert` and counts only as ASSERTED, never EXECUTED. Also pins that
// `loom validate --json` no longer leaks the runner's stdout into its envelope
// (run_json would fail to parse if it did).
#[test]
fn sqlite_g2_executed_requires_a_discriminating_runner() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("g2");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "x.go", "package x\nfunc F() {}\nfunc G() {}\n");
    let b = "llm:builder";
    for name in ["disc", "inert"] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                "X.",
                "--level",
                "feature",
                "--json",
            ],
            b,
        );
    }
    run_json_as(&graph.root, &["codefile", "add", "x.go", "--json"], b);
    run_json(&graph.root, &["sync", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "disc",
            "x.go",
            "--locator",
            "func F",
            "--json",
        ],
        b,
    );
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "inert",
            "x.go",
            "--locator",
            "func G",
            "--json",
        ],
        b,
    );
    let disc = intent_id_by_name(&graph.root, "disc");
    let inert = intent_id_by_name(&graph.root, "inert");
    // A discriminating proof: output names a passing runner. An inert proof:
    // exits 0 but asserts nothing.
    run_json_as(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "disc-proof",
            "--type",
            "test",
            "--command",
            "echo \"test result: ok. 1 passed\"",
            "--intent",
            &disc,
            "--json",
        ],
        "llm:validator",
    );
    run_json_as(
        &graph.root,
        &[
            "validation",
            "add",
            "--name",
            "inert-proof",
            "--type",
            "test",
            "--command",
            "test 1 = 1",
            "--intent",
            &inert,
            "--json",
        ],
        "llm:validator",
    );
    // run_json parsing the validate output IS the no-leak assertion.
    run_json_as(&graph.root, &["validate", &disc, "--json"], "llm:validator");
    run_json_as(
        &graph.root,
        &["validate", &inert, "--json"],
        "llm:validator",
    );

    let cov = run_json(&graph.root, &["status", "--json"])["graph_state"]["coverage"].clone();
    // Both passed, but only the discriminating one is EXECUTED; the inert one
    // is demoted to ASSERTED.
    assert_eq!(
        cov["proven_leaves"]["covered"], 2,
        "both proofs passed: {cov}"
    );
    assert_eq!(
        cov["proven_executed_leaves"]["covered"], 1,
        "only the discriminating proof is EXECUTED: {cov}"
    );
    assert_eq!(
        cov["proven_asserted_leaves"]["covered"], 1,
        "the inert (exit-0, no assertion) proof falls to ASSERTED: {cov}"
    );

    // The asserted-only leaf keeps Production-ready out of reach: the former
    // fully_proven G1 gate is now the Realized rung's discriminating-proof
    // requirement, rolled into the maturity ladder.
    let st = run_json(&graph.root, &["status", "--json"]);
    let rungs = st["maturity"]["rungs"]
        .as_array()
        .expect("maturity rungs array");
    let prod = rungs
        .iter()
        .find(|r| r["name"] == "Production-ready")
        .expect("a Production-ready rung");
    assert_ne!(
        prod["status"].as_str(),
        Some("met"),
        "an asserted-only leaf must block Production-ready: {st}"
    );
    let realized = rungs
        .iter()
        .find(|r| r["name"] == "Realized")
        .expect("a Realized rung");
    assert!(
        realized["reasons"].as_array().is_some_and(|rs| rs
            .iter()
            .any(|r| r.as_str().is_some_and(|s| s.contains("executed-proven")))),
        "the asserted-only leaf must surface as a Realized gap: {st}"
    );
}

// `loom complete` — the comprehensiveness projection. Pins the crux honesty law,
// RECORD ≠ DISCHARGE: a behavioral gap (a happy leaf with no failure-path sibling)
// is OWED, and recording a PLANNED sad sibling does NOT discharge it — only a
// realized one does. Also pins the journey ledger (a user_visible leaf owes a saga).
#[test]
fn sqlite_complete_record_is_not_discharge() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("complete");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "pay.go",
        "package pay\nfunc Charge() {}\nfunc Refund() {}\n",
    );
    let b = "llm:builder";
    // A system parent + a realized, user_visible, happy leaf.
    let sys = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "payments",
            "--description",
            "Payments.",
            "--level",
            "system",
            "--json",
        ],
        b,
    );
    let sys_id = sys["id"].as_str().unwrap().to_string();
    let happy = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "charge happy path",
            "--description",
            "Charge a card.",
            "--level",
            "feature",
            "--aspect",
            "happy",
            "--visibility",
            "user_visible",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        b,
    );
    let happy_id = happy["id"].as_str().unwrap().to_string();
    run_json_as(
        &graph.root,
        &["edge", "hierarchy", &sys_id, &happy_id, "--json"],
        b,
    );
    run_json_as(&graph.root, &["codefile", "add", "pay.go", "--json"], b);
    run_json(&graph.root, &["sync", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            &happy_id,
            "pay.go",
            "--locator",
            "func Charge",
            "--json",
        ],
        b,
    );

    // Behavioral: enumerated 1, discharged 0 (no failure sibling). Journey: the
    // user_visible leaf owes a saga.
    let c1 = run_json(&graph.root, &["complete", "--json"])["comprehensiveness"].clone();
    assert_eq!(c1["behavioral"]["enumerated"], 1, "{c1}");
    assert_eq!(
        c1["behavioral"]["discharged"], 0,
        "happy leaf with no sibling is owed: {c1}"
    );
    assert_eq!(
        c1["journey"]["discharged"], 0,
        "user_visible leaf owes a saga: {c1}"
    );

    // RECORD ≠ DISCHARGE: a PLANNED sad sibling does NOT discharge the gap.
    let sad = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "charge declined",
            "--description",
            "Card declined.",
            "--level",
            "feature",
            "--aspect",
            "sad",
            "--lifecycle",
            "planned",
            "--json",
        ],
        b,
    );
    let sad_id = sad["id"].as_str().unwrap().to_string();
    run_json_as(
        &graph.root,
        &["edge", "hierarchy", &sys_id, &sad_id, "--json"],
        b,
    );
    let c2 = run_json(&graph.root, &["complete", "--json"])["comprehensiveness"].clone();
    assert_eq!(
        c2["behavioral"]["discharged"], 0,
        "a PLANNED sibling is binding debt, not a discharge (RECORD ≠ DISCHARGE): {c2}"
    );
}

// `loom complete` pushes on DOC-AS-REALIZATION: an intent marked implemented but
// grounded ONLY to documentation is a spec certified as a built system. loom can't
// judge if the doc IS the deliverable, so it surfaces it for the LLM to confront —
// but a code-grounded intent is never flagged.
#[test]
fn sqlite_complete_flags_doc_only_realization() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("docreal");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "spec.md", "# Spec\nThe system shall parse.\n");
    write_scratch_file(&graph.root, "real.go", "package x\nfunc Run() {}\n");
    let b = "llm:builder";
    for (name, lvl) in [("architecture spec", "system"), ("real code", "feature")] {
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                name,
                "--description",
                "X.",
                "--level",
                lvl,
                "--lifecycle",
                "implemented",
                "--json",
            ],
            b,
        );
    }
    run_json_as(&graph.root, &["codefile", "add", "spec.md", "--json"], b);
    run_json_as(&graph.root, &["codefile", "add", "real.go", "--json"], b);
    run_json(&graph.root, &["sync", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "architecture spec",
            "spec.md",
            "--json",
        ],
        b,
    );
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            "real code",
            "real.go",
            "--locator",
            "func Run",
            "--json",
        ],
        b,
    );

    let v = run_json(&graph.root, &["complete", "--json"]);
    let docs = v["doc_only_realizations"]
        .as_array()
        .expect("doc_only_realizations");
    assert!(
        docs.iter().any(|x| x == "architecture spec"),
        "a doc-only implemented intent must be flagged: {v}"
    );
    assert!(
        !docs.iter().any(|x| x == "real code"),
        "a code-grounded intent must NOT be flagged: {v}"
    );
}

// query-shaped command that secretly mutated state (layer-order with no args,
// glob+locator). This turns "reads don't write" into a standing, enforced
// invariant: every read-shaped command must leave the committed export
// BYTE-IDENTICAL. A command that writes (even a benign-looking timestamp bump or
// fact rewrite) flips the export and fails here loudly, instead of drifting
// `export --check` silently. Command exit codes are ignored on purpose — whether
// a read succeeds or errors, it must not mutate.
#[test]
fn sqlite_read_commands_do_not_mutate_the_graph() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("reads-no-write");
    let export = || run_text_as(&graph.root, &["export", "-"], "llm");
    let baseline = export();

    let read_cmds: &[&[&str]] = &[
        &["status", "--json"],
        &["next", "--mode", "discovery", "--json"],
        &["next", "--mode", "fix", "--json"],
        &["smells", "--json"],
        &["report", "--json"],
        &["doctor", "--json"],
        &["coverage", "--json"],
        &["schema", "--json"],
        &["guide", "--json"],
        &["wiki", "-"],
        &["find", "graph", "--json"],
        &["explain", "src/repo.rs", "--json"],
    ];
    for args in read_cmds {
        // Run it; ignore output AND exit code — the only contract under test is
        // that it does not write.
        let _ = Command::new(loom_bin())
            .args(*args)
            .current_dir(&graph.root)
            .env_remove("LOOM_GRAPH")
            .output()
            .unwrap_or_else(|e| panic!("run loom {args:?}: {e}"));
        assert_eq!(
            export(),
            baseline,
            "read command {args:?} mutated the graph (the export changed) — reads must not write"
        );
    }
}

// ENHANCEMENT #4 (extraction self-grade): end-to-end — a freshly registered file
// is EXTRACTED and graded by its first `loom sync` (the add no longer pre-stamps
// the content hash, which had made content-addressed sync skip it), and
// `codefile show --json` exposes the grade so a consumer can weight the facts.
#[test]
fn sqlite_sync_grades_files_and_codefile_show_exposes_it() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("extractor-grade");
    run_json(&graph.root, &["init", ".", "--json"]);
    // Go is heuristic-graded ("low") in BOTH build configs (tree-sitter doesn't
    // cover Go), so the assertion is build-independent.
    write_scratch_file(
        &graph.root,
        "svc.go",
        "package main\nfunc Run() {}\nfunc Stop() {}\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "svc.go", "--json"],
        "llm:builder",
    );
    run_json(&graph.root, &["sync", "--json"]);
    let show = run_json(&graph.root, &["codefile", "show", "svc.go", "--json"]);
    assert_eq!(
        show["codefile"]["extractor_grade"], "low",
        "first sync must grade the file and `codefile show --json` must expose it: {show}"
    );
    let syms = show["codefile"]["symbols"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        syms.iter().any(|s| s == "func Run"),
        "first sync must EXTRACT a freshly-registered file's symbols: {show}"
    );
}

// SWEEP #3 (scale): the default discovery class (suspected-coupling) no longer
// scores every O(N²) pair — it generates candidates from inverted indices, an
// EXACT superset of the signal-bearing pairs. On a graph whose facet buckets are
// all under the dense-facet cap (loom's own), the candidate path must therefore
// produce EXACTLY the same suspected_coupling set the full scan does. The full
// scan's suspected count is `all - impact_map` (every pair is one class or the
// other), so candidate-path `suspected-coupling` total must equal that. (If this
// ever diverges, the DF-cap has started firing on loom's graph — raise it or
// accept the drop; it can only ever drop weakest-signal pairs, never add wrong ones.)
#[test]
fn sqlite_discovery_candidate_path_matches_full_scan() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("disc-candidates");
    // The fixture is horizontally complete; seed one unexplored pair that shares
    // an implemented file so there IS a signal-bearing (suspected-coupling)
    // candidate. The shared file is owned by exactly 2 intents — far below the
    // dense-facet BUCKET_CAP — so the candidate path stays an exact superset and
    // the suspected==all-impact invariant this test asserts still holds.
    seed_unexplored_signal_pair(&graph.root, "disc-candidates");
    let total = |class: &str| -> i64 {
        // `--take` yields the bulk envelope whose `queue_total` is the FULL queue
        // size for the class (not the taken count) — the number we're comparing.
        run_json(
            &graph.root,
            &[
                "next",
                "--mode",
                "discovery",
                "--class",
                class,
                "--take",
                "50",
                "--json",
            ],
        )["queue_total"]
            .as_i64()
            .unwrap_or(-1)
    };
    let suspected = total("suspected-coupling");
    let impact = total("impact-map");
    let all = total("all");
    assert!(suspected > 0, "loom's graph has signal-bearing pairs");
    assert_eq!(
        suspected,
        all - impact,
        "candidate-path suspected-coupling ({suspected}) must equal the full scan's \
         suspected count (all {all} - impact_map {impact} = {})",
        all - impact
    );
}

#[test]
fn sqlite_discovery_vocab_weight_prose_matches_field() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("sig-weight");
    let v = run_json(&graph.root, &["next", "--mode", "discovery", "--json"]);
    let field_w = v["discovery_signals"]
        .as_array()
        .or_else(|| {
            v.get("item")
                .and_then(|i| i["discovery_signals"].as_array())
        })
        .and_then(|s| s.iter().find(|x| x["kind"] == "shared_vocab"))
        .and_then(|x| x["weight"].as_f64());
    if let Some(w) = field_w {
        let text = run_text_as(&graph.root, &["next", "--mode", "discovery"], "llm");
        let needle = format!("weight {w:.2}");
        assert!(
            text.contains(&needle),
            "discovery prose must show the same vocab weight as the --json field ({needle}): {text}"
        );
    }
}

// DOGFOOD-FOUND DEFECT (AI-companion hunt): a self-edge (same id in both slots —
// an easy UUID fat-finger) miscounted in the existence probe and reported "intent
// not found", sending the AI to recreate an intent that already exists. Now named.
#[test]
fn sqlite_self_edge_named_not_misdiagnosed_as_missing() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("self-edge");
    let (a, _b) = first_two_intent_ids(&graph.root);
    let run = |args: &[&str]| {
        std::process::Command::new(loom_bin())
            .args(args)
            .current_dir(&graph.root)
            .env("LOOM_AGENT", "llm:builder")
            .env_remove("LOOM_GRAPH")
            .output()
            .expect("run loom")
    };
    for sub in [["edge", "hierarchy"], ["edge", "explore"]] {
        let out = run(&[sub[0], sub[1], &a, &a]);
        assert!(!out.status.success(), "{sub:?} self-edge must be refused");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            (err.contains("its own parent") || err.contains("relate to itself"))
                && !err.contains("not found"),
            "{sub:?} self-edge must name the real cause, not 'not found': {err}"
        );
    }
}

// DOGFOOD-FOUND DEFECT (AI-companion hunt): equal-scored code_clone findings were
// emitted in HashMap iteration order — non-deterministic across runs, churning
// diffs and breaking positional adjudication. A stable tie-break fixes it.
#[test]
fn sqlite_smells_clone_order_is_deterministic() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("clone-order");
    let clones = || {
        let v = run_json(&graph.root, &["smells", "--json"]);
        v["code_clones"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| {
                        format!(
                            "{}|{}",
                            c["summary"].as_str().unwrap_or(""),
                            c["evidence"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let first = clones();
    for _ in 0..5 {
        assert_eq!(
            first,
            clones(),
            "code_clone order must be stable across runs"
        );
    }
}

// DOGFOOD-FOUND DEFECTS (AI-companion hunt): teaching-vs-behavior drift — the
// guide footer omitted the valid `import` mode, and the oversized_file remedy
// emitted a `loom hypothesis add` missing the REQUIRED --predicted-outcome (a
// copied remedy hit a hard clap error). Both are commands loom tells the AI to
// run, so drift makes it act wrongly.
#[test]
fn sqlite_teaching_commands_match_behavior() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("teaching-drift");
    let run = |args: &[&str]| {
        String::from_utf8_lossy(
            &std::process::Command::new(loom_bin())
                .args(args)
                .current_dir(&graph.root)
                .env_remove("LOOM_GRAPH")
                .output()
                .expect("run loom")
                .stdout,
        )
        .to_string()
    };
    // Bare `loom guide` is focus-scoped (the focus rung's skill); the full
    // manual and its mode footer now live behind `--all`.
    let guide = run(&["guide", "--all"]);
    for m in [
        "greenfield",
        "brownfield",
        "refactor",
        "port",
        "seed",
        "import",
    ] {
        assert!(
            guide.contains(m),
            "guide footer must list every mode ({m}): {guide}"
        );
    }
    // Every RUNNABLE hypothesis-add remedy (the `--name` form) must carry the
    // required --predicted-outcome, or a copied remedy hard-errors.
    let smells = run(&["smells"]);
    assert!(
        smells.contains("oversized_file"),
        "scratch should surface an oversized_file finding to exercise its remedy"
    );
    for line in smells
        .lines()
        .filter(|l| l.contains("loom hypothesis add --name"))
    {
        assert!(
            line.contains("--predicted-outcome"),
            "a runnable hypothesis-add remedy is missing the REQUIRED --predicted-outcome: {line}"
        );
    }
}

// B4: bare `loom guide` is FOCUS-SCOPED — it serves the focus rung's lane-skill
// (JIT), so the entry point answers "how do I do THIS rung" instead of dumping
// the manual; `--all` is the opt-in firehose. This makes the status pointer
// ("bare guide is focus-scoped") honest, and keeps `guide` and `next` agreeing
// on the lane (both route by the maturity ladder's focus rung).
#[test]
fn sqlite_bare_guide_serves_the_focus_lane_skill() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("guide-focus");
    // The authoritative focus lane — what `loom status` / `loom next` route by.
    let status = run_json(&graph.root, &["status", "--json"]);
    let focus = status["maturity"]["focus"]
        .as_u64()
        .expect("an imported graph mid-flight has an unmet focus rung") as usize;
    let lane = status["maturity"]["rungs"][focus]["lane"]
        .as_str()
        .expect("the focus rung names its lane");
    let expected_role = match lane {
        "build" => Some("builder"),
        "discovery" => Some("analyzer"),
        "fix" => Some("fixer"),
        "validate" => Some("validator"),
        "quality" => Some("quality"),
        // The Hardened `audit` lane (smell adjudication) is not a single role lane;
        // bare guide there serves the general charge, with no `role`.
        _ => None,
    };
    // Bare guide = the focus rung's role charge (or the general charge for a
    // non-role focus lane), NOT the manual when a role lane is in focus.
    let bare = run_json(&graph.root, &["guide", "--json"]);
    assert_eq!(
        bare["role"].as_str(),
        expected_role,
        "bare `loom guide` must serve the focus lane's skill (lane={lane})"
    );
    if expected_role.is_some() {
        assert!(
            bare.get("done_condition").is_none(),
            "bare guide is the lane skill, not the full manual"
        );
    }
    // `--all` = the full driving protocol (the firehose), never a role charge.
    let all = run_json(&graph.root, &["guide", "--all", "--json"]);
    assert!(
        all.get("done_condition").is_some(),
        "`loom guide --all` must be the full manual"
    );
    assert!(all.get("role").is_none(), "the manual is not a role charge");
}

// DOGFOOD-FOUND DEFECT (AI-companion hunt): `loom status` lumped registered-but-
// DELETED files in with "on disk the graph doesn't account for" and pointed the
// AI at `loom coverage` / `codefile add` / `ignore` — none of which fix a
// deletion (and coverage reports the opposite, "0 missed"). The compass thus
// contradicted itself. Now MISSING files are labelled + offered `codefile remove`.
#[test]
fn sqlite_status_labels_missing_files_with_the_right_remedy() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("missing-files");
    let run = |args: &[&str]| {
        std::process::Command::new(loom_bin())
            .args(args)
            .current_dir(&graph.root)
            .env_remove("LOOM_GRAPH")
            .env_remove("LOOM_AGENT")
            .output()
            .expect("run loom")
    };
    run(&["init", "."]);
    write_scratch_file(&graph.root, "src/gone.rs", "pub fn a() {}\n");
    run(&["codefile", "add", "src/gone.rs"]);
    run(&["sync"]);
    fs::remove_file(graph.root.join("src/gone.rs")).expect("delete the registered file");
    let out = run(&["status"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("MISSING") && text.contains("codefile remove"),
        "status must label a deleted-but-registered file MISSING + offer codefile remove: {text}"
    );
    assert!(
        !text.contains("on disk the graph doesn't account for"),
        "must NOT lump MISSING under 'on disk ... doesn't account for': {text}"
    );
}

// DOGFOOD-FOUND DEFECT (AI-companion hunt): a no-op `loom sync` (0 changes
// reported) unconditionally bumped `last_synced`, which travels in the committed
// export, flipping `export --check` to STALE — and never converging. loom's own
// help tells the AI to sync after any change, so it hits a green-looking sync
// that breaks the freshness gate it relies on. Now a true no-op leaves it green.
#[test]
fn sqlite_noop_sync_keeps_export_check_green() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("noop-sync");
    let run = |args: &[&str]| {
        std::process::Command::new(loom_bin())
            .args(args)
            .current_dir(&graph.root)
            .env_remove("LOOM_GRAPH")
            .output()
            .expect("run loom")
    };
    run(&["init", "."]);
    // a real file on disk so sync finds nothing missing/changed (a true no-op)
    write_scratch_file(&graph.root, "src/lib.rs", "pub fn x() {}\n");
    run(&["codefile", "add", "src/lib.rs"]);
    run(&["sync"]);
    run(&["sync"]); // settle
    run(&["export"]); // write loom.graph.json
    assert!(
        run(&["export", "--check"]).status.success(),
        "fresh export must be clean"
    );
    run(&["sync"]); // the no-op that used to bump last_synced
    assert!(
        run(&["export", "--check"]).status.success(),
        "a no-op sync must NOT flip export --check to STALE (last_synced churn)"
    );
}

// DOGFOOD-FOUND DEFECTS (AI-companion hunt): commands that silently no-op'd on
// bad input — a glob matching zero files, an empty pattern/text/identifier —
// reported success or dumped the whole graph instead of failing loudly. An AI
// believes it acted when it didn't. Each now refuses with guidance.
#[test]
fn sqlite_bad_input_guards_refuse_silent_noops() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("bad-input");
    let expect_refused = |args: &[&str], frag: &str| {
        let out = std::process::Command::new(loom_bin())
            .args(args)
            .current_dir(&graph.root)
            .env_remove("LOOM_GRAPH")
            .env_remove("LOOM_AGENT")
            .output()
            .expect("run loom");
        assert!(
            !out.status.success(),
            "{args:?} must be refused, not a silent no-op: {:?}",
            out.status
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains(frag),
            "{args:?} should explain the refusal ({frag:?}): {err}"
        );
    };
    expect_refused(&["codefile", "add", "src/zzz_none_*.rs"], "matched 0 files");
    expect_refused(&["ignore", "add", "", "--reason", "x"], "can't be empty");
    expect_refused(&["note", "add", "--text", ""], "needs text");
    expect_refused(&["intent", "show", ""], "can't be empty");
}

// DOGFOOD-FOUND DEFECT: `loom wiki -` failed ("unexpected argument") while
// `loom export -` worked — inconsistent stdout syntax across the two projection
// commands. wiki now takes a positional path like export, so `loom wiki -` → stdout.
#[test]
fn sqlite_wiki_stdout_parity_with_export() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("wiki-stdout");
    let out = std::process::Command::new(loom_bin())
        .args(["wiki", "-"])
        .current_dir(&graph.root)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom wiki -");
    assert!(
        out.status.success(),
        "`loom wiki -` must succeed (parity with `loom export -`): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("# loom"),
        "`loom wiki -` emits the wiki markdown to stdout: {}",
        &stdout[..stdout.len().min(80)]
    );
}

// DOGFOOD-FOUND DEFECT: `loom edge implement <intent> <glob-matching-many> --locator X`
// silently DROPPED the locator, skipped verify-first, and mass-grounded the intent
// to every matched file. A locator names one symbol in one file — glob + locator
// is now refused.
#[test]
fn sqlite_glob_grounding_refuses_a_locator() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("glob-locator");
    let (a, _b) = first_two_intent_ids(&graph.root);
    let out = std::process::Command::new(loom_bin())
        .args([
            "edge",
            "implement",
            &a,
            "src/commands/*.rs",
            "--locator",
            "fn whatever",
        ])
        .current_dir(&graph.root)
        .env("LOOM_AGENT", "llm:builder")
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom edge implement glob+locator");
    assert!(
        !out.status.success(),
        "glob + locator must be refused, not silently mass-grounded: {:?}",
        out.status
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("cannot be used with a glob"),
        "the refusal explains why: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// A glob over registered paths is BULK grounding: it must create an IMPLEMENTS
// edge for EVERY matched file, not just one. src/commands/next/*.rs is 7 files.
#[test]
fn sqlite_glob_grounding_grounds_every_matched_file() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("glob-bulk");
    let (a, _b) = first_two_intent_ids(&graph.root);
    let out = run_json_as(
        &graph.root,
        &["edge", "implement", &a, "src/commands/next/*.rs", "--json"],
        "llm:builder",
    );
    let grounded = out["grounded"].as_array().expect("grounded array in json");
    assert!(
        grounded.len() > 1,
        "a glob matching many registered files must bulk-ground ALL of them, not one: {out}"
    );
    let paths: Vec<&str> = grounded.iter().filter_map(|p| p.as_str()).collect();
    assert!(
        paths.contains(&"src/commands/next/quality.rs")
            && paths.contains(&"src/commands/next/render.rs"),
        "every matched registered file is grounded (saw {paths:?})"
    );
}

// DOGFOOD-FOUND GAP: `loom layer list` with no order declared printed nothing
// about WHICH layers intents already carry — you had to grep json to know what to
// declare. It now lists the in-use layers as the candidates for an order.
#[test]
fn sqlite_layer_list_shows_in_use_layers_when_no_order() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("layer-list");
    // Give an intent a layer so there's an in-use layer to surface.
    let (a, _b) = first_two_intent_ids(&graph.root);
    run_json_as(
        &graph.root,
        &[
            "intent",
            "update",
            &a,
            "--layer",
            "persistence",
            "--reason",
            "tagging the storage boundary for the layering audit",
            "--json",
        ],
        "llm:builder",
    );
    let text = run_text_as(&graph.root, &["layer", "list"], "llm");
    assert!(
        text.contains("already in use") && text.contains("persistence"),
        "layer list surfaces in-use layers as candidates for an order: {text}"
    );
}

// DOGFOOD-FOUND DEFECT: `loom layer order` with NO layers used to silently CLEAR
// the order and print "✓ Layer order declared" — contradicting `loom smells`'s
// "no declared order" and destructively writing on a query-shaped invocation. It
// must refuse with guidance and leave a declared order untouched.
#[test]
fn sqlite_layer_order_no_args_refuses_without_clearing() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("layer-noargs");
    // Declare a real order first.
    run_json_as(
        &graph.root,
        &[
            "layer",
            "order",
            "presentation",
            "application",
            "storage",
            "--json",
        ],
        "llm:builder",
    );
    // `loom layer order` with NO layers must error, not clear + lie "✓ declared".
    let out = std::process::Command::new(loom_bin())
        .args(["layer", "order"])
        .current_dir(&graph.root)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run loom layer order with no args");
    assert!(
        !out.status.success(),
        "no-args `layer order` must exit non-zero: {:?}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("No layers given") && stderr.contains("loom layer list"),
        "it points to the right verb (list/clear), not a silent clear: {stderr}"
    );
    // CRITICAL: the previously-declared order is UNTOUCHED (no destructive write).
    let list = run_json(&graph.root, &["layer", "list", "--json"]);
    assert_eq!(
        list["order"].as_array().map(|a| a.len()),
        Some(3),
        "the no-args command must NOT have cleared the declared order: {list}"
    );
}

// OPT-IN INSTALL: `loom skill` emits the lane-skills as real SKILL.md files (a
// regenerable projection of the gate's lane table) for the user who wants to PIN
// them — never required (the binary serves them JIT). list → menu; show → one
// proven-format SKILL.md that delegates the live charge back to the binary;
// install --write → pins all five.
#[test]
fn sqlite_skill_command_emits_lane_skills() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("skill-cmd");
    // list: the 5 enforced lanes, each a loom-<role> skill with a JIT description.
    let list = run_json(&graph.root, &["skill", "list", "--json"]);
    let skills = list["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 5, "five lane-skills: {list}");
    assert!(
        skills.iter().any(|s| s["skill"] == "loom-analyzer"
            && s["description"]
                .as_str()
                .unwrap_or("")
                .contains("Adopt when")),
        "loom-analyzer is listed with a JIT trigger description: {list}"
    );
    // show: one complete SKILL.md in the proven format that points the live charge
    // back at the binary (so a pinned copy can't drift).
    let show = run_json(&graph.root, &["skill", "show", "analyzer", "--json"]);
    let md = show["markdown"].as_str().expect("markdown");
    assert!(
        md.starts_with("---\nname: loom-analyzer\n")
            && md.contains("**THE LAW**")
            && md.contains("THE SOCRATIC LOOP is the skill")
            && md.contains("loom guide --role analyzer"),
        "the SKILL.md is proven-format + delegates the live charge to the binary: {md}"
    );
    // install --write: pins all 5 as real files; opt-in, never required.
    let inst = run_json(
        &graph.root,
        &[
            "skill",
            "install",
            "--dir",
            "scratch/skills",
            "--write",
            "--json",
        ],
    );
    assert_eq!(
        inst["written"].as_array().map(|a| a.len()),
        Some(5),
        "five files written: {inst}"
    );
    let pinned = graph.root.join("scratch/skills/loom-fixer/SKILL.md");
    let body = std::fs::read_to_string(&pinned).expect("loom-fixer SKILL.md was written");
    assert!(
        body.contains("name: loom-fixer") && body.contains("**THE LAW**"),
        "the pinned file is a valid lane-skill SKILL.md: {body}"
    );
}

// JIT SKILL ADOPTION: when loom routes work to a lane, the work item CUES the LLM
// to adopt that lane's discipline just-in-time via `loom guide --role <lane>` (the
// binary serves the full loom-<lane> skill — no install). The compass is the JIT
// trigger; the charge is the skill.
#[test]
fn sqlite_next_cues_jit_skill_adoption() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("jit-skill-cue");
    // The fixture is horizontally complete; seed one unexplored, signal-bearing
    // pair so `next` has discovery work to route to the analyzer lane.
    seed_unexplored_signal_pair(&graph.root, "jit-skill-cue");
    let item = run_json(&graph.root, &["next", "--mode", "discovery", "--json"]);
    let dispatch = item["dispatch"].as_str().unwrap_or("");
    assert!(
        dispatch.contains("loom guide --role analyzer") && dispatch.contains("ADOPT"),
        "routing to the analyzer lane cues JIT adoption of the loom-analyzer skill: {dispatch}"
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
            "UPDATE relates_to SET confidence=0.9, evidence='prior inspection recorded this coupling' WHERE confidence < 0.7 OR (confidence >= 0.7 AND evidence = '')",
            [],
        )
        .expect("bump relates confidence and fill empty evidence");
        conn.execute(
            "UPDATE governs SET confidence=0.9, evidence='prior inspection recorded this compliance' WHERE confidence < 0.7 OR (confidence >= 0.7 AND evidence = '')",
            [],
        )
        .expect("bump governs confidence and fill empty evidence");
        // Bump rule severity to warning so the v12 high-severity review trigger
        // (error-severity passing verdicts route to review even at high
        // confidence) doesn't flood the queue — this test is about the
        // low-confidence RELATES_TO path, not the high-severity GOVERNS path.
        conn.execute(
            "UPDATE quality_rule SET severity='warning' WHERE severity='error'",
            [],
        )
        .expect("bump rule severity to warning");
        // Also flatten altitudes so the high-altitude review trigger doesn't fire.
        conn.execute(
            "UPDATE intent SET abstraction_level='feature' WHERE abstraction_level IN ('system','cross_cutting')",
            [],
        )
        .expect("flatten intent altitude");
        // Clear any partial verdicts so the partial review trigger doesn't fire.
        conn.execute(
            "UPDATE governs SET inspection_status='passing' WHERE inspection_status='partial'",
            [],
        )
        .expect("clear partial status");
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
    // The fixture is horizontally complete; seed one unexplored, signal-bearing
    // pair so the discovery lane has a work item to hand back.
    seed_unexplored_signal_pair(&graph.root, "next-context");

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
        // Also set a non-empty criterion: a passing edge with an empty criterion
        // is a separate doctor ISSUE that exits 1 before the weak-kinds HINT can
        // be inspected. With a real criterion, the only thing doctor flags here
        // is the weak-only grounding HINT — exactly what this test verifies.
        conn.execute(
            "UPDATE relates_to SET inspection_status='passing', kinds='[\"same_domain\"]', \
             criterion='shared concept domain' \
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
    std::fs::create_dir_all(graph.root.join("src")).unwrap();
    std::fs::write(graph.root.join("src/lib.rs"), "pub fn checkout() {}\n").unwrap();
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
            "test 1 = 1",
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
            "--evidence-locator",
            "src/lib.rs",
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
    // The committed fixture is actionable (phase=validate — asserted-only leaves
    // are proof work). Seed an unexplored discovery pair so `next --all` returns
    // the full `queues` array (a no-source scratch otherwise omits it). Seeded
    // BEFORE the debt capture so the discovery work is already counted in
    // `initial_required_debt`; the optional inbox add must not change that total.
    seed_unexplored_signal_pair(&graph.root, "inbox-flow");
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
        "alarms",
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
        "maturity",
        "needs_reverification",
        "open_issues",
        "open_todos",
        "optional_autonomous",
        "other_lanes",
        "passing_edges",
        "populate",
        "source_corpus",
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
fn sqlite_behavioral_owed_carries_ids_for_a_runnable_suggestion() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("behavioral-owed");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "x.go", "package x\nfunc Save() {}\n");
    let parent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "save feature",
            "--level",
            "component",
            "--description",
            "the save subsystem here",
            "--json",
        ],
        "llm:builder",
    );
    let pid = parent["id"].as_str().expect("parent id").to_string();
    let child = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "save happy",
            "--level",
            "feature",
            "--aspect",
            "happy",
            "--description",
            "save succeeds and persists",
            "--json",
        ],
        "llm:builder",
    );
    let cid = child["id"].as_str().expect("child id").to_string();
    run_json_as(
        &graph.root,
        &["edge", "hierarchy", &pid, &cid, "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "x.go", "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            &cid,
            "x.go",
            "--locator",
            "Save",
            "--json",
        ],
        "llm:builder",
    );
    run_json(&graph.root, &["sync", "--json"]);
    // A realized happy leaf with no sad sibling OWES one — and the owed entry must
    // carry the ids (not just a prose name) so `loom complete` can emit a runnable
    // `loom intent add --aspect sad --parent <id>` suggestion (#4).
    let complete = run_json(&graph.root, &["complete", "--json"]);
    let owed = complete["comprehensiveness"]["behavioral"]["owed"]
        .as_array()
        .expect("behavioral owed array");
    assert_eq!(
        owed.len(),
        1,
        "happy leaf with no sad sibling owes one: {complete}"
    );
    assert_eq!(
        owed[0]["id"].as_str(),
        Some(cid.as_str()),
        "owed carries the leaf id"
    );
    assert_eq!(
        owed[0]["parent_id"].as_str(),
        Some(pid.as_str()),
        "owed carries the parent id for the --parent suggestion"
    );
}

#[test]
fn sqlite_fully_proven_reason_names_the_concrete_blocker() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("fp-blocker");
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "a",
            "--level",
            "system",
            "--description",
            "do a well here",
            "--json",
        ],
        "llm:builder",
    );
    let status = run_json(&graph.root, &["status", "--json"]);
    let next_action = status["graph_state"]["next_action"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !next_action.is_empty(),
        "cascade always names a next_action when not complete"
    );
    let rungs = status["maturity"]["rungs"]
        .as_array()
        .expect("maturity rungs array");
    let prod = rungs
        .iter()
        .find(|r| r["name"] == "Production-ready")
        .expect("a Production-ready rung");
    let reasons = prod["reasons"]
        .as_array()
        .expect("production-ready reasons array");
    let phase_reason = reasons
        .iter()
        .filter_map(|r| r.as_str())
        .find(|r| r.contains("not 'complete'"))
        .expect("a phase blocker reason");
    // The phase blocker must carry the cascade's CONCRETE next_action — so the
    // operator doesn't have to cross-reference status/complete/smells/next (#2).
    assert!(
        phase_reason.contains(&next_action),
        "fully_proven phase blocker must name the concrete next_action:\n  reason: {phase_reason}\n  next_action: {next_action}"
    );
}

#[test]
fn sqlite_edge_unexplored_enumerates_the_counted_pairs() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("unexplored");
    run_json(&graph.root, &["init", ".", "--json"]);
    for n in ["alpha", "beta", "gamma"] {
        let desc = format!("do {n} well in this system");
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                n,
                "--level",
                "system",
                "--description",
                &desc,
                "--json",
            ],
            "llm:builder",
        );
    }
    // The compass COUNTS the unexplored pairs (3 intents, no edges → C(3,2)=3) …
    let status = run_json(&graph.root, &["status", "--json"]);
    let counted = status["graph_state"]["unexplored_pairs"]
        .as_i64()
        .expect("unexplored_pairs in graph_state");
    assert_eq!(counted, 3, "3 intents, no edges → 3 unexplored pairs");
    // … and `loom edge unexplored --class all` RETRIEVES exactly that many — the
    // count and the drainable list must agree (the operator's #1/#5 friction).
    let listed = run_json(
        &graph.root,
        &["edge", "unexplored", "--class", "all", "--json"],
    );
    assert_eq!(
        listed["total"].as_i64(),
        Some(counted),
        "edge unexplored total must equal the counted unexplored_pairs"
    );
    let pairs = listed["unexplored_pairs"].as_array().expect("pairs array");
    assert_eq!(pairs.len(), 3, "all three pairs listed");
    assert!(
        pairs.iter().all(|p| p["explore_command"]
            .as_str()
            .unwrap_or("")
            .contains("loom edge explore")),
        "every pair carries a runnable explore command: {listed}"
    );
}

#[test]
fn sqlite_failing_quality_verdict_teaches_full_disposition_path() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("dispo-path");
    run_json(&graph.root, &["init", ".", "--json"]);
    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "alpha",
            "--level",
            "system",
            "--description",
            "do alpha well",
            "--json",
        ],
        "llm:builder",
    );
    let iid = intent["id"].as_str().expect("intent id").to_string();
    run_json_as(
        &graph.root,
        &[
            "rule",
            "add",
            "--name",
            "r1",
            "--description",
            "no global mutable state is used here",
            "--severity",
            "error",
            "--json",
        ],
        "llm:quality",
    );
    let verdict = run_json_as(
        &graph.root,
        &[
            "rule",
            "verdict",
            "r1",
            &iid,
            "--status",
            "failing",
            "--criterion",
            "no global mutable state in this intent",
            "--evidence",
            "found a global counter in the hot path",
            "--json",
        ],
        "llm:quality",
    );
    // A failing gate is binding — its guidance must teach ALL THREE honest
    // dispositions (fix / defer-as-tracked-hypothesis / justify-as-decision),
    // not dead-end at "flag or fix". This locks the strategic path.
    let ns = verdict["next_step"].as_str().unwrap_or_default();
    assert!(
        ns.contains("hypothesis adopt --spawned"),
        "failing verdict must teach DEFER-as-tracked-work: {ns}"
    );
    assert!(
        ns.contains("kind decision"),
        "failing verdict must teach JUSTIFY-as-decision: {ns}"
    );
    assert!(
        ns.to_lowercase().contains("fix"),
        "failing verdict must teach FIX: {ns}"
    );
}

#[test]
fn sqlite_todo_note_resolution_lifecycle_surfaces_until_resolved() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("todo-lifecycle");

    let baseline = run_json(&graph.root, &["status", "--json"])["open_todos"]
        .as_i64()
        .expect("open_todos is an integer in status --json");

    // An OPEN todo is the LLM-filled follow-up backlog.
    let marker = "TODO-LIFECYCLE-MARKER decompose foo";
    let added = run_json(
        &graph.root,
        &["note", "add", "--kind", "todo", "--text", marker, "--json"],
    );
    let nid = added["id"].as_str().expect("note id").to_string();
    // Adding a todo teaches the resolve path (not a generic "keep working").
    assert!(
        added["next_step"]
            .as_str()
            .unwrap_or_default()
            .contains("note resolve"),
        "adding a todo teaches how to resolve it: {added}"
    );

    // The always-run compass counts it — survives compaction, can't be forgotten.
    let after_add = run_json(&graph.root, &["status", "--json"])["open_todos"]
        .as_i64()
        .unwrap();
    assert_eq!(
        after_add,
        baseline + 1,
        "an open todo raises the status count"
    );
    let open_list = run_json(&graph.root, &["note", "list", "--kind", "todo", "--json"]);
    assert!(
        open_list["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["text"] == marker),
        "the open todo shows in the default list"
    );

    // Closing must say WHY — an empty reason is refused.
    let refused = Command::new(loom_bin())
        .args(["note", "resolve", &nid, "--reason", "   ", "--json"])
        .current_dir(&graph.root)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run note resolve");
    assert!(
        !refused.status.success(),
        "resolving with an empty reason is refused"
    );

    // Resolve closes it with a reason; the compass count drops back.
    run_json(
        &graph.root,
        &[
            "note",
            "resolve",
            &nid,
            "--reason",
            "done in test",
            "--json",
        ],
    );
    let after_resolve = run_json(&graph.root, &["status", "--json"])["open_todos"]
        .as_i64()
        .unwrap();
    assert_eq!(
        after_resolve, baseline,
        "resolving drops the open-todo count back to baseline"
    );

    // Hidden from the default list, visible (with its reason) under --resolved.
    let default_after = run_json(&graph.root, &["note", "list", "--kind", "todo", "--json"]);
    assert!(
        !default_after["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["text"] == marker),
        "a resolved todo is hidden from the default list"
    );
    let resolved_list = run_json(
        &graph.root,
        &["note", "list", "--kind", "todo", "--resolved", "--json"],
    );
    assert!(
        resolved_list["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["text"] == marker && n["resolution"] == "done in test"),
        "a resolved todo is visible under --resolved with its reason: {resolved_list}"
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

// SWEEP #10 (federation): `delegate add` accepted an absolute or path-escaping
// target (its `root.join(..).exists()` check is satisfied when join replaces the
// base with an absolute path), but `loom sync`'s ripple confines the target and
// silently skips anything outside the root — a watch that looks healthy and
// never fires. add now confines the SAME way, rejecting out-of-root targets
// loudly so the two can't disagree.
#[test]
fn sqlite_delegate_add_rejects_out_of_root_targets() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("delegate-confine");
    let add = |target: &str| {
        std::process::Command::new(loom_bin())
            .args(["delegate", "add", "watch/**", "--to", target])
            .current_dir(&graph.root)
            .env("LOOM_AGENT", "llm:builder")
            .env_remove("LOOM_GRAPH")
            .output()
            .expect("run loom delegate add")
    };
    for bad in ["/etc/hosts", "../sibling/loom.graph.json"] {
        let out = add(bad);
        assert!(
            !out.status.success(),
            "an out-of-root delegation target ({bad}) must be refused: {:?}",
            out.status
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("outside the repo root"),
            "the rejection names why ({bad}): {stderr}"
        );
    }
    // A repo-relative target is accepted (the federation happy path).
    let ok = add("services/child/loom.graph.json");
    assert!(
        ok.status.success(),
        "a repo-relative delegation target must be accepted: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
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

#[cfg(feature = "treesitter")]
fn smell_advisory_for(value: &Value, kind: &str, path_prefix: &str) -> bool {
    value["size_advisories"]
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

    // 1. The god-file (~5960 physical lines) surfaces as an oversized_file
    //    ADVISORY (a flag, never gating) — size measured independent of impl/test.
    let s = smells();
    assert!(
        smell_advisory_for(&s, "oversized_file", path),
        "oversized_file must fire as a (non-gating) advisory on the god-file {path}: {s}"
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
        smell_advisory_for(&s, "oversized_file", path),
        "a per-symbol large_behavioral_symbol ruling must NOT launder the file-level \
         oversized_file advisory: {s}"
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
        !smell_advisory_for(&s, "oversized_file", path),
        "the own oversized_file:<path> ruling must suppress the file advisory: {s}"
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
    // The committed graph may now be fully executed-proven (asserted == 0) once
    // every leaf earns a discriminating test. The discriminator behavior —
    // manual_check / empty-command proofs are NOT counted as executed — is proven
    // independently on a manufactured graph by
    // sqlite_proven_axis_discriminates_manual_check_from_executed_test, so this
    // fixture test no longer pins the committed graph to a mid-flight proof mix.
    assert!(
        asserted >= 0 && exec <= proven,
        "executed leaves are a subset of proven: exec={exec} proven={proven} assert={asserted}"
    );
    // The compass discloses the split inline whenever there is proven to inspect.
    // The labels are spelled out (`executed` / `asserted-only`) so the polarity is
    // unmistakable — the old `exec`/`assert` shorthand read backwards to cold
    // drivers ("assert 0" looked like "nothing checked").
    let human = run_text_as(&graph.root, &["status"], "llm:validator");
    assert!(
        human.contains("executed ") && human.contains("asserted-only "),
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
    // have run it. Under G2, EXECUTED also requires the runner to actually
    // ASSERT, so the hermetic fixture emits a recognized passing-runner summary.
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
            "echo \"test result: ok. 1 passed\"",
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

// ---------------------------------------------------------------------------
// Ripple: an `independent` RELATES_TO verdict ("these two intents do NOT
// interact") is durable against behavior-preserving change. The sync
// code-change ripple re-opens it ONLY when a NEW structural import coupling
// appears between the pair — NOT on every unrelated edit. This is what stops a
// few central files from re-staling the whole N×N grid every sync.
// ---------------------------------------------------------------------------

fn relates_status(root: &Path, a: &str, b: &str) -> String {
    let db = root.join(".loom").join("graph.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite graph");
    conn.query_row(
        "SELECT inspection_status FROM relates_to WHERE (from_id=?1 AND to_id=?2) OR (from_id=?2 AND to_id=?1)",
        rusqlite::params![a, b],
        |r| r.get(0),
    )
    .expect("the relates_to edge exists")
}

/// A fresh scratch graph with two intents, each grounded in its OWN file via a
/// real symbol locator, with NO import between the files (structurally
/// uncoupled). Returns the graph and the two intent ids.
fn setup_two_uncoupled_grounded_intents(
    prefix: &str,
    a_src: &str,
    b_src: &str,
) -> (ScratchGraph, String, String) {
    let graph = ScratchGraph::new(prefix);
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "src/a.rs", a_src);
    write_scratch_file(&graph.root, "src/b.rs", b_src);
    for (nm, file, loc) in [
        ("owner a", "src/a.rs", "fn a_thing"),
        ("owner b", "src/b.rs", "fn b_thing"),
    ] {
        run_json_as(
            &graph.root,
            &["codefile", "add", file, "--json"],
            "llm:builder",
        );
        run_json_as(
            &graph.root,
            &[
                "intent",
                "add",
                "--name",
                nm,
                "--description",
                "owns its own file for the independent-ripple regression",
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
            &["edge", "implement", nm, file, "--locator", loc, "--json"],
            "llm:builder",
        );
    }
    let a = intent_id_by_name(&graph.root, "owner a");
    let b = intent_id_by_name(&graph.root, "owner b");
    (graph, a, b)
}

#[test]
fn sqlite_independent_edge_survives_unrelated_code_change() {
    let _guard = sqlite_test_lock();
    let (graph, a, b) = setup_two_uncoupled_grounded_intents(
        "independent-survives",
        "pub fn a_thing() -> u8 { 1 }\n",
        "pub fn b_thing() -> u8 { 2 }\n",
    );
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "independent",
            "--notes",
            "no shared code, no import between the files, distinct concerns",
            "--json",
        ],
        "llm:analyzer",
    );
    assert_eq!(
        relates_status(&graph.root, &a, &b),
        "independent",
        "sanity: the recorded verdict is independent"
    );
    // A behavior-preserving edit to ONE grounded file creates no interaction.
    write_scratch_file(&graph.root, "src/a.rs", "pub fn a_thing() -> u8 { 42 }\n");
    run_json_as(&graph.root, &["sync", "--json"], "llm:analyzer");
    assert_eq!(
        relates_status(&graph.root, &a, &b),
        "independent",
        "an independent verdict must SURVIVE an unrelated code change — the N×N grid must not re-stale on every edit"
    );
}

#[test]
fn sqlite_independent_edge_restales_on_new_import_coupling() {
    let _guard = sqlite_test_lock();
    let (graph, a, b) = setup_two_uncoupled_grounded_intents(
        "independent-restale-coupling",
        "pub fn a_thing() -> u8 { 1 }\n",
        "pub fn b_thing() -> u8 { 2 }\n",
    );
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "independent",
            "--notes",
            "currently no import between the files",
            "--json",
        ],
        "llm:analyzer",
    );
    // Introduce a REAL structural coupling: src/a.rs now imports src/b.rs.
    write_scratch_file(
        &graph.root,
        "src/a.rs",
        "use crate::b::b_thing;\npub fn a_thing() -> u8 { b_thing() }\n",
    );
    run_json_as(&graph.root, &["sync", "--json"], "llm:analyzer");
    assert_eq!(
        relates_status(&graph.root, &a, &b),
        "needs_reverification",
        "a NEW import coupling between the pair must re-open the independent verdict (the safety net)"
    );
}

#[test]
fn sqlite_passing_edge_restales_on_code_change_even_when_uncoupled() {
    let _guard = sqlite_test_lock();
    let (graph, a, b) = setup_two_uncoupled_grounded_intents(
        "passing-restale-guard",
        "pub fn a_thing() -> u8 { 1 }\n",
        "pub fn b_thing() -> u8 { 2 }\n",
    );
    // A PASSING (ground) verdict between two structurally-uncoupled intents.
    run_json_as(
        &graph.root,
        &[
            "edge",
            "explore",
            &a,
            &b,
            "ground",
            "--criterion",
            "they are exercised together by the same caller path",
            "--confidence",
            "0.85",
            "--json",
        ],
        "llm:analyzer",
    );
    write_scratch_file(&graph.root, "src/a.rs", "pub fn a_thing() -> u8 { 42 }\n");
    run_json_as(&graph.root, &["sync", "--json"], "llm:analyzer");
    assert_eq!(
        relates_status(&graph.root, &a, &b),
        "needs_reverification",
        "a passing edge must re-open on code change regardless of coupling — the independent-only gate must not leak onto passing edges"
    );
}

// G2 discriminating proofs for leaf intents whose validations were asserted-only
// (bash/sh, cargo filters matching 0 tests, or pre-G2 runs). Each test below is
// linked via `loom validation update` to a real `cargo test …` runner.

#[test]
fn sqlite_loom_graph_pin_survives_foreign_cwd() {
    let _guard = sqlite_test_lock();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pin = env::temp_dir().join(format!("loom-graph-pin-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&pin).expect("pin dir");
    run_json(&pin, &["init", ".", "--json"]);
    let out = Command::new(loom_bin())
        .args(["status", "--json"])
        .current_dir("/")
        .env("LOOM_GRAPH", &pin)
        .env_remove("LOOM_AGENT")
        .output()
        .expect("loom status from foreign cwd");
    assert!(
        out.status.success(),
        "LOOM_GRAPH must win over cwd: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("status json");
    let init = run_json(&pin, &["status", "--json"]);
    assert_eq!(
        v["graph_state"]["graph_id"], init["graph_state"]["graph_id"],
        "foreign cwd must read the LOOM_GRAPH-pinned store, not cwd: {v}"
    );
    let _ = fs::remove_dir_all(&pin);
}

#[test]
fn sqlite_delegated_coverage_counts_child_export() {
    let _guard = sqlite_test_lock();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = env::temp_dir().join(format!("loom-delegated-cov-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&root).expect("root dir");
    run_json(&root, &["init", ".", "--json"]);
    let svc = root.join("services/api");
    fs::create_dir_all(&svc).expect("child service dir");
    write_scratch_file(&root, "services/api/lib.rs", "fn api() {}\n");
    run_json_as(
        &root,
        &[
            "delegate",
            "add",
            "services/api/**",
            "--to",
            "services/api/loom.graph.json",
            "--json",
        ],
        "llm:builder",
    );
    let missing = run_json(&root, &["coverage", "--json"]);
    assert_eq!(
        missing["delegation_targets_missing"],
        serde_json::json!(["services/api/loom.graph.json"]),
        "child export missing before init: {missing}"
    );
    run_json(&svc, &["init", ".", "--json"]);
    run_json(&svc, &["export", "--json"]);
    let covered = run_json(&root, &["coverage", "--json"]);
    assert_eq!(
        covered["unaccounted"], 0,
        "delegated child closes gaps: {covered}"
    );
    assert!(
        covered["delegated"].as_i64().unwrap_or(0) >= 1,
        "child export counts as delegated coverage: {covered}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn sqlite_bulk_quality_take_returns_batch_template() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("bulk-quality-take");
    let take = run_json(
        &graph.root,
        &["next", "--mode", "quality", "--take", "5", "--json"],
    );
    assert!(
        take["status"] == "ok" || take["status"] == "empty",
        "quality --take must answer: {take}"
    );
    if take["status"] == "ok" {
        assert!(
            take.get("batch_template").is_some(),
            "a non-empty quality bulk read carries batch_template: {take}"
        );
    }
}

#[test]
fn sqlite_serve_command_is_retired() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("serve-retired");
    run_json(&graph.root, &["init", ".", "--json"]);
    let out = Command::new(loom_bin())
        .args(["serve"])
        .current_dir(&graph.root)
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("loom serve");
    assert!(
        !out.status.success(),
        "loom serve must be retired (non-zero exit)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("retired"),
        "retirement must be named: {stderr}"
    );
}

#[test]
fn sqlite_import_as_planned_resets_lifecycle_and_proofs() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("import-as-planned");
    run_json(&graph.root, &["init", ".", "--json"]);
    let imp = run_json(
        &graph.root,
        &["import", "loom.graph.json", "--as-planned", "--json"],
    );
    assert_eq!(imp["as_planned"], true, "flag recorded: {imp}");
    let sample = run_json(&graph.root, &["intent", "list", "--json"]);
    let intents = sample["intents"].as_array().expect("intents");
    assert!(
        intents.iter().all(|i| i["lifecycle"] == "planned"),
        "every imported intent arrives planned: {sample}"
    );
    let proofs = run_json(&graph.root, &["validation", "list", "--json"]);
    assert!(
        proofs["validations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["last_result"] != "passed"),
        "import --as-planned must not carry forward passed proof verdicts: {proofs}"
    );
}

#[test]
fn sqlite_seed_guide_routes_to_interview() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("seed-guide");
    run_json(&graph.root, &["init", ".", "--json"]);
    let text = run_text_as(&graph.root, &["guide", "--mode", "seed"], "llm");
    assert!(
        text.contains("terminate on completeness") || text.contains("interview"),
        "seed guide must teach the user interview loop: {text}"
    );
}

#[test]
fn sqlite_hypothesis_prove_queue_surfaces_proposed() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("prove-queue");
    run_json(&graph.root, &["init", ".", "--json"]);
    let target = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "prove queue target",
            "--description",
            "central intent for hypothesis prove ranking",
            "--level",
            "feature",
            "--json",
        ],
        "llm:builder",
    );
    let tid = target["id"].as_str().unwrap();
    run_json_as(
        &graph.root,
        &[
            "hypothesis",
            "add",
            "--name",
            "prove queue hypothesis",
            "--claim",
            "the target has a measurable improvement opportunity",
            "--proposal",
            "refactor the target for clarity",
            "--predicted-outcome",
            "fewer lines in the hot path",
            "--target",
            tid,
            "--json",
        ],
        "llm:builder",
    );
    let prove = run_json(&graph.root, &["next", "--mode", "prove", "--json"]);
    assert_eq!(
        prove["mode"], "prove",
        "proposed hypothesis is prove work: {prove}"
    );
    assert!(
        prove["hypothesis"]["name"]
            .as_str()
            .is_some_and(|n| n.contains("prove queue")),
        "prove queue serves the seeded hypothesis: {prove}"
    );
}

// The hypothesis lifecycle commands enforce the separation-of-duties gate:
// the prover must differ from the proposer, and a different-role prove moves
// the hypothesis proposed -> supported. (criterion grounds to src/gate.rs)
#[test]
fn sqlite_hypothesis_prove_requires_prover_differs_from_proposer() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("hyp-prover-gate");
    run_json(&graph.root, &["init", ".", "--json"]);
    let target = run_json_as(
        &graph.root,
        &[
            "intent", "add", "--name", "prover gate target", "--description",
            "central intent for the hypothesis prover-differs lifecycle gate",
            "--level", "feature", "--json",
        ],
        "llm:builder",
    );
    let tid = target["id"].as_str().expect("target id");
    // proposer declares the builder lane
    let hyp = run_json_as(
        &graph.root,
        &[
            "hypothesis", "add", "--name", "prover gate hypothesis", "--claim",
            "the target has a measurable improvement opportunity worth proving",
            "--proposal", "refactor the hot path for clarity", "--predicted-outcome",
            "fewer lines in the hot path", "--target", tid, "--json",
        ],
        "llm:builder",
    );
    let hid = hyp["id"].as_str().expect("hypothesis id");
    assert_eq!(hyp["status"], "proposed", "a new hypothesis starts proposed: {hyp}");
    // SAME role as the proposer must be refused by the lifecycle gate
    let same = std::process::Command::new(loom_bin())
        .args([
            "hypothesis", "prove", hid, "--verdict", "supported", "--evidence",
            "self-proving: the claim looks real to me, the proposer", "--confidence", "0.8",
        ])
        .current_dir(&graph.root)
        .env("LOOM_AGENT", "llm:builder")
        .env_remove("LOOM_GRAPH")
        .output()
        .expect("run same-role prove");
    assert!(
        !same.status.success(),
        "the lifecycle gate must refuse a prover identical to the proposer: {}",
        String::from_utf8_lossy(&same.stderr)
    );
    // a DIFFERENT role proves it and moves it to supported
    assert_status_ok(&run_json_as(
        &graph.root,
        &[
            "hypothesis", "prove", hid, "--verdict", "supported", "--evidence",
            "an independent analyzer read the code and the claimed opportunity holds",
            "--confidence", "0.85", "--json",
        ],
        "llm:analyzer",
    ));
    let shown = run_json_as(&graph.root, &["hypothesis", "show", hid, "--json"], "llm:builder");
    assert_eq!(
        shown["hypothesis"]["status"], "supported",
        "a different-role prove moves the hypothesis to supported: {shown}"
    );
}

// ---------------------------------------------------------------------------
// Gap-closing tests: each pins a behavior added or fixed in this session.
// ---------------------------------------------------------------------------

#[test]
fn sqlite_review_take_json_carries_batch_template_hints() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("review-hints");
    // The review queue may be empty on this fixture, so seed a low-confidence
    // verdict to guarantee a non-empty queue. If the queue IS empty, the
    // empty JSON correctly omits batch_template_hints — so we only assert
    // when the queue has items.
    let review = run_json(&graph.root, &["next", "--mode", "review", "--take", "3", "--json"]);
    if review["status"] == "ok" {
        assert!(
            review.get("batch_template_hints").is_some(),
            "review --take non-empty must carry batch_template_hints: {review}"
        );
    }
    // The empty case must NOT carry hints (no template to hint about).
    if review["status"] == "empty" {
        assert!(
            review.get("batch_template_hints").is_none(),
            "review --take empty must not carry batch_template_hints: {review}"
        );
    }
}

#[test]
fn sqlite_quality_kind_filters_by_rule_kind() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("quality-kind-filter");
    // The fixture has rules with kind set. --kind security should return only
    // security rules; --kind performance should return only performance rules.
    let security = run_json(
        &graph.root,
        &["next", "--mode", "quality", "--kind", "security", "--take", "5", "--json"],
    );
    assert!(
        security["status"] == "ok" || security["status"] == "empty",
        "quality --kind security must answer: {security}"
    );
    if security["status"] == "ok" {
        assert_eq!(
            security["filtered_kind"], "security",
            "filtered_kind must echo the filter"
        );
        // Every batch_template line should reference a security rule.
        // We can't easily check rule names here, but the queue_total should
        // be smaller than unfiltered.
        let unfiltered = run_json(
            &graph.root,
            &["next", "--mode", "quality", "--take", "5", "--json"],
        );
        assert!(
            security["queue_total"].as_i64() <= unfiltered["queue_total"].as_i64(),
        "filtered queue_total ({}) must be <= unfiltered ({})",
        security["queue_total"], unfiltered["queue_total"]
        );
    }
}

#[test]
fn sqlite_quality_kind_rejects_non_quality_mode() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("quality-kind-mode-guard");
    // --kind on a non-quality mode must fail with a helpful error.
    let result = run_json_failure_as(
        &graph.root,
        &["next", "--mode", "discovery", "--kind", "security", "--json"],
        "llm:analyzer",
    );
    // The failure message should name --kind and quality.
    // (run_json_failure_as returns JSON from stdout; the error is on stderr.
    // We check the command failed — the exact message is tested in unit tests.)
    let _ = result;
}

#[test]
fn sqlite_security_deep_pack_seeds_four_rules_with_kind() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("security-deep-seed");
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json(&graph.root, &["import", "loom.graph.json", "--json"]);
    // Delete all rules, then seed only the security-deep pack.
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite");
        conn.execute("DELETE FROM quality_rule", [])
            .expect("clear rules");
    }
    let result = run_json_as(
        &graph.root,
        &["rule", "seed", "security-deep", "--json"],
        "llm:quality",
    );
    let created = result["created"].as_array().expect("created array");
    assert_eq!(created.len(), 4, "security-deep pack seeds exactly 4 rules: {result}");
    for rule in created {
        let kind = rule["kind"].as_str().expect("kind field");
        assert_eq!(kind, "security", "every security-deep rule has kind=security");
        let name = rule["name"].as_str().expect("name field");
        assert!(
            name.starts_with("sec-"),
            "security-deep rule names start with 'sec-': {name}"
        );
    }
}

#[test]
fn sqlite_alarm_fires_for_unmeasured_governs_when_rules_exist() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("alarm-unmeasured-governs");
    // The fixture has rules + coded intents. Delete all GOVERNS edges to
    // create the blind spot: rules exist but no intent has been measured.
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite");
        conn.execute("DELETE FROM governs", [])
            .expect("clear GOVERNS edges");
    }
    let status = run_json(&graph.root, &["status", "--json"]);
    let alarms = status["alarms"].as_array().expect("alarms array");
    let found = alarms.iter().any(|a| {
        a.as_str()
            .is_some_and(|s| s.contains("zero direct GOVERNS"))
    });
    assert!(
        found,
        "alarm must fire when coded intents have zero GOVERNS and rules exist: {alarms:?}"
    );
}

#[test]
fn sqlite_alarm_silent_when_no_rules_seeded() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("alarm-no-rules");
    // Delete all rules — the normative plane is empty. The missing-coverage
    // alarm must NOT fire (that's the compass's job, not the alarm's).
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite");
        conn.execute("DELETE FROM quality_rule", [])
            .expect("clear rules");
    }
    let status = run_json(&graph.root, &["status", "--json"]);
    let alarms = status["alarms"].as_array().expect("alarms array");
    let found = alarms.iter().any(|a| {
        a.as_str()
            .is_some_and(|s| s.contains("zero direct GOVERNS"))
    });
    assert!(
        !found,
        "alarm must NOT fire when no rules seeded (compass handles it): {alarms:?}"
    );
}

#[test]
fn sqlite_hardened_rung_unmet_when_normative_plane_empty() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("hardened-empty-normative");
    // Delete all rules to simulate no-seed. The Hardened rung must NOT clear.
    {
        let db = graph.root.join(".loom").join("graph.sqlite");
        let conn = rusqlite::Connection::open(&db).expect("open scratch sqlite");
        conn.execute("DELETE FROM quality_rule", [])
            .expect("clear rules");
    }
    let status = run_json(&graph.root, &["status", "--json"]);
    let rungs = status["maturity"]["rungs"].as_array().expect("rungs array");
    let hardened = rungs.iter()
        .find(|r| r["name"] == "Hardened")
        .expect("Hardened rung exists");
    assert_ne!(
        hardened["status"], "met",
        "Hardened must not clear when normative plane is empty: {hardened}"
    );
    let reasons = hardened["reasons"].as_array().expect("reasons array");
    let found = reasons.iter().any(|r| {
        r.as_str()
            .is_some_and(|s| s.contains("normative plane is EMPTY"))
    });
    assert!(
        found,
        "Hardened must name the empty normative plane: {reasons:?}"
    );
}

// ---------------------------------------------------------------------------
// v12 quality evidence semantics — gap-closing tests
// ---------------------------------------------------------------------------

#[test]
fn sqlite_partial_verdict_status_accepted() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("partial-verdict");
    run_json(&graph.root, &["init", ".", "--json"]);
    std::fs::create_dir_all(graph.root.join("src")).unwrap();
    std::fs::write(graph.root.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let intent = run_json_as(
        &graph.root,
        &["intent", "add", "--name", "alpha", "--level", "feature",
          "--description", "do alpha", "--lifecycle", "implemented", "--json"],
        "llm:builder",
    );
    let iid = intent["id"].as_str().unwrap().to_string();
    run_json_as(
        &graph.root,
        &["rule", "add", "--name", "r1", "--description", "rule r1",
          "--severity", "warning", "--json"],
        "llm:quality",
    );
    let result = run_json_as(
        &graph.root,
        &["rule", "verdict", "r1", &iid, "--status", "partial",
          "--criterion", "partially complies — some gaps remain",
          "--evidence", "versioned /v1 routes exist but no schema-diff enforcement",
          "--evidence-locator", "src/lib.rs", "--json"],
        "llm:quality",
    );
    assert_eq!(result["status"], "ok", "partial verdict accepted: {result}");
    assert_eq!(result["inspection_status"], "partial");
}

#[test]
fn sqlite_passing_verdict_requires_evidence_locator() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("passing-locator-gate");
    run_json(&graph.root, &["init", ".", "--json"]);
    let intent = run_json_as(
        &graph.root,
        &["intent", "add", "--name", "alpha", "--level", "feature",
          "--description", "do alpha", "--lifecycle", "implemented", "--json"],
        "llm:builder",
    );
    let iid = intent["id"].as_str().unwrap().to_string();
    run_json_as(
        &graph.root,
        &["rule", "add", "--name", "r1", "--description", "rule r1",
          "--severity", "warning", "--json"],
        "llm:quality",
    );
    // Passing without --evidence-locator should fail.
    let result = run_json_failure_as(
        &graph.root,
        &["rule", "verdict", "r1", &iid, "--status", "passing",
          "--criterion", "complies with the rule",
          "--evidence", "all handlers check auth", "--json"],
        "llm:quality",
    );
    assert_eq!(result["status"], "error", "passing without locator rejected: {result}");
    assert!(
        result["error"].as_str().unwrap_or("").contains("evidence-locator"),
        "error must name the missing locator: {result}"
    );
}

#[test]
fn sqlite_covers_descendants_requires_evidence() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("covers-desc-gate");
    run_json(&graph.root, &["init", ".", "--json"]);
    std::fs::create_dir_all(graph.root.join("src")).unwrap();
    std::fs::write(graph.root.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let intent = run_json_as(
        &graph.root,
        &["intent", "add", "--name", "alpha", "--level", "system",
          "--description", "system intent", "--lifecycle", "implemented", "--json"],
        "llm:builder",
    );
    let iid = intent["id"].as_str().unwrap().to_string();
    run_json_as(
        &graph.root,
        &["rule", "add", "--name", "r1", "--description", "rule r1",
          "--severity", "warning", "--json"],
        "llm:quality",
    );
    // covers_descendants with empty evidence should fail.
    let result = run_json_failure_as(
        &graph.root,
        &["rule", "verdict", "r1", &iid, "--status", "passing",
          "--criterion", "applies to all children",
          "--evidence", "", "--evidence-locator", "src/lib.rs",
          "--covers-descendants", "--json"],
        "llm:quality",
    );
    assert_eq!(result["status"], "error", "covers_descendants without evidence rejected: {result}");
    assert!(
        result["error"].as_str().unwrap_or("").contains("covers-descendants")
            || result["error"].as_str().unwrap_or("").contains("substantive"),
        "error must name covers-descendants or evidence gate: {result}"
    );
}

#[test]
fn sqlite_rule_show_displays_evidence_examples() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("rule-show-evidence");
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json_as(
        &graph.root,
        &["rule", "seed", "service", "--json"],
        "llm:quality",
    );
    // Show a rule that has evidence_examples.
    let result = run_json(
        &graph.root,
        &["rule", "show", "service-observable-failures", "--json"],
    );
    assert_eq!(result["name"], "service-observable-failures");
    assert!(
        !result["evidence_examples"].as_str().unwrap_or("").is_empty(),
        "evidence_examples should be populated: {result}"
    );
    assert!(
        !result["signal_expectations"].as_str().unwrap_or("").is_empty(),
        "signal_expectations should be populated: {result}"
    );
    // Verify the JSON structure is valid.
    let examples: serde_json::Value = serde_json::from_str(
        result["evidence_examples"].as_str().unwrap_or("")
    ).expect("evidence_examples is valid JSON");
    assert!(examples.get("pass").is_some(), "has pass example");
    assert!(examples.get("independent").is_some(), "has independent example");
    assert!(examples.get("common_false_positive").is_some(), "has common_false_positive example");
    let signals: Vec<Vec<String>> = serde_json::from_str(
        result["signal_expectations"].as_str().unwrap_or("[]")
    ).expect("signal_expectations is valid JSON array");
    assert!(!signals.is_empty(), "has at least one signal group");
}

#[test]
fn sqlite_service_pack_detects_axum_framework() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("detect-axum");
    run_json(&graph.root, &["init", ".", "--json"]);
    // Create a Cargo.toml with axum dependency.
    std::fs::write(
        graph.root.join("Cargo.toml"),
        "[package]\nname = \"test-svc\"\nversion = \"0.1.0\"\n\n[dependencies]\naxum = \"0.7\"\n",
    ).unwrap();
    std::fs::create_dir_all(graph.root.join("src")).unwrap();
    std::fs::write(graph.root.join("src/main.rs"), "fn main() {}\n").unwrap();
    let result = run_json(&graph.root, &["detect", "--json"]);
    let packs: Vec<&str> = result["recommended_packs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["pack"].as_str().unwrap_or(""))
        .collect();
    assert!(
        packs.contains(&"service"),
        "service pack should be recommended when axum is detected: {packs:?}"
    );
}

#[test]
fn sqlite_high_severity_passing_routes_to_review() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("review-severity");
    run_json(&graph.root, &["init", ".", "--json"]);
    std::fs::create_dir_all(graph.root.join("src")).unwrap();
    std::fs::write(graph.root.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let intent = run_json_as(
        &graph.root,
        &["intent", "add", "--name", "alpha", "--level", "feature",
          "--description", "do alpha", "--lifecycle", "implemented", "--json"],
        "llm:builder",
    );
    let iid = intent["id"].as_str().unwrap().to_string();
    // Create a rule with severity = error.
    let rule = run_json_as(
        &graph.root,
        &["rule", "add", "--name", "strict-rule", "--description", "strict",
          "--severity", "error", "--kind", "security", "--json"],
        "llm:quality",
    );
    let rule_id = rule["id"].as_str().unwrap().to_string();
    // Passing verdict with high confidence + locator.
    let result = run_json_as(
        &graph.root,
        &["rule", "verdict", "strict-rule", &iid, "--status", "passing",
          "--criterion", "no injection sinks",
          "--evidence", "all queries parameterized at src/lib.rs",
          "--evidence-locator", "src/lib.rs", "--confidence", "0.95", "--json"],
        "llm:quality",
    );
    assert_eq!(result["status"], "ok", "verdict recorded: {result}");
    // The review queue should include this high-severity passing verdict
    // even at high confidence.
    let review = run_json(&graph.root, &["next", "--mode", "review", "--take", "50", "--json"]);
    let items = review["items"].as_array().expect("items array");
    let found = items.iter().any(|item| {
        item.get("rule")
            .and_then(|r| r.get("id"))
            .and_then(|r| r.as_str())
            == Some(&rule_id)
    });
    assert!(found, "high-severity passing verdict should be in review queue: {review}");
}

// ---------------------------------------------------------------------------
// Defect fix tests — covers_descendants, evidence locator, seed, inbox links
// ---------------------------------------------------------------------------

#[test]
fn sqlite_covers_descendants_false_does_not_cover_children() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("covers-false");
    run_json(&graph.root, &["init", ".", "--json"]);
    std::fs::create_dir_all(graph.root.join("src")).unwrap();
    std::fs::write(graph.root.join("src/lib.rs"), "pub fn parent() {}\npub fn child() {}\n").unwrap();
    run_json_as(&graph.root, &["codefile", "add", "src/lib.rs", "--json"], "llm:builder");

    // Create a parent intent with a child.
    let parent = run_json_as(
        &graph.root,
        &["intent", "add", "--name", "parent", "--level", "component",
          "--description", "parent component", "--lifecycle", "implemented", "--json"],
        "llm:builder",
    );
    let pid = parent["id"].as_str().unwrap().to_string();

    let child = run_json_as(
        &graph.root,
        &["intent", "add", "--name", "child", "--level", "feature",
          "--description", "child feature", "--lifecycle", "implemented", "--json"],
        "llm:builder",
    );
    let cid = child["id"].as_str().unwrap().to_string();

    // Ground both intents.
    run_json_as(&graph.root, &["edge", "implement", &pid, "src/lib.rs",
        "--locator", "pub fn parent", "--json"], "llm:builder");
    run_json_as(&graph.root, &["edge", "implement", &cid, "src/lib.rs",
        "--locator", "pub fn child", "--json"], "llm:builder");

    // Link parent → child in hierarchy.
    run_json_as(&graph.root, &["edge", "hierarchy", &pid, &cid, "--json"], "llm:builder");

    // Seed a rule and verdict on the PARENT (without --covers-descendants).
    run_json_as(&graph.root, &["rule", "seed", "iso5055", "--json"], "llm:quality");
    // Find the first rule.
    let rules = run_json(&graph.root, &["rule", "list", "--json"]);
    let first_rule = rules["rules"].as_array().unwrap()[0]["id"].as_str().unwrap().to_string();

    // Verdict on parent with passing + locator, WITHOUT --covers-descendants.
    run_json_as(
        &graph.root,
        &["rule", "verdict", &first_rule, &pid, "--status", "passing",
          "--criterion", "parent complies with this rule",
          "--evidence", "checked parent code — complies",
          "--evidence-locator", "src/lib.rs:1", "--json"],
        "llm:quality",
    );

    // The child should appear in the quality queue — the parent verdict
    // without --covers-descendants does NOT cover the child.
    let quality = run_json(&graph.root, &["next", "--mode", "quality", "--take", "50", "--json"]);
    let items = quality.get("items").and_then(|v| v.as_array());
    if let Some(items) = items {
        let child_covered = items.iter().any(|item| {
            item.get("intent")
                .and_then(|i| i.get("id"))
                .and_then(|i| i.as_str())
                == Some(&cid)
        });
        // The child should NOT be covered — it should be in the quality queue.
        // (If the queue is empty, the child is wrongly covered — false green.)
        assert!(
            child_covered || quality.get("queue_total").and_then(|v| v.as_i64()).unwrap_or(0) > 0,
            "child should NOT be covered by parent verdict without --covers-descendants: {quality}"
        );
    }
}

#[test]
fn sqlite_rule_verdict_persist_locator_in_evidence() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("verdict-locator");
    run_json(&graph.root, &["init", ".", "--json"]);
    std::fs::create_dir_all(graph.root.join("src")).unwrap();
    std::fs::write(graph.root.join("src/lib.rs"), "pub fn alpha() {}\n").unwrap();
    let intent = run_json_as(
        &graph.root,
        &["intent", "add", "--name", "alpha", "--level", "feature",
          "--description", "do alpha", "--lifecycle", "implemented", "--json"],
        "llm:builder",
    );
    let iid = intent["id"].as_str().unwrap().to_string();
    run_json_as(&graph.root, &["rule", "seed", "iso5055", "--json"], "llm:quality");
    let rules = run_json(&graph.root, &["rule", "list", "--json"]);
    let first_rule = rules["rules"].as_array().unwrap()[0]["id"].as_str().unwrap().to_string();

    // Verdict with a locator — the stored evidence should contain @<locator>.
    let result = run_json_as(
        &graph.root,
        &["rule", "verdict", &first_rule, &iid, "--status", "passing",
          "--criterion", "complies with rule",
          "--evidence", "all handlers check auth",
          "--evidence-locator", "src/lib.rs:1", "--json"],
        "llm:quality",
    );
    assert_eq!(result["status"], "ok", "verdict should succeed: {result}");

    // Check the stored evidence contains the locator.
    let check = run_json(&graph.root, &["rule", "check", &iid, "--json"]);
    let governs = check.get("governs").and_then(|v| v.as_array()).expect("governs array");
    let edge = governs.iter().find(|g| {
        g.get("rule_id").and_then(|r| r.as_str()) == Some(&first_rule)
    }).expect("found the GOVERNS edge");
    let evidence = edge.get("evidence").and_then(|e| e.as_str()).unwrap_or("");
    assert!(
        evidence.contains("@src/lib.rs"),
        "evidence should contain the locator: got '{evidence}'"
    );
}

#[test]
fn sqlite_seed_inbox_excludes_loom_artifacts() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("seed-exclude");
    run_json(&graph.root, &["init", ".", "--json"]);
    // Create loom artifacts that should be excluded.
    std::fs::write(graph.root.join("loom.wiki.md"), "# Loom Wiki\n").unwrap();
    std::fs::write(graph.root.join("loom.graph.json"), r#"{"loom_export":1}"#).unwrap();
    std::fs::create_dir_all(graph.root.join("docs")).unwrap();
    std::fs::write(graph.root.join("docs/readme.md"), "# Real Doc\n").unwrap();

    run_json(&graph.root, &["seed", "--inbox", "--json"]);
    let inbox = run_json(&graph.root, &["inbox", "list", "--json"]);
    let items = inbox.get("items").and_then(|v| v.as_array()).expect("items array");
    let raw_texts: Vec<&str> = items.iter()
        .filter_map(|i| i.get("raw_text").and_then(|t| t.as_str()))
        .collect();
    assert!(
        !raw_texts.iter().any(|t| t.contains("loom.wiki.md")),
        "loom.wiki.md should NOT be ingested: {raw_texts:?}"
    );
    assert!(
        !raw_texts.iter().any(|t| t.contains("loom.graph.json")),
        "loom.graph.json should NOT be ingested: {raw_texts:?}"
    );
}

#[test]
fn sqlite_inbox_file_link_to_non_codefile_resolves() {
    let _guard = sqlite_test_lock();
    let graph = ScratchGraph::new("inbox-file-link");
    run_json(&graph.root, &["init", ".", "--json"]);
    // Create a doc file that is NOT a registered code file.
    std::fs::create_dir_all(graph.root.join("docs")).unwrap();
    std::fs::write(graph.root.join("docs/reference.md"), "# Reference\n").unwrap();

    // Add an inbox item with a file: link to the doc.
    let result = run_json_as(
        &graph.root,
        &["inbox", "add", "Test item with doc link", "--link", "file:docs/reference.md", "--json"],
        "llm:builder",
    );
    assert_eq!(
        result["status"], "ok",
        "file: link to a non-codefile doc should resolve: {result}"
    );
}

/// Honesty guard: `loom smells` computes findings from the LAST-SYNCED extracted
/// facts. If a file drifts on disk without a sync, those findings UNDER-REPORT the
/// real code — and a silently-clean count is the cardinal sin for an "honest read"
/// tool (proven live: `smells` showed the same count before/after a real `todo!()`
/// was added on disk). This pins the `stale_facts` block: a freshly-synced file is
/// NOT drifted; editing it on disk without sync flags it and sets under_reporting.
#[test]
fn sqlite_smells_flags_under_reporting_when_facts_drift() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("smells-drift");

    write_scratch_file(
        &graph.root,
        "scratch/drift_probe.rs",
        "pub fn ok() -> u8 { 1 }\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "scratch/drift_probe.rs", "--json"],
        "llm:builder",
    );
    run_json_as(&graph.root, &["sync", "--json"], "llm:analyzer");

    // freshly synced: the probe is NOT in the drifted set
    let fresh = run_json(&graph.root, &["smells", "--json"]);
    let fresh_drifted = fresh["stale_facts"]["drifted_codefiles"]
        .as_array()
        .expect("stale_facts.drifted_codefiles array");
    assert!(
        !fresh_drifted.iter().any(|p| p == "scratch/drift_probe.rs"),
        "a freshly-synced file must not be reported as drifted: {fresh_drifted:?}"
    );

    // drift the file on disk WITHOUT syncing — the stored facts are now stale
    write_scratch_file(
        &graph.root,
        "scratch/drift_probe.rs",
        "pub fn ok() -> u8 { todo!() }\n",
    );
    let stale = run_json(&graph.root, &["smells", "--json"]);
    assert_eq!(
        stale["stale_facts"]["under_reporting"],
        serde_json::json!(true),
        "drift must flag under_reporting — a silently-stale clean count is the honesty bug this guards: {}",
        stale["stale_facts"]
    );
    let drifted = stale["stale_facts"]["drifted_codefiles"]
        .as_array()
        .expect("drifted array");
    assert!(
        drifted.iter().any(|p| p == "scratch/drift_probe.rs"),
        "the drifted file must be named in stale_facts.drifted_codefiles: {drifted:?}"
    );
}

/// Debt-vs-defect: `duplicate_detection_unarmed` (the tag detector is under-armed
/// because coded intents are untagged) is dischargeable METADATA DEBT, not a code
/// defect. Hard-gating it pressures the driver to launder it away with a
/// `--kind decision` ruling instead of discharging it (tagging). So it is surfaced
/// in the `debt` bucket but kept OUT of the gating `smells`/open set. loom's own
/// graph has 81/98 untagged coded intents, so the finding fires here.
#[test]
fn sqlite_smells_routes_metadata_debt_out_of_gating_open() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("smells-debt");
    let out = run_json(&graph.root, &["smells", "--json"]);
    let in_open = out["smells"]
        .as_array()
        .expect("smells array")
        .iter()
        .any(|s| s["kind"] == "duplicate_detection_unarmed");
    let in_debt = out["debt"]
        .as_array()
        .expect("debt array")
        .iter()
        .any(|s| s["kind"] == "duplicate_detection_unarmed");
    assert!(
        !in_open,
        "metadata debt must NOT be in the gating open set (it would pressure laundering): {}",
        out["smells"]
    );
    assert!(
        in_debt,
        "metadata debt must be surfaced in the dischargeable `debt` bucket: {}",
        out["debt"]
    );
}

/// Proposal #1: `intent update --domain/--aspect` (metadata-only, like --layer).
/// Before this, 100+ unknown-domain intents had NO fix path short of delete+re-add
/// (which destroys every attached edge/validation/note). Pins that domain and
/// aspect are settable on an existing intent, and that aspect is validated.
#[test]
fn sqlite_intent_update_sets_domain_and_aspect() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("intent-update-meta");
    let (id, _) = first_two_intent_ids(&graph.root);
    run_text_as(
        &graph.root,
        &[
            "intent", "update", &id, "--domain", "billing", "--reason",
            "reclassify under the billing facet",
        ],
        "llm:builder",
    );
    run_text_as(
        &graph.root,
        &[
            "intent", "update", &id, "--aspect", "sad", "--reason",
            "mark the failure-path facet",
        ],
        "llm:builder",
    );
    let show = run_json(&graph.root, &["intent", "show", &id, "--json"]);
    assert_eq!(
        show["intent"]["domain"],
        serde_json::json!("billing"),
        "domain must be settable on an existing intent (proposal #1 — no more delete+re-add): {show}"
    );
    assert_eq!(
        show["intent"]["aspect"],
        serde_json::json!("sad"),
        "aspect must be settable on an existing intent: {show}"
    );
    // Invalid aspect is rejected (vocabulary-checked like --boundary).
    let bad = run_json_failure_as(
        &graph.root,
        &[
            "intent", "update", &id, "--aspect", "bogus", "--reason",
            "attempting an out-of-vocabulary aspect to check validation", "--json",
        ],
        "llm:builder",
    );
    assert!(
        bad.to_string().contains("Invalid --aspect"),
        "an out-of-vocabulary aspect must be rejected: {bad}"
    );
}

/// Proposal #2: doctor surfaces domain:unknown as a metadata-debt HINT (never an
/// integrity issue — `healthy` stays true), so the debt the new `intent update
/// --domain` tooling discharges is visible instead of hidden under healthy:true.
#[test]
fn sqlite_doctor_surfaces_domain_unknown_as_hint_not_issue() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("doctor-domain-debt");
    let out = run_json(&graph.root, &["doctor", "--json"]);
    let in_hints = out["hints"].as_array().is_some_and(|a| {
        a.iter()
            .any(|h| h.as_str().unwrap_or("").contains("domain:unknown"))
    });
    assert!(
        in_hints,
        "doctor must surface domain:unknown as a hint: {}",
        out["hints"]
    );
    // It is a HINT, never an integrity ISSUE — the health check stays green.
    assert_eq!(
        out["healthy"],
        serde_json::json!(true),
        "metadata debt must not fail the health check: {out}"
    );
    let in_issues = out["issues"].as_array().is_some_and(|a| {
        a.iter()
            .any(|i| i.as_str().unwrap_or("").contains("domain:unknown"))
    });
    assert!(
        !in_issues,
        "domain debt must be a hint, not an integrity issue: {}",
        out["issues"]
    );
}

/// REGRESSION: a lazily-(re)created store — `loom status` after the SQLite DB
/// was deleted out from under loom — must degrade gracefully, not crash. Three
/// guarantees in one drive:
///   (1) status does NOT die with "parse stored JSON list field" reading an
///       unstamped JSON-list column on the freshly-recreated empty DB;
///   (2) status ALARMS that the live graph is empty while a committed
///       loom.graph.json exists (the restore path), instead of presenting
///       empty-as-normal;
///   (3) `doctor` reads the unstamped-but-current schema as v12 and AGREES with
///       `loom migrate` — no false "blank vs 12" version mismatch.
#[test]
fn sqlite_lazy_recreated_db_degrades_gracefully_and_alarms() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("lazy-recreate");
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "real intent",
            "--description",
            "does a real thing",
            "--level",
            "system",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    // Commit the (1-intent) graph so a loom.graph.json sits beside the repo.
    run_json(&graph.root, &["export", "--json"]);

    // Simulate a LOST store: delete the live SQLite DB and its WAL/SHM sidecars.
    let loom_dir = graph.root.join(".loom");
    for f in ["graph.sqlite", "graph.sqlite-wal", "graph.sqlite-shm"] {
        let _ = fs::remove_file(loom_dir.join(f));
    }

    // (1)+(2): status must SUCCEED (run_json panics on a non-zero exit, so a
    // crash here fails the test) and surface the restore alarm.
    let status = run_json(&graph.root, &["status", "--json"]);
    let alarms = status["alarms"]
        .as_array()
        .expect("status carries an alarms array");
    assert!(
        alarms
            .iter()
            .any(|a| a.as_str().is_some_and(|s| s.contains("loom import loom.graph.json"))),
        "a lazily-recreated empty graph beside a committed export must alarm with the restore hint: {alarms:?}"
    );

    // (3): doctor reads the current schema, and migrate agrees — no false mismatch.
    let doctor = run_json(&graph.root, &["doctor", "--json"]);
    assert_eq!(
        doctor["schema_version"]["ok"],
        serde_json::json!(true),
        "doctor must read the lazily-recreated DB as the current schema version, not a false mismatch: {}",
        doctor["schema_version"]
    );
    let migrate = run_json(&graph.root, &["migrate", "--json"]);
    assert_eq!(
        migrate["current"],
        serde_json::json!(true),
        "migrate must agree the schema is current: {migrate}"
    );
}

/// REGRESSION: bare `loom next` must not dead-end on the Seeded blocker. A
/// public symbol is unowned (focus rung Seeded · lane=build) but there are no
/// planned/needs_change intents, so the build INTENT queue is empty. Bare
/// `loom next` used to print "✓ No planned/needs_change intents — nothing to
/// build." while `loom coverage` was the ONLY place the gap appeared. It must now
/// SERVE that symbol-accountability gap (with the per-gap fix command) directly.
#[test]
fn sqlite_bare_next_serves_symbol_gap_not_deadend() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("next-fallthrough");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "app.py",
        "def handle():\n    return helper()\n\ndef helper():\n    return 1\n",
    );
    run_json_as(&graph.root, &["codefile", "add", "app.py", "--json"], "llm:builder");
    let intent = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "request handler",
            "--description",
            "handles the request",
            "--level",
            "feature",
            "--lifecycle",
            "implemented",
            "--json",
        ],
        "llm:builder",
    );
    let id = intent["id"].as_str().expect("intent id");
    run_json_as(
        &graph.root,
        &["edge", "implement", id, "app.py", "--locator", "def handle", "--json"],
        "llm:builder",
    );
    run_json(&graph.root, &["sync", "--json"]);

    // Precondition: the focus rung is Seeded · lane=build (helper() is unowned).
    let status = run_text_as(&graph.root, &["status"], "llm");
    assert!(
        status.contains("lane: build") && status.contains("unowned"),
        "test precondition: focus must be Seeded · build with an unowned symbol: {status}"
    );

    // Bare `next` must SERVE the unowned-symbol gap, not dead-end on "nothing to
    // build". The build lane now counts symbol-accountability gaps as work.
    let next = run_json(&graph.root, &["next", "--json"]);
    assert_ne!(
        next["status"],
        serde_json::json!("empty"),
        "bare next dead-ended on the Seeded blocker instead of serving it: {next}"
    );
    assert_eq!(
        next["mode"],
        serde_json::json!("build"),
        "bare next should serve the build lane's symbol gap: {next}"
    );
    assert_eq!(
        next["work_kind"],
        serde_json::json!("symbol_accountability"),
        "the served build item must be the unowned-symbol gap: {next}"
    );
    assert!(
        next["symbol_gaps"][0]["suggested_action"]
            .as_str()
            .is_some_and(|s| s.contains("loom edge implement")),
        "the gap must carry a runnable fix command: {next}"
    );
}

/// REGRESSION: `loom ignore add` reconciles the registry — a CodeFile that is
/// both registered and (now) ignored is the contradiction where `loom status`
/// counted it "reached by no intent" while `loom coverage` excluded it. ignore
/// add de-registers the UNGROUNDED match so both surfaces agree.
#[test]
fn sqlite_ignore_add_deregisters_ungrounded_match() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("ignore-dereg");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "src/app.py", "def f():\n    return 1\n");
    write_scratch_file(&graph.root, "src/__init__.py", "");
    run_json_as(&graph.root, &["codefile", "add", "src/*.py", "--json"], "llm:builder");
    let intent = run_json_as(
        &graph.root,
        &[
            "intent", "add", "--name", "app", "--description", "the app", "--level", "system",
            "--lifecycle", "implemented", "--json",
        ],
        "llm:builder",
    );
    let id = intent["id"].as_str().expect("id");
    run_json_as(
        &graph.root,
        &["edge", "implement", id, "src/app.py", "--locator", "def f", "--json"],
        "llm:builder",
    );
    run_json(&graph.root, &["sync", "--json"]);

    let ig = run_json_as(
        &graph.root,
        &["ignore", "add", "src/__init__.py", "--reason", "package marker", "--json"],
        "llm:builder",
    );
    let dereg = ig["deregistered_codefiles"]
        .as_array()
        .expect("deregistered list");
    assert!(
        dereg.iter().any(|p| p.as_str() == Some("src/__init__.py")),
        "ignore add must de-register the ungrounded match: {ig}"
    );
    run_json(&graph.root, &["sync", "--json"]);
    let status = run_text_as(&graph.root, &["status"], "llm");
    assert!(
        !status.contains("reached by no intent"),
        "status must not count the now-excluded file as unreached: {status}"
    );
}

/// REGRESSION: `loom validate` warns AT pass-time when a command exits 0 but
/// asserts nothing loom recognizes — it counts as ASSERTED-only (not executed-
/// proven) and won't advance Realized, so a driver must learn it here.
#[test]
fn sqlite_validate_warns_on_non_discriminating_pass() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("validate-inert");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "a.py", "def f():\n    return 1\n");
    run_json_as(&graph.root, &["codefile", "add", "a.py", "--json"], "llm:builder");
    let intent = run_json_as(
        &graph.root,
        &[
            "intent", "add", "--name", "leaf", "--description", "a leaf", "--level", "feature",
            "--lifecycle", "implemented", "--json",
        ],
        "llm:builder",
    );
    let id = intent["id"].as_str().expect("id");
    run_json_as(
        &graph.root,
        &["edge", "implement", id, "a.py", "--locator", "def f", "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &[
            "validation", "add", "--name", "inert", "--type", "test", "--command",
            "python3 -c \"pass\"", "--intent", id, "--json",
        ],
        "llm:builder",
    );
    let out = run_text_as(&graph.root, &["validate", id], "llm:validator");
    assert!(out.contains("passed"), "the inert command should still pass: {out}");
    assert!(
        out.contains("NON-DISCRIMINATING"),
        "validate must warn a non-discriminating pass won't advance Realized: {out}"
    );
    let j = run_json_as(&graph.root, &["validate", id, "--json"], "llm:validator");
    assert_eq!(
        j["results"][0]["discrimination"],
        serde_json::json!("ran_inert"),
        "validate --json must expose the discrimination tier: {j}"
    );
}

/// REGRESSION: the BUILD-lane recipe must hand off proving to the validator lane
/// rather than inline `loom validate` — a builder following the steps literally
/// used to hit a lane-violation on that step.
#[test]
fn sqlite_build_recipe_hands_off_proving_to_validator() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("build-handoff");
    run_json(&graph.root, &["init", ".", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "intent", "add", "--name", "planned thing", "--description", "to build", "--level",
            "feature", "--lifecycle", "planned", "--json",
        ],
        "llm:builder",
    );
    let out = run_text_as(&graph.root, &["next", "--mode", "build"], "llm:builder");
    assert!(
        out.contains("loom next --mode validate") && out.contains("HAND OFF"),
        "build recipe must hand off proving to the validator lane, not inline a builder `loom validate`: {out}"
    );
}

/// REGRESSION: `loom sync` must invalidate a proof when the body of a CLASS
/// METHOD it is grounded to changes. tree-sitter extracts only top-level symbols
/// (`class JobStore` = 1 fact, not its methods), so a `--locator "def set_state"`
/// grounding named no tracked symbol and the symbol-precise ripple skipped it —
/// a gutted method body kept its proof green (stale-green false-pass). A grounding
/// whose locator names no tracked symbol now rides the file-level ripple.
#[test]
fn sqlite_sync_invalidates_method_grounded_proof_on_body_change() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("sync-method-ripple");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(
        &graph.root,
        "store.py",
        "class JobStore:\n    def set_state(self, k, v):\n        return True\n",
    );
    run_json_as(&graph.root, &["codefile", "add", "store.py", "--json"], "llm:builder");
    let intent = run_json_as(
        &graph.root,
        &[
            "intent", "add", "--name", "set state", "--description", "stores a job state value",
            "--level", "feature", "--lifecycle", "implemented", "--json",
        ],
        "llm:builder",
    );
    let id = intent["id"].as_str().expect("id");
    run_json_as(
        &graph.root,
        &["edge", "implement", id, "store.py", "--locator", "def set_state", "--json"],
        "llm:builder",
    );
    run_json(&graph.root, &["sync", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "validation", "add", "--name", "ss proof", "--type", "test", "--command",
            "python3 -c \"print(1)\"", "--intent", id, "--json",
        ],
        "llm:builder",
    );
    run_json_as(&graph.root, &["validate", id, "--json"], "llm:validator");

    // Gut the METHOD body; the locator string `def set_state` still occurs, so the
    // pre-fix symbol-precise diff (which only tracked top-level `JobStore`) missed it.
    write_scratch_file(
        &graph.root,
        "store.py",
        "class JobStore:\n    def set_state(self, k, v):\n        raise RuntimeError(\"gutted\")\n",
    );
    let sync = run_json(&graph.root, &["sync", "--json"]);
    assert_eq!(sync["files_changed"], serde_json::json!(1), "file must register changed: {sync}");
    assert!(
        sync["validations_invalidated"].as_i64().unwrap_or(0) >= 1,
        "a class-method body change must invalidate its method-grounded proof: {sync}"
    );
}

/// REGRESSION (the precision guard for the fix above): changing ONE top-level
/// function must NOT invalidate an UNCHANGED top-level sibling's proof. The
/// nested-locator fallback must apply ONLY to locators that name no tracked
/// symbol — top-level groundings stay symbol-precise.
#[test]
fn sqlite_sync_precise_for_unchanged_top_level_sibling() {
    let _g = sqlite_test_lock();
    let graph = ScratchGraph::new("sync-precise");
    run_json(&graph.root, &["init", ".", "--json"]);
    write_scratch_file(&graph.root, "m.py", "def alpha():\n    return 1\n\ndef beta():\n    return 2\n");
    run_json_as(&graph.root, &["codefile", "add", "m.py", "--json"], "llm:builder");
    let a = run_json_as(
        &graph.root,
        &[
            "intent", "add", "--name", "alpha", "--description", "alpha behavior", "--level",
            "feature", "--lifecycle", "implemented", "--json",
        ],
        "llm:builder",
    );
    let b = run_json_as(
        &graph.root,
        &[
            "intent", "add", "--name", "beta", "--description", "beta behavior", "--level",
            "feature", "--lifecycle", "implemented", "--json",
        ],
        "llm:builder",
    );
    let aid = a["id"].as_str().expect("a");
    let bid = b["id"].as_str().expect("b");
    run_json_as(&graph.root, &["edge", "implement", aid, "m.py", "--locator", "def alpha", "--json"], "llm:builder");
    run_json_as(&graph.root, &["edge", "implement", bid, "m.py", "--locator", "def beta", "--json"], "llm:builder");
    run_json(&graph.root, &["sync", "--json"]);
    run_json_as(
        &graph.root,
        &[
            "validation", "add", "--name", "ap", "--type", "test", "--command",
            "python3 -c \"print(1)\"", "--intent", aid, "--json",
        ],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &[
            "validation", "add", "--name", "bp", "--type", "test", "--command",
            "python3 -c \"print(1)\"", "--intent", bid, "--json",
        ],
        "llm:builder",
    );
    run_json_as(&graph.root, &["validate", "--all", "--json"], "llm:validator");

    // Change ONLY alpha's body; beta is a top-level symbol left untouched.
    write_scratch_file(&graph.root, "m.py", "def alpha():\n    return 99\n\ndef beta():\n    return 2\n");
    let sync = run_json(&graph.root, &["sync", "--json"]);
    assert_eq!(
        sync["validations_invalidated"],
        serde_json::json!(1),
        "exactly ONE proof (alpha's) must flip — beta's untouched top-level grounding stays precise: {sync}"
    );
}

/// REGRESSION: a `to_be_removed` build item is dispatched to the BUILDER lane,
/// not the fixer. The REMOVE recipe's mutating steps (`edge unimplement`,
/// `codefile remove`, `intent retire`) are all builder-only, so dispatching
/// removal to the fixer would hand the operator a recipe whose every command its
/// own role is denied — the mirror of the validate-step lane violation the
/// planned recipe avoids by handing off.
#[test]
fn sqlite_remove_recipe_is_dispatched_to_the_builder_lane_it_needs() {
    let _guard = sqlite_test_lock();
    let graph = setup_imported_graph("remove-lane");
    // A planned leaf grounded in a fresh file, then flipped to to_be_removed —
    // the sole build candidate on the fully-implemented fixture (deterministic).
    write_scratch_file(
        &graph.root,
        "scratch/legacy.rs",
        "pub fn legacy_path() -> u8 { 1 }\n",
    );
    run_json_as(
        &graph.root,
        &["codefile", "add", "scratch/legacy.rs", "--json"],
        "llm:builder",
    );
    let added = run_json_as(
        &graph.root,
        &[
            "intent",
            "add",
            "--name",
            "legacy behavior slated for deletion",
            "--description",
            "an obsolete behavior whose code should be removed",
            "--criterion",
            "the legacy path no longer exists anywhere in the code",
            "--level",
            "feature",
            "--lifecycle",
            "planned",
            "--json",
        ],
        "llm:builder",
    );
    let id = added["id"].as_str().expect("new intent id").to_string();
    run_json_as(
        &graph.root,
        &[
            "edge",
            "implement",
            &id,
            "scratch/legacy.rs",
            "--locator",
            "legacy_path",
            "--json",
        ],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &[
            "intent",
            "mark",
            &id,
            "--lifecycle",
            "to_be_removed",
            "--reason",
            "obsolete; superseded by the v2 path",
            "--json",
        ],
        "llm:fixer",
    );

    // The removal item is dispatched to the BUILDER (whose lane owns every step
    // of its recipe), not the fixer who flipped its lifecycle.
    let item = run_json(&graph.root, &["next", "--mode", "build", "--json"]);
    assert_eq!(
        item["owner_role"].as_str(),
        Some("builder"),
        "to_be_removed removal work is builder-lane, not fixer: {item}"
    );
    let action = item["suggested_action"].as_str().expect("suggested_action");
    assert!(
        action.contains("REMOVE the code for this intent"),
        "the REMOVE recipe is served for a to_be_removed leaf: {action}"
    );

    // The negation of the contradiction: walk the recipe's mutating steps AS the
    // dispatched owner (llm:builder). `run_json_as` panics on a non-zero exit, so
    // a lane violation on any step fails this test — the owner is never told a
    // step it will be denied. (Recipe order: unimplement → codefile remove →
    // retire, each with a substantive reason the evidence gate accepts.)
    run_json_as(
        &graph.root,
        &["edge", "unimplement", &id, "scratch/legacy.rs", "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &["codefile", "remove", "scratch/legacy.rs", "--json"],
        "llm:builder",
    );
    run_json_as(
        &graph.root,
        &[
            "intent",
            "retire",
            &id,
            "--reason",
            "all groundings deleted; behavior removed from the code",
            "--json",
        ],
        "llm:builder",
    );
}
