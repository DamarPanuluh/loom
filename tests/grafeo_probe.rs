//! Empirical probes against grafeo 0.5.42 — answers the open questions behind
//! the "fully leverage Grafeo" gap analysis. Each probe prints a `PROBE <name>:
//! VERDICT — …` line; run with:
//!
//!     cargo test --test grafeo_probe -- --nocapture --test-threads=1
//!
//! These tests only hard-fail on probe-infrastructure errors (a setup INSERT
//! failing). Capability questions are *reported*, not asserted, because the
//! point is to learn what this grafeo version actually does — including the
//! known failure modes (edge-property filtering, CALL join-back).

use grafeo::{Config, GrafeoDB, Session, Value};
use std::collections::HashMap;

fn mem() -> (GrafeoDB, Session) {
    let db = GrafeoDB::new_in_memory();
    let session = db.session();
    (db, session)
}

fn rows(s: &Session, q: &str) -> Result<Vec<Vec<Value>>, String> {
    s.execute(q)
        .map(|r| r.rows().to_vec())
        .map_err(|e| e.to_string())
}

fn must(s: &Session, q: &str) -> Vec<Vec<Value>> {
    match rows(s, q) {
        Ok(r) => r,
        Err(e) => panic!("probe setup query failed: {e}\nquery: {q}"),
    }
}

fn s_val(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string(),
        Value::Int64(n) => n.to_string(),
        Value::Float64(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "NULL".into(),
        other => format!("{other:?}"),
    }
}

fn dump(label: &str, r: &grafeo::QueryResult) {
    let cols = r.columns.join(", ");
    let body: Vec<String> = r
        .rows()
        .iter()
        .take(10)
        .map(|row| row.iter().map(s_val).collect::<Vec<_>>().join(" | "))
        .collect();
    println!("  {label}: columns=[{cols}] rows={}", r.rows().len());
    for line in body {
        println!("    {line}");
    }
}

// ---------------------------------------------------------------------------
// Probe 1 — parameterized queries ($param via execute_with_params)
// ---------------------------------------------------------------------------

#[test]
fn probe_params() {
    let (_db, s) = mem();
    let mut p = HashMap::new();
    p.insert(
        "name".to_string(),
        Value::String("o'brien — tricky 'quotes'".into()),
    );
    p.insert("score".to_string(), Value::Float64(0.9));

    let ins = s.execute_with_params("INSERT (:T {name: $name, score: $score})", p.clone());
    match &ins {
        Ok(_) => println!("PROBE params(insert): VERDICT — INSERT with $params works"),
        Err(e) => println!("PROBE params(insert): VERDICT — INSERT with $params FAILED: {e}"),
    }

    let rd = s.execute_with_params("MATCH (n:T) WHERE n.name = $name RETURN n.name, n.score", p);
    match rd {
        Ok(r) if r.rows().len() == 1 => {
            dump("param read-back", &r);
            println!(
                "PROBE params(match): VERDICT — $params in WHERE works, quotes round-trip intact"
            );
        }
        Ok(r) => {
            dump("param read-back", &r);
            println!(
                "PROBE params(match): VERDICT — query ran but returned {} rows (expected 1)",
                r.rows().len()
            );
        }
        Err(e) => println!("PROBE params(match): VERDICT — $params in WHERE FAILED: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Probe 2 — the known bug, reproduced as a baseline: filtering a relationship
// by its own property (WHERE r.x and inline {x: ...}), including immediately
// after a write.
// ---------------------------------------------------------------------------

#[test]
fn probe_edge_property_filter() {
    let (_db, s) = mem();
    must(&s, "INSERT (:T {id: 'a'}), (:T {id: 'b'}), (:T {id: 'c'})");
    must(
        &s,
        "MATCH (a:T {id: 'a'}), (b:T {id: 'b'}) INSERT (a)-[:R {status: 'failing'}]->(b)",
    );
    must(
        &s,
        "MATCH (b:T {id: 'b'}), (c:T {id: 'c'}) INSERT (b)-[:R {status: 'passing'}]->(c)",
    );

    let mut where_counts = vec![];
    let mut inline_counts = vec![];
    for _ in 0..30 {
        where_counts.push(
            rows(
                &s,
                "MATCH (a:T)-[r:R]->(b:T) WHERE r.status = 'failing' RETURN a.id",
            )
            .map(|r| r.len())
            .unwrap_or(usize::MAX),
        );
        inline_counts.push(
            rows(&s, "MATCH (a:T)-[r:R {status: 'failing'}]->(b:T) RETURN a.id")
                .map(|r| r.len())
                .unwrap_or(usize::MAX),
        );
    }
    let uniq = |v: &Vec<usize>| {
        let mut u = v.clone();
        u.sort_unstable();
        u.dedup();
        u
    };
    println!(
        "PROBE edge_prop_filter(WHERE): VERDICT — expected always [1], observed counts {:?} over 30 runs",
        uniq(&where_counts)
    );
    println!(
        "PROBE edge_prop_filter(inline): VERDICT — expected always [1], observed counts {:?} over 30 runs",
        uniq(&inline_counts)
    );

    // Immediately-after-write variant: SET then filter on the new value.
    must(
        &s,
        "MATCH (a:T {id: 'a'})-[r:R]->(b:T {id: 'b'}) SET r.status = 'passing'",
    );
    let after = rows(
        &s,
        "MATCH (a:T)-[r:R]->(b:T) WHERE r.status = 'passing' RETURN a.id",
    )
    .map(|r| r.len());
    println!(
        "PROBE edge_prop_filter(after-SET): VERDICT — expected Ok(2), observed {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Probe 3 — transactions: read-your-writes on relationships inside an explicit
// transaction, edge-property filter inside the txn, commit visibility, rollback.
// ---------------------------------------------------------------------------

#[test]
fn probe_transactions() {
    let (_db, s) = mem();
    must(&s, "INSERT (:T {id: 'a'}), (:T {id: 'b'})");

    match rows(&s, "START TRANSACTION") {
        Ok(_) => println!("PROBE txn(start): VERDICT — START TRANSACTION accepted"),
        Err(e) => {
            println!("PROBE txn(start): VERDICT — START TRANSACTION FAILED: {e}");
            return;
        }
    }

    must(
        &s,
        "MATCH (a:T {id: 'a'}), (b:T {id: 'b'}) INSERT (a)-[:R {status: 'uninspected'}]->(b)",
    );

    // Read-your-writes on a relationship, inside the txn, by endpoints.
    let ryw = rows(
        &s,
        "MATCH (a:T {id: 'a'})-[r:R]->(b:T {id: 'b'}) RETURN r.status",
    );
    println!(
        "PROBE txn(read-your-writes edge): VERDICT — expected Ok 1 row 'uninspected', observed {:?}",
        ryw.map(|r| r.iter().map(|row| s_val(&row[0])).collect::<Vec<_>>())
    );

    must(
        &s,
        "MATCH (a:T {id: 'a'})-[r:R]->(b:T {id: 'b'}) SET r.status = 'failing'",
    );
    // The historically-flaky pattern, inside an explicit txn:
    let in_txn = rows(
        &s,
        "MATCH (a:T)-[r:R]->(b:T) WHERE r.status = 'failing' RETURN a.id",
    )
    .map(|r| r.len());
    println!(
        "PROBE txn(edge-prop filter in txn): VERDICT — expected Ok(1), observed {in_txn:?}"
    );

    match rows(&s, "COMMIT") {
        Ok(_) => println!("PROBE txn(commit): VERDICT — COMMIT accepted"),
        Err(e) => println!("PROBE txn(commit): VERDICT — COMMIT FAILED: {e}"),
    }
    let post = rows(
        &s,
        "MATCH (a:T {id: 'a'})-[r:R]->(b:T {id: 'b'}) RETURN r.status",
    );
    println!(
        "PROBE txn(post-commit): VERDICT — expected ['failing'], observed {:?}",
        post.map(|r| r.iter().map(|row| s_val(&row[0])).collect::<Vec<_>>())
    );

    // Rollback probe.
    if rows(&s, "START TRANSACTION").is_ok() {
        must(&s, "INSERT (:T {id: 'ghost'})");
        let seen_in_txn = rows(&s, "MATCH (n:T {id: 'ghost'}) RETURN n.id").map(|r| r.len());
        let rb = rows(&s, "ROLLBACK");
        let seen_after = rows(&s, "MATCH (n:T {id: 'ghost'}) RETURN n.id").map(|r| r.len());
        println!(
            "PROBE txn(rollback): VERDICT — in-txn {seen_in_txn:?} (expect Ok(1)), rollback {}, after {seen_after:?} (expect Ok(0))",
            if rb.is_ok() { "accepted" } else { "FAILED" }
        );
    }
}

// ---------------------------------------------------------------------------
// Probe 4 — MERGE: node upsert, ON CREATE/ON MATCH, and relationship MERGE
// (the part most likely to trip the edge bug).
// ---------------------------------------------------------------------------

#[test]
fn probe_merge() {
    let (_db, s) = mem();

    let m1 = rows(&s, "MERGE (n:T {id: 'x'}) ON CREATE SET n.created = 'yes'");
    let m2 = rows(&s, "MERGE (n:T {id: 'x'}) ON MATCH SET n.matched = 'yes'");
    match (&m1, &m2) {
        (Ok(_), Ok(_)) => {
            let n = must(&s, "MATCH (n:T {id: 'x'}) RETURN n.created, n.matched");
            println!(
                "PROBE merge(node): VERDICT — MERGE accepted; node count {} (expect 1), created/matched = {:?}",
                n.len(),
                n.first().map(|r| (s_val(&r[0]), s_val(&r[1])))
            );
        }
        _ => println!(
            "PROBE merge(node): VERDICT — MERGE FAILED: {:?} / {:?}",
            m1.err(),
            m2.err()
        ),
    }

    must(&s, "INSERT (:T {id: 'p'}), (:T {id: 'q'})");
    let e1 = rows(
        &s,
        "MATCH (a:T {id: 'p'}), (b:T {id: 'q'}) MERGE (a)-[r:R]->(b) ON CREATE SET r.status = 'uninspected'",
    );
    let e2 = rows(
        &s,
        "MATCH (a:T {id: 'p'}), (b:T {id: 'q'}) MERGE (a)-[r:R]->(b) ON MATCH SET r.status = 'seen'",
    );
    match (&e1, &e2) {
        (Ok(_), Ok(_)) => {
            let n = must(&s, "MATCH (a:T {id: 'p'})-[r:R]->(b:T {id: 'q'}) RETURN r.status");
            println!(
                "PROBE merge(edge): VERDICT — relationship MERGE accepted; edge count {} (expect 1), status {:?} (expect 'seen')",
                n.len(),
                n.first().map(|r| s_val(&r[0]))
            );
        }
        _ => println!(
            "PROBE merge(edge): VERDICT — relationship MERGE FAILED: {:?} / {:?}",
            e1.err(),
            e2.err()
        ),
    }
}

// ---------------------------------------------------------------------------
// Probe 4b — shapes the planned refactor builds on: MERGE+RETURN in one
// statement, inequality filters on edge and node properties, IS NULL on edges,
// and CREATE INDEX (creation, duplicate handling, queries still correct).
// ---------------------------------------------------------------------------

#[test]
fn probe_refactor_shapes() {
    let (_db, s) = mem();
    must(
        &s,
        "INSERT (:T {id: 'a', status: 'confirmed'}), (:T {id: 'b', status: 'deprecated'}), (:T {id: 'c', status: 'confirmed'})",
    );
    must(
        &s,
        "MATCH (a:T {id: 'a'}), (b:T {id: 'b'}) INSERT (a)-[:R {status: 'independent'}]->(b)",
    );
    must(
        &s,
        "MATCH (a:T {id: 'a'}), (c:T {id: 'c'}) INSERT (a)-[:R {status: 'passing'}]->(c)",
    );

    // MERGE + RETURN in one statement (needed for get_or_create in one trip).
    match s.execute(
        "MATCH (a:T {id: 'a'}), (c:T {id: 'c'}) \
         MERGE (a)-[r:R]->(c) ON CREATE SET r.status = 'uninspected' \
         RETURN r.status",
    ) {
        Ok(r) => {
            let v = r.rows().first().map(|row| s_val(&row[0]));
            println!(
                "PROBE shapes(merge+return): VERDICT — rows {}, status {:?} (expect 1 row 'passing': matched existing, ON CREATE skipped)",
                r.rows().len(),
                v
            );
        }
        Err(e) => println!("PROBE shapes(merge+return): VERDICT — FAILED: {e}"),
    }

    // MERGE + RETURN where the edge does NOT exist yet: ON CREATE SET must
    // fire and the RETURN must see the just-set values (create-path
    // read-your-writes inside one statement).
    match s.execute(
        "MATCH (b:T {id: 'b'}), (c:T {id: 'c'}) \
         MERGE (b)-[r:R]->(c) ON CREATE SET r.status = 'uninspected', r.note = 'fresh' \
         RETURN r.status, r.note",
    ) {
        Ok(r) => {
            let v = r
                .rows()
                .first()
                .map(|row| (s_val(&row[0]), s_val(&row[1])));
            println!(
                "PROBE shapes(merge-create+return): VERDICT — rows {}, values {:?} (expect 1 row ('uninspected','fresh'))",
                r.rows().len(),
                v
            );
        }
        Err(e) => println!("PROBE shapes(merge-create+return): VERDICT — FAILED: {e}"),
    }

    // Inequality on an edge property.
    let edge_neq = rows(
        &s,
        "MATCH (a:T)-[r:R]->(b:T) WHERE r.status <> 'independent' RETURN a.id, b.id",
    )
    .map(|r| r.len());
    println!("PROBE shapes(edge <>): VERDICT — {edge_neq:?} (expect Ok(1))");

    // Inequality on node properties of both endpoints (degree-query shape).
    let node_neq = rows(
        &s,
        "MATCH (a:T)-[r:R]->(b:T) WHERE a.status <> 'deprecated' AND b.status <> 'deprecated' RETURN a.id, b.id",
    )
    .map(|r| r.len());
    println!("PROBE shapes(node <> both ends): VERDICT — {node_neq:?} (expect Ok(1))");

    // Combined edge + node inequality (the full degree-query pushdown).
    let combined = rows(
        &s,
        "MATCH (a:T)-[r:R]->(b:T) WHERE r.status <> 'independent' AND a.status <> 'deprecated' AND b.status <> 'deprecated' RETURN a.id",
    )
    .map(|r| r.len());
    println!("PROBE shapes(combined pushdown): VERDICT — {combined:?} (expect Ok(1))");

    // CREATE INDEX: default kind, duplicate re-create, correctness after.
    match s.execute("CREATE INDEX probe_idx FOR (n:T) ON (n.id)") {
        Ok(_) => println!("PROBE shapes(index create): VERDICT — accepted"),
        Err(e) => println!("PROBE shapes(index create): VERDICT — FAILED: {e}"),
    }
    match s.execute("CREATE INDEX probe_idx FOR (n:T) ON (n.id)") {
        Ok(_) => println!("PROBE shapes(index dup): VERDICT — duplicate accepted (idempotent)"),
        Err(e) => println!("PROBE shapes(index dup): VERDICT — duplicate errors: {e}"),
    }
    let post_index = rows(&s, "MATCH (n:T {id: 'a'}) RETURN n.status").map(|r| r.len());
    let post_index_miss = rows(&s, "MATCH (n:T {id: 'zzz'}) RETURN n.status").map(|r| r.len());
    println!(
        "PROBE shapes(index correctness): VERDICT — hit {post_index:?} (expect Ok(1)), miss {post_index_miss:?} (expect Ok(0))"
    );

    // Write-after-index: do index-backed lookups see new rows?
    must(&s, "INSERT (:T {id: 'd', status: 'confirmed'})");
    let fresh = rows(&s, "MATCH (n:T {id: 'd'}) RETURN n.status").map(|r| r.len());
    println!("PROBE shapes(index freshness): VERDICT — {fresh:?} (expect Ok(1))");
}

// ---------------------------------------------------------------------------
// Probe 4c — native LIST property values (the schema-v5 question): write via
// $param and via literal, read back, filter with IN, expand with UNWIND,
// SET to a new list, and behavior of an empty list.
// ---------------------------------------------------------------------------

#[test]
fn probe_list_values() {
    let (_db, s) = mem();

    // Write a list property via $param.
    let mut p = HashMap::new();
    p.insert(
        "tags".to_string(),
        Value::List(vec![
            Value::String("authz".into()),
            Value::String("storage".into()),
        ].into()),
    );
    p.insert("id".to_string(), Value::String("i1".into()));
    match s.execute_with_params("INSERT (:T {id: $id, tags: $tags})", p) {
        Ok(_) => println!("PROBE list(param-write): VERDICT — list via $param accepted"),
        Err(e) => {
            println!("PROBE list(param-write): VERDICT — FAILED: {e}");
            return;
        }
    }
    // And via literal syntax.
    match s.execute("INSERT (:T {id: 'i2', tags: ['storage', 'sync']})") {
        Ok(_) => println!("PROBE list(literal-write): VERDICT — list literal accepted"),
        Err(e) => println!("PROBE list(literal-write): VERDICT — FAILED: {e}"),
    }
    // Empty list.
    match s.execute("INSERT (:T {id: 'i3', tags: []})") {
        Ok(_) => println!("PROBE list(empty-write): VERDICT — empty list accepted"),
        Err(e) => println!("PROBE list(empty-write): VERDICT — FAILED: {e}"),
    }

    // Read back: what does a list look like in QueryResult?
    match s.execute("MATCH (n:T {id: 'i1'}) RETURN n.tags") {
        Ok(r) => {
            let v = r.rows().first().map(|row| format!("{:?}", row[0]));
            println!("PROBE list(read): VERDICT — value shape: {v:?}");
        }
        Err(e) => println!("PROBE list(read): VERDICT — FAILED: {e}"),
    }
    // Empty-list read-back (does it survive as List([]) or become Null?).
    match s.execute("MATCH (n:T {id: 'i3'}) RETURN n.tags") {
        Ok(r) => {
            let v = r.rows().first().map(|row| format!("{:?}", row[0]));
            println!("PROBE list(empty-read): VERDICT — value shape: {v:?}");
        }
        Err(e) => println!("PROBE list(empty-read): VERDICT — FAILED: {e}"),
    }

    // Membership filter: IN over a stored list.
    match s.execute("MATCH (n:T) WHERE 'storage' IN n.tags RETURN n.id ORDER BY n.id") {
        Ok(r) => {
            let ids: Vec<String> = r.rows().iter().map(|row| s_val(&row[0])).collect();
            println!(
                "PROBE list(IN-filter): VERDICT — matched {ids:?} (expect [\"i1\", \"i2\"])"
            );
        }
        Err(e) => println!("PROBE list(IN-filter): VERDICT — FAILED: {e}"),
    }

    // UNWIND a stored list.
    match s.execute("MATCH (n:T {id: 'i2'}) UNWIND n.tags AS t RETURN t ORDER BY t") {
        Ok(r) => {
            let ts: Vec<String> = r.rows().iter().map(|row| s_val(&row[0])).collect();
            println!("PROBE list(UNWIND): VERDICT — {ts:?} (expect [\"storage\", \"sync\"])");
        }
        Err(e) => println!("PROBE list(UNWIND): VERDICT — FAILED: {e}"),
    }

    // SET to a different list via $param.
    let mut p2 = HashMap::new();
    p2.insert(
        "tags".to_string(),
        Value::List(vec![Value::String("cli".into())].into()),
    );
    match s.execute_with_params("MATCH (n:T {id: 'i1'}) SET n.tags = $tags", p2) {
        Ok(_) => {
            let after = s
                .execute("MATCH (n:T {id: 'i1'}) RETURN n.tags")
                .ok()
                .and_then(|r| r.rows().first().map(|row| format!("{:?}", row[0])));
            println!("PROBE list(SET): VERDICT — after SET: {after:?} (expect List [cli])");
        }
        Err(e) => println!("PROBE list(SET): VERDICT — FAILED: {e}"),
    }

    // size() on a stored list (useful for tag-cap checks in-query).
    match s.execute("MATCH (n:T {id: 'i2'}) RETURN size(n.tags)") {
        Ok(r) => {
            let v = r.rows().first().map(|row| s_val(&row[0]));
            println!("PROBE list(size): VERDICT — {v:?} (expect 2)");
        }
        Err(e) => println!("PROBE list(size): VERDICT — FAILED: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Probe 5 — CALL procedures and the join-back problem. Three questions:
//   (a) does id(n) work in plain MATCH (would enable a Rust-side join)?
//   (b) what do algorithm procedures return?
//   (c) does CALL … YIELD compose with a trailing MATCH (query-side join)?
// ---------------------------------------------------------------------------

#[test]
fn probe_algo_join_back() {
    let (_db, s) = mem();
    must(&s, "INSERT (:T {id: 'hub'}), (:T {id: 's1'}), (:T {id: 's2'}), (:T {id: 's3'})");
    for spoke in ["s1", "s2", "s3"] {
        must(
            &s,
            &format!("MATCH (a:T {{id: 'hub'}}), (b:T {{id: '{spoke}'}}) INSERT (a)-[:R]->(b)"),
        );
    }

    // (a) id() in plain MATCH — the key to joining CALL output in Rust.
    match s.execute("MATCH (n:T) RETURN id(n), n.id") {
        Ok(r) => {
            dump("id(n) map", &r);
            println!("PROBE call(id-fn): VERDICT — id(n) works; Rust-side join is possible");
        }
        Err(e) => println!("PROBE call(id-fn): VERDICT — id(n) FAILED: {e}"),
    }

    // (b) raw algorithm output.
    match s.execute("CALL grafeo.degree_centrality() YIELD node_id, total_degree RETURN node_id, total_degree") {
        Ok(r) => {
            dump("degree_centrality", &r);
            println!("PROBE call(degree): VERDICT — procedure runs; see ids above");
        }
        Err(e) => println!("PROBE call(degree): VERDICT — degree_centrality FAILED: {e}"),
    }
    match s.execute("CALL grafeo.pagerank() YIELD node_id, score RETURN node_id, score") {
        Ok(r) => dump("pagerank", &r),
        Err(e) => println!("  pagerank failed: {e}"),
    }

    // (c) query-side join: CALL … then MATCH on the yielded id. loom previously
    // observed the trailing MATCH parses but is silently dropped.
    match s.execute(
        "CALL grafeo.degree_centrality() YIELD node_id, total_degree \
         MATCH (n:T) WHERE id(n) = node_id RETURN n.id, total_degree",
    ) {
        Ok(r) => {
            dump("CALL+MATCH join", &r);
            let joined = r.columns.iter().any(|c| c == "n.id") && !r.rows().is_empty();
            println!(
                "PROBE call(query-join): VERDICT — {}",
                if joined {
                    "trailing MATCH actually joins: query-side join WORKS"
                } else {
                    "ran but did not join (columns/rows wrong) — the silently-dropped-MATCH behavior"
                }
            );
        }
        Err(e) => println!("PROBE call(query-join): VERDICT — errored (better than silent): {e}"),
    }
}

// ---------------------------------------------------------------------------
// Probe 6 — full-text index + search, and joining its node_ids back.
// ---------------------------------------------------------------------------

#[test]
fn probe_text_search_join_back() {
    let (_db, s) = mem();
    must(
        &s,
        "INSERT (:T {id: 'i1', description: 'priority scored work queue for agents'}), \
                (:T {id: 'i2', description: 'graph persistence layer'})",
    );

    match s.execute("CREATE INDEX probe_ft FOR (n:T) ON (n.description) USING TEXT") {
        Ok(_) => println!("PROBE text(index): VERDICT — FULLTEXT index creation accepted"),
        Err(e) => {
            println!("PROBE text(index): VERDICT — index creation FAILED: {e}");
            return;
        }
    }

    match s.execute("CALL grafeo.search.text('T', 'description', 'priority queue', 5)") {
        Ok(r) => {
            dump("search.text", &r);
            // Try the Rust-side join: map internal ids to the id property.
            if let Ok(map) = s.execute("MATCH (n:T) RETURN id(n), n.id") {
                dump("id map for join", &map);
                println!(
                    "PROBE text(join): VERDICT — if the node_id values above appear in the id map, \
                     Rust-side join works and `loom find` could use the native index"
                );
            }
        }
        Err(e) => println!("PROBE text(search): VERDICT — search.text FAILED: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Probe 6b — stress the historically-flaky patterns: many SETs on the same
// relationship (MVCC version churn), then filter by the edge's own properties,
// and match the edge by its own `id` property — the exact patterns loom's
// workarounds exist for.
// ---------------------------------------------------------------------------

#[test]
fn probe_edge_property_stress() {
    let (_db, s) = mem();
    must(&s, "INSERT (:T {id: 'a'}), (:T {id: 'b'}), (:T {id: 'c'})");
    must(
        &s,
        "MATCH (a:T {id: 'a'}), (b:T {id: 'b'}) INSERT (a)-[:R {id: 'edge-1', status: 'uninspected'}]->(b)",
    );
    must(
        &s,
        "MATCH (b:T {id: 'b'}), (c:T {id: 'c'}) INSERT (b)-[:R {id: 'edge-2', status: 'passing'}]->(c)",
    );

    // 50 write/read cycles on the same relationship: each SET creates a new
    // MVCC version; each read filters by the value just written.
    let mut wrong = 0;
    for i in 0..50 {
        let status = if i % 2 == 0 { "failing" } else { "passing" };
        must(
            &s,
            &format!("MATCH (a:T {{id: 'a'}})-[r:R]->(b:T {{id: 'b'}}) SET r.status = '{status}'"),
        );
        let got = rows(
            &s,
            &format!("MATCH (a:T)-[r:R]->(b:T) WHERE r.status = '{status}' AND a.id = 'a' RETURN a.id"),
        )
        .map(|r| r.len())
        .unwrap_or(usize::MAX);
        if got != 1 {
            wrong += 1;
        }
    }
    println!(
        "PROBE edge_stress(set-then-filter x50): VERDICT — {wrong}/50 reads wrong (0 = deterministic)"
    );

    // Match a relationship by its own id property — loom's docs call this
    // specifically unreliable.
    let mut by_id_wrong = 0;
    for _ in 0..30 {
        let got = rows(&s, "MATCH (a:T)-[r:R]->(b:T) WHERE r.id = 'edge-1' RETURN r.status")
            .map(|r| r.len())
            .unwrap_or(usize::MAX);
        if got != 1 {
            by_id_wrong += 1;
        }
    }
    let inline = rows(&s, "MATCH (a:T)-[r:R {id: 'edge-2'}]->(b:T) RETURN r.status")
        .map(|r| r.len());
    println!(
        "PROBE edge_stress(match-by-r.id x30): VERDICT — {by_id_wrong}/30 reads wrong; inline {{id}} form returned {inline:?} (expect Ok(1))"
    );

    // Diagnose: is `id` a reserved/shadowed property name on relationships?
    if let Ok(r) = s.execute("MATCH (a:T {id: 'a'})-[r:R]->(b:T {id: 'b'}) RETURN r.id, r.status") {
        dump("r.id read-back", &r);
    }
    // Same shape with a non-reserved name: does a `uid` property filter fine?
    must(
        &s,
        "MATCH (a:T {id: 'a'})-[r:R]->(b:T {id: 'b'}) SET r.uid = 'edge-1-uid'",
    );
    let by_uid = rows(&s, "MATCH (a:T)-[r:R]->(b:T) WHERE r.uid = 'edge-1-uid' RETURN r.status")
        .map(|r| r.len());
    println!(
        "PROBE edge_stress(match-by-r.uid): VERDICT — {by_uid:?} (Ok(1) proves only the NAME `id` is broken, not edge-property matching)"
    );

    // Read the same relationship's property straight after insert in a tight
    // loop of fresh edges (insert → immediate read, repeated).
    let mut fresh_wrong = 0;
    for i in 0..30 {
        must(&s, &format!("INSERT (:T {{id: 'n{i}'}})"));
        must(
            &s,
            &format!("MATCH (a:T {{id: 'a'}}), (b:T {{id: 'n{i}'}}) INSERT (a)-[:F {{tag: 't{i}'}}]->(b)"),
        );
        let got = rows(
            &s,
            &format!("MATCH (a:T {{id: 'a'}})-[r:F]->(b:T {{id: 'n{i}'}}) WHERE r.tag = 't{i}' RETURN r.tag"),
        )
        .map(|r| r.len())
        .unwrap_or(usize::MAX);
        if got != 1 {
            fresh_wrong += 1;
        }
    }
    println!(
        "PROBE edge_stress(insert-then-read x30): VERDICT — {fresh_wrong}/30 immediate reads wrong (0 = read-your-writes holds)"
    );
}

// ---------------------------------------------------------------------------
// Probe 6c — same stress shapes against a PERSISTENT db (loom's real mode):
// WAL + checkpoint paths differ from in-memory, so reliability must be shown
// here too before any conclusion about edge-property filtering.
// ---------------------------------------------------------------------------

#[test]
fn probe_edge_property_stress_persistent() {
    let dir = std::env::temp_dir().join(format!("grafeo-probe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let db = GrafeoDB::with_config(Config::persistent(dir.join("probe.grafeo")))
        .expect("open persistent db");
    let s = db.session();

    must(&s, "INSERT (:T {id: 'a'}), (:T {id: 'b'})");
    must(
        &s,
        "MATCH (a:T {id: 'a'}), (b:T {id: 'b'}) INSERT (a)-[:R {status: 'uninspected'}]->(b)",
    );

    let mut wrong = 0;
    for i in 0..50 {
        let status = if i % 2 == 0 { "failing" } else { "passing" };
        must(
            &s,
            &format!("MATCH (a:T {{id: 'a'}})-[r:R]->(b:T {{id: 'b'}}) SET r.status = '{status}'"),
        );
        let got = rows(
            &s,
            &format!("MATCH (a:T)-[r:R]->(b:T) WHERE r.status = '{status}' RETURN a.id"),
        )
        .map(|r| r.len())
        .unwrap_or(usize::MAX);
        if got != 1 {
            wrong += 1;
        }
    }
    println!(
        "PROBE edge_stress_persistent(set-then-filter x50): VERDICT — {wrong}/50 reads wrong (0 = deterministic on disk too)"
    );

    drop(s);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Probe 6d — IN-PROCESS concurrency: multiple Sessions on ONE GrafeoDB handle,
// readers running while a writer writes. (Cross-process is governed by the
// file lock — probe 7. This is the claim that matters for a future
// `loom serve` daemon: writers never block readers, snapshot isolation.)
// ---------------------------------------------------------------------------

#[test]
fn probe_in_process_concurrent_sessions() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let db = Arc::new(GrafeoDB::new_in_memory());
    let seed = db.session();
    must(&seed, "INSERT (:T {id: 'seed', n: 0})");
    drop(seed);

    let read_errors = Arc::new(AtomicUsize::new(0));
    let inconsistent = Arc::new(AtomicUsize::new(0));

    // One writer thread: 200 inserts. Four reader threads hammering reads.
    let writer = {
        let db = Arc::clone(&db);
        std::thread::spawn(move || {
            let s = db.session();
            for i in 0..200 {
                let _ = s.execute(&format!("INSERT (:T {{id: 'w{i}', n: {i}}})"));
            }
        })
    };
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let db = Arc::clone(&db);
            let errs = Arc::clone(&read_errors);
            let inc = Arc::clone(&inconsistent);
            std::thread::spawn(move || {
                let s = db.session();
                let mut last = 0usize;
                for _ in 0..200 {
                    match s.execute("MATCH (n:T) RETURN count(n) AS c") {
                        Ok(r) => {
                            let c = r
                                .rows()
                                .first()
                                .map(|row| match &row[0] {
                                    Value::Int64(n) => *n as usize,
                                    _ => 0,
                                })
                                .unwrap_or(0);
                            // Counts must be monotonically non-decreasing per
                            // reader (snapshots never go backwards) and ≥1.
                            if c < last || c == 0 {
                                inc.fetch_add(1, Ordering::Relaxed);
                            }
                            last = c;
                        }
                        Err(_) => {
                            errs.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            })
        })
        .collect();

    writer.join().expect("writer thread");
    for r in readers {
        r.join().expect("reader thread");
    }

    let final_count = {
        let s = db.session();
        rows(&s, "MATCH (n:T) RETURN count(n) AS c")
            .ok()
            .and_then(|r| {
                r.first().map(|row| match &row[0] {
                    Value::Int64(n) => *n,
                    _ => -1,
                })
            })
    };
    println!(
        "PROBE in_process_concurrency: VERDICT — read errors {}, backwards/zero snapshots {}, final count {:?} (expect 0, 0, Some(201))",
        read_errors.load(Ordering::Relaxed),
        inconsistent.load(Ordering::Relaxed),
        final_count
    );
}

// ---------------------------------------------------------------------------
// Probe 7 — concurrent access modes on a persistent DB: can a ReadOnly handle
// open while a ReadWrite handle holds the graph (the multi-agent question)?
// ---------------------------------------------------------------------------

#[test]
fn probe_read_only_concurrent() {
    let dir = std::env::temp_dir().join(format!("grafeo-probe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("probe.grafeo");

    {
        let writer =
            GrafeoDB::with_config(Config::persistent(&path)).expect("open ReadWrite handle");
        let ws = writer.session();
        must(&ws, "INSERT (:T {id: 'seed'})");

        // Second ReadWrite on the same path — expect the exclusive lock to refuse.
        let second_rw = GrafeoDB::with_config(Config::persistent(&path));
        println!(
            "PROBE lock(rw+rw): VERDICT — second ReadWrite open: {}",
            match &second_rw {
                Ok(_) => "Ok (NO exclusive lock?! two writers possible — dangerous)".to_string(),
                Err(e) => format!("refused as expected: {e}"),
            }
        );

        // ReadOnly while the writer is alive — the multi-agent unlock if Ok.
        match GrafeoDB::with_config(Config::read_only(&path)) {
            Ok(ro) => {
                let rs = ro.session();
                let seen = rows(&rs, "MATCH (n:T) RETURN n.id").map(|r| r.len());
                println!(
                    "PROBE lock(rw+ro): VERDICT — ReadOnly open alongside writer: Ok; read saw {seen:?} rows (writer may not have checkpointed yet)"
                );
            }
            Err(e) => println!(
                "PROBE lock(rw+ro): VERDICT — ReadOnly open alongside writer REFUSED: {e}"
            ),
        }
    }

    // Writer dropped — ReadOnly alone should definitely work.
    match GrafeoDB::with_config(Config::read_only(&path)) {
        Ok(ro) => {
            let rs = ro.session();
            let seen = rows(&rs, "MATCH (n:T) RETURN n.id").map(|r| r.len());
            println!(
                "PROBE lock(ro-alone): VERDICT — ReadOnly after writer closed: Ok; rows {seen:?} (expect Ok(1))"
            );
        }
        Err(e) => println!("PROBE lock(ro-alone): VERDICT — FAILED: {e}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
