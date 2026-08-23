//! Ring 22 — in-band delivery and packet identity.
//!
//! Two contracts:
//!
//! 1. **An MCP tool and its CLI twin cannot diverge.** Every tool is a wrapper
//!    over the same function the CLI calls; the tool result must equal what the
//!    library produces directly. A second implementation behind the same name is
//!    how loom's own `context` text and `--json` surfaces drifted apart.
//! 2. **Every served packet is journaled.** A packet id is minted where a packet
//!    leaves the process and recorded append-only, so "did loom's context change
//!    the outcome?" is answerable from the record rather than self-reported.

use loom::mcp::{InspectedMcpRequestKind, McpTranscriptEffect};
use loom::model::{EdgeKind, InspectionStatus, NodeType, TruthClass};
use loom::store::Store;
use serde_json::{json, Value};
mod common;
use common::*;

fn seeded(tmp: &Tmp) -> Store {
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/cart.rs"), "pub fn add_item() {}\n").unwrap();
    let store = Store::init(tmp.path(), Some("t"), false).unwrap();
    let intent = store
        .add_node(
            NodeType::Intent,
            "items can be added to a cart",
            "an item lands in the cart",
            "implemented",
            json!({}),
        )
        .unwrap();
    let cf = codefile(&store, "src/cart.rs");
    let e = store
        .add_edge(
            EdgeKind::Implements,
            &intent.id,
            &cf.id,
            TruthClass::Asserted,
        )
        .unwrap();
    store
        .record_verdict(
            &e.id,
            InspectionStatus::Passing,
            "add_item creates the line",
            "src/cart.rs:1",
            0.9,
            "llm",
        )
        .unwrap();
    store
}

fn call(root: &std::path::Path, name: &str, args: Value) -> Value {
    let response = loom::mcp::handle(
        Some(root),
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                "params":{"name":name,"arguments":args}}),
    )
    .expect("tools/call is a request, not a notification");
    assert_eq!(
        response["result"]["isError"], false,
        "tool {name} failed: {response}"
    );
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

fn journal_events(root: &std::path::Path, event: &str) -> Vec<Value> {
    loom::journal::read(root)
        .unwrap()
        .into_iter()
        .filter(|e| e.event == event)
        .map(|e| e.payload)
        .collect()
}

#[test]
fn journal_tool_makes_local_provenance_explicit_on_the_wire() {
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    store
        .append_journal(
            "batch_authorization",
            "bfixture",
            json!({"claim":"ratification", "operation":"ratify"}),
        )
        .unwrap();
    drop(store);

    let rows = call(
        tmp.path(),
        "loom_journal",
        json!({"event":"batch_authorization", "limit":1}),
    );
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["origin"], "local");
    assert_eq!(rows[0]["event"], "batch_authorization");
}

#[test]
fn tools_return_exactly_what_the_library_produces() {
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    drop(store);

    // loom_status vs the value behind `loom status --json`.
    let via_tool = call(tmp.path(), "loom_status", json!({}));
    let store = Store::open_read(tmp.path()).unwrap();
    let direct = serde_json::to_value(loom::maturity::ladder(&store).unwrap()).unwrap();
    assert_eq!(
        via_tool["maturity"], direct,
        "the MCP status tool and the ladder must be the same computation"
    );
    assert_eq!(via_tool["compass"]["phase"], direct["phase"]);
}

#[test]
fn served_packets_carry_ids_and_land_in_the_journal() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));

    let next = call(tmp.path(), "loom_next", json!({}));
    let work = &next["work_item"];
    assert!(!work.is_null(), "the seeded graph owes work");
    let next_id = work["packet_id"].as_str().unwrap().to_string();
    assert!(next_id.starts_with("pkt-"));

    let ctx = call(tmp.path(), "loom_context", json!({"target": "cart"}));
    let ctx_id = ctx["packet_id"].as_str().unwrap().to_string();
    assert_ne!(next_id, ctx_id, "each serving is distinct");

    // Both servings are on the append-only record, with the kind and the entity
    // they were about — that is what makes efficacy attributable later.
    let served: Vec<Value> = journal_events(tmp.path(), "packet_served")
        .into_iter()
        .flat_map(|p| p["packets"].as_array().cloned().unwrap_or_default())
        .collect();
    let ids: Vec<&str> = served.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&next_id.as_str()), "next packet journaled");
    assert!(ids.contains(&ctx_id.as_str()), "context packet journaled");
    let ctx_entry = served
        .iter()
        .find(|p| p["id"] == ctx_id.as_str())
        .expect("context entry");
    assert_eq!(ctx_entry["kind"], "context");
    assert!(!ctx_entry["target"].as_str().unwrap().is_empty());
}

#[test]
fn assembling_a_packet_is_not_serving_it() {
    // The efficacy denominator must count real servings only. Reading the
    // ladder or the queue roster assembles nothing that reaches a consumer, so
    // it must not appear on the record.
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    let before = journal_events(tmp.path(), "packet_served").len();
    loom::maturity::ladder(&store).unwrap();
    loom::workitem::queue_items(&store, loom::lane::Lane::Validate).unwrap();
    loom::workitem::next(&store, None).unwrap();
    assert_eq!(
        journal_events(tmp.path(), "packet_served").len(),
        before,
        "library-level assembly must not inflate the served-packet record"
    );
}

#[test]
fn a_failing_tool_reports_why_instead_of_a_transport_error() {
    // The model should see loom's refusal and adapt; a protocol-level error
    // would hide the reason.
    let tmp = Tmp::new();
    drop(seeded(&tmp));
    let response = loom::mcp::handle(
        Some(tmp.path()),
        &json!({"jsonrpc":"2.0","id":9,"method":"tools/call",
                "params":{"name":"loom_context","arguments":{"target":"nothing matches this"}}}),
    )
    .unwrap();
    assert!(response.get("error").is_none(), "not a transport error");
    assert_eq!(response["result"]["isError"], true);
    let body: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("could not resolve"),
        "the refusal names what failed: {body}"
    );
}

/// Every capability is reachable in-band. A partner interrupts; a CLI waits —
/// and a capability that exists only as a subprocess is one an agent has to
/// stop and shell out for, which is the thing MCP is here to avoid.
#[test]
fn nothing_ships_cli_only() {
    let response = loom::mcp::handle(
        None,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
    )
    .expect("tools/list is a request");
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("a tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "loom_status",
        "loom_next",
        "loom_context",
        "loom_impact",
        "loom_observe",
        "loom_absorb",
        "loom_journal",
        "loom_apply",
    ] {
        assert!(
            names.contains(&expected),
            "{expected} is not served: {names:?}"
        );
    }
    // And every tool declares a schema an agent can call without guessing.
    for tool in response["result"]["tools"].as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(
            tool["description"].as_str().map(|d| d.len()).unwrap_or(0) > 40,
            "a tool nobody understands is a tool nobody calls: {tool}"
        );
    }
}

/// `loom_observe` runs the command and reports what happened. The outcome is
/// never taken from the caller — there is no argument with which to supply one.
#[test]
fn observing_in_band_records_what_loom_saw() {
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    let intent = store
        .list_nodes(Some(loom::model::NodeType::Intent), usize::MAX)
        .unwrap()
        .remove(0);
    drop(store);

    let v = call(
        tmp.path(),
        "loom_observe",
        json!({ "command": ["true"], "for_behavior": intent.id }),
    );
    assert_eq!(v["observed"], true, "{v}");
    assert_eq!(v["exit_code"], 0, "{v}");
    assert_eq!(
        v["strength"], "S1",
        "loom ran it and it passed — liveness, not behavior: {v}"
    );

    // The schema has no field for an outcome, which is the structural half of
    // the same guarantee.
    let listed = loom::mcp::handle(
        None,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
    )
    .unwrap();
    let observe_tool = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "loom_observe")
        .unwrap();
    let props = &observe_tool["inputSchema"]["properties"];
    for forbidden in ["outcome", "result", "passed", "exit_code"] {
        assert!(
            props.get(forbidden).is_none(),
            "a caller must not be able to report an outcome: {observe_tool}"
        );
    }
}

/// `loom_absorb` observes and writes nothing.
#[test]
fn absorbing_in_band_is_a_pure_read() {
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    let before = loom::travel::export_to_file(&store).unwrap();
    let before = std::fs::read_to_string(&before).unwrap();
    drop(store);

    let v = call(tmp.path(), "loom_absorb", json!({}));
    assert!(v["items"].is_array(), "{v}");

    let store = Store::open(tmp.path()).unwrap();
    let after = loom::travel::export_to_file(&store).unwrap();
    assert_eq!(
        before,
        std::fs::read_to_string(&after).unwrap(),
        "absorb must not mutate the graph"
    );
}

/// An in-band batch goes through the same gates as one from disk — a batch
/// cannot accept what a single write would refuse.
#[test]
fn an_in_band_batch_is_gated_like_any_other_write() {
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    drop(store);

    // A verdict with placeholder evidence is refused, exactly as `loom apply`
    // would refuse it from a file.
    let response = loom::mcp::handle(
        Some(tmp.path()),
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"loom_apply",
            "arguments":{"fragment":{"intents":[{"name":"", "description":"x"}]}}
        }}),
    )
    .expect("a request");
    assert_eq!(
        response["result"]["isError"], true,
        "an invalid batch must fail as a tool result, not silently apply: {response}"
    );
}

/// The published `loom_apply` inputSchema must describe the real batch shape —
/// not an opaque object a caller can only learn by failing. It must carry the
/// eight sections, refuse unknown fields, and be self-contained: schemars
/// emits `#/$defs/X` pointers, which resolve from the ROOT of the schema
/// document the client was handed (`inputSchema`), so the test walks every
/// `$ref` and resolves it from there — a `$defs` parked anywhere else is a
/// dangling reference and fails here.
#[test]
fn loom_apply_input_schema_publishes_the_batch_shape() {
    let response = loom::mcp::handle(
        None,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
    )
    .expect("tools/list is a request");
    let tool = response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "loom_apply")
        .expect("loom_apply is served");
    let input_schema = &tool["inputSchema"];
    let fragment = &input_schema["properties"]["fragment"];

    for section in [
        "intents",
        "groundings",
        "relationships",
        "verdicts",
        "rule_verdicts",
        "adjudications",
        "vocab",
        "tags",
    ] {
        assert!(
            fragment["properties"].get(section).is_some(),
            "the schema must name section `{section}`: {fragment}"
        );
    }
    assert_eq!(
        fragment["additionalProperties"], false,
        "the parser denies unknown fields; the schema must say so: {fragment}"
    );

    // Every $ref must resolve against the inputSchema ROOT. A pointer that
    // cannot be walked is a schema a client cannot follow.
    fn collect_refs(value: &Value, refs: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(r) = map.get("$ref").and_then(Value::as_str) {
                    refs.push(r.to_string());
                }
                for v in map.values() {
                    collect_refs(v, refs);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| collect_refs(v, refs)),
            _ => {}
        }
    }
    let mut refs = Vec::new();
    collect_refs(input_schema, &mut refs);
    assert!(
        !refs.is_empty(),
        "schemars emits $ref pointers; finding none means the derivation changed \
         shape and this walk proves nothing — assert the inlined bodies instead"
    );
    for r in &refs {
        assert!(
            r.starts_with("#/"),
            "only document-local pointers are resolvable by a tools/list client: {r}"
        );
        let mut cur = input_schema;
        for seg in r[2..].split('/') {
            let seg = seg.replace("~1", "/").replace("~0", "~");
            cur = cur
                .get(&seg)
                .unwrap_or_else(|| panic!("$ref `{r}` dangles at segment `{seg}`"));
        }
    }

    // The item schemas must be concrete. `items` is a `$ref`, so resolve it
    // through the same root walk the client would do, then read the body.
    let intent_item_ref = fragment["properties"]["intents"]["items"]["$ref"]
        .as_str()
        .expect("intents.items is a $ref to the shared spec");
    let intent_item = input_schema
        .pointer(intent_item_ref.trim_start_matches('#'))
        .expect("the walk above proved every $ref resolves");
    let required: Vec<&str> = intent_item["required"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap()).collect())
        .unwrap_or_default();
    assert_eq!(
        required,
        vec!["name"],
        "an intent needs only a name: {intent_item}"
    );
}

/// A fragment accepted by `handle` validates against the published schema's
/// declared sections and the result reports created ids — the wire contract
/// an agent reads from tools/list is the one the call actually honors.
#[test]
fn loom_apply_result_reports_created_intent_ids() {
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    drop(store);

    let v = call(
        tmp.path(),
        "loom_apply",
        json!({"fragment":{
            "intents":[{"name":"payment can be captured","description":"capturing settles the charge"}],
            "vocab":[{"term":"payments","why":"money flows"}],
            "tags":[{"intent":"payment can be captured","terms":["payments"]}]
        }}),
    );
    assert_eq!(v["intents_added"], 1, "{v}");
    let id = v["intent_ids"]["payment can be captured"]
        .as_str()
        .expect("created intents report their ids");
    let store = Store::open(tmp.path()).unwrap();
    let node = store
        .resolve_node("payment can be captured", Some(NodeType::Intent))
        .unwrap();
    assert_eq!(node.id, id, "the reported id resolves the created node");
}

// ---------------------------------------------------------------------------
// The transport loop itself.
//
// `handle` answers one request; `serve` is the loop around it, and the loop has
// its own contract: a session survives bad input. Driving it over in-memory
// buffers proves the real code path end to end — the same function
// `loom mcp serve` runs, not a re-implementation of it.
// ---------------------------------------------------------------------------

/// Drive the real serve loop over `input`, returning one parsed response per
/// line it wrote.
fn serve_lines(root: &std::path::Path, input: &str) -> Vec<Value> {
    let mut out: Vec<u8> = Vec::new();
    loom::mcp::serve(Some(root), std::io::Cursor::new(input.as_bytes()), &mut out)
        .expect("the loop ends cleanly at EOF");
    String::from_utf8(out)
        .expect("responses are UTF-8")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line is one JSON-RPC response"))
        .collect()
}

/// The loop answers each request in order and stops at EOF.
#[test]
fn the_serve_loop_answers_every_request_and_ends_at_eof() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));

    let responses = serve_lines(
        tmp.path(),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n\
         {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n",
    );

    assert_eq!(
        responses.len(),
        2,
        "one response per request: {responses:?}"
    );
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[1]["id"], 2);
    assert!(
        responses[1]["result"]["tools"]
            .as_array()
            .expect("tools/list returns an array")
            .iter()
            .any(|t| t["name"] == "loom_status"),
        "the listed tools are the real ones: {:?}",
        responses[1]
    );
}

/// A blank line is not a request. Skipping it must not cost a response.
#[test]
fn blank_lines_are_skipped_rather_than_answered() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));

    let responses = serve_lines(
        tmp.path(),
        "\n\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"params\":{}}\n\n",
    );
    assert_eq!(
        responses.len(),
        1,
        "only the request is answered: {responses:?}"
    );
    assert_eq!(responses[0]["id"], 1);
}

/// Unparseable input is a -32700 with a null id — the id is unknowable — and
/// the session keeps going. A loop that died here would fail every later
/// request for the wrong reason.
#[test]
fn a_parse_error_is_answered_and_the_session_survives() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));

    let responses = serve_lines(
        tmp.path(),
        "not json at all\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\",\"params\":{}}\n",
    );

    assert_eq!(
        responses.len(),
        2,
        "both lines produced a response: {responses:?}"
    );
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert!(
        responses[0]["id"].is_null(),
        "an unparseable line has no knowable id"
    );
    assert_eq!(
        responses[1]["id"], 7,
        "the request after the bad line is still served: {responses:?}"
    );
}

/// A notification has no id and must not be answered at all.
#[test]
fn a_notification_draws_no_response() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));

    let responses = serve_lines(
        tmp.path(),
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n\
         {\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\",\"params\":{}}\n",
    );

    assert_eq!(
        responses.len(),
        1,
        "the notification is silent; only the ping is answered: {responses:?}"
    );
    assert_eq!(responses[0]["id"], 9);
}

/// Non-UTF-8 bytes must not abort the server: they decode lossily and come
/// back as a parse error like any other malformed line.
#[test]
fn invalid_utf8_is_a_parse_error_not_a_dead_session() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));

    let mut input: Vec<u8> = vec![0xff, 0xfe, b'\n'];
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\",\"params\":{}}\n");
    let mut out: Vec<u8> = Vec::new();
    loom::mcp::serve(Some(tmp.path()), std::io::Cursor::new(input), &mut out)
        .expect("bad bytes do not abort the loop");

    let responses: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    assert_eq!(responses.len(), 2, "{responses:?}");
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert_eq!(responses[1]["id"], 3, "the session survived the bad bytes");
}

/// A tool that exists but cannot answer is `isError` on a live transport —
/// distinct from -32602, which means the tool was never there.
#[test]
fn a_failing_tool_and_an_unknown_tool_are_different_conditions() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));

    let responses = serve_lines(
        tmp.path(),
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"loom_nope\",\"arguments\":{}}}\n\
         {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"loom_context\",\"arguments\":{\"target\":\"nothing in this graph matches\"}}}\n\
         {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\",\"params\":{}}\n",
    );

    assert_eq!(responses.len(), 3, "{responses:?}");
    assert_eq!(
        responses[0]["error"]["code"], -32602,
        "an unknown tool is a bad request, not a tool that ran: {:?}",
        responses[0]
    );
    assert_eq!(
        responses[1]["result"]["isError"], true,
        "a tool that ran and could not answer reports isError: {:?}",
        responses[1]
    );
    assert_eq!(
        responses[2]["id"], 3,
        "the session remains usable after a failing known tool: {responses:?}"
    );
    assert_eq!(responses[2]["result"], json!({}));
}

#[test]
fn transcript_adapter_drives_one_live_session_and_preserves_every_response() {
    let tmp = Tmp::new();
    let store = seeded(&tmp);
    std::fs::write(
        tmp.path().join("src/cart.rs"),
        "pub fn add_item() {}\npub fn checkout() { add_item(); }\n",
    )
    .unwrap();
    loom::sync::run(&store, tmp.path()).unwrap();
    drop(store);

    let requests = json!([
        {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}},
        {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}},
        {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"loom_impact","arguments":{"target":"add_item"}
        }},
        {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
            "name":"loom_nope","arguments":{}
        }},
        {"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
            "name":"loom_context","arguments":{"target":"nothing in this graph matches"}
        }},
        {"jsonrpc":"2.0","id":6,"method":"ping","params":{}}
    ]);

    let report = loom::mcp::transcript(Some(tmp.path()), &requests.to_string()).unwrap();
    assert_eq!(report["protocol"], "json-rpc-2.0");
    assert_eq!(report["request_count"], 6);
    assert_eq!(report["response_count"], 6);
    assert_eq!(report["session_completed"], true);
    let responses = report["responses"].as_array().unwrap();
    assert_eq!(
        responses
            .iter()
            .map(|response| response["id"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6],
        "responses retain wire order: {responses:?}"
    );

    assert_eq!(responses[0]["result"]["serverInfo"]["name"], "loom");
    let tools = responses[1]["result"]["tools"].as_array().unwrap();
    assert!(tools
        .iter()
        .any(|tool| { tool["name"] == "loom_impact" && tool["inputSchema"]["type"] == "object" }));

    assert_eq!(responses[2]["result"]["isError"], false);
    let impact: Value = serde_json::from_str(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(impact["target"], "add_item");
    assert!(
        impact["callers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|caller| caller["symbol"] == "checkout"),
        "live impact contains the extracted caller: {impact}"
    );
    assert!(impact["resolution"]["exact"].as_u64().unwrap() >= 1);

    assert_eq!(responses[3]["error"]["code"], -32602);
    assert!(responses[3].get("result").is_none());
    assert_eq!(responses[4]["result"]["isError"], true);
    assert!(responses[4].get("error").is_none());
    assert_eq!(responses[5]["result"], json!({}));
}

#[test]
fn transcript_adapter_rejects_invalid_or_malformed_request_arrays() {
    for (input, expected) in [
        ("not-json", "valid JSON"),
        ("{}", "JSON array"),
        ("[]", "at least one request"),
        (r#"[1]"#, "request 1 must be a JSON object"),
        (
            r#"[{"jsonrpc":"1.0","id":1,"method":"ping"}]"#,
            "jsonrpc \"2.0\"",
        ),
        (r#"[{"jsonrpc":"2.0","id":1}]"#, "non-empty string method"),
        (r#"[{"jsonrpc":"2.0","method":"ping"}]"#, "must have an id"),
        (
            r#"[{"jsonrpc":"2.0","id":{},"method":"ping"}]"#,
            "id must be a string or number",
        ),
        (
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping","params":true}]"#,
            "ping params must be an empty object",
        ),
    ] {
        let error = loom::mcp::transcript(None, input).unwrap_err().to_string();
        assert!(error.contains(expected), "{input}: {error}");
    }
}

#[test]
fn every_reviewed_mcp_surface_transcript_passes_strict_nonexecuting_inspection() {
    let manifests = [
        include_str!("../journeys/surfaces/mcp-in-band.surface.json"),
        include_str!("../journeys/surfaces/plain-next-not-ratify.surface.json"),
        include_str!("../journeys/surfaces/ratify-guard.surface.json"),
        include_str!("../journeys/surfaces/self-audit.surface.json"),
        include_str!("../journeys/surfaces/system-purpose.surface.json"),
    ];
    let mut transcripts = 0;
    let mut requests = 0;
    let mut initialize = 0;
    let mut tools_list = 0;
    let mut ping = 0;
    let mut unknown_probe = 0;
    let mut reads = 0;
    let mut observes = 0;
    let mut applies = 0;

    for source in manifests {
        let manifest: Value = serde_json::from_str(source).unwrap();
        for operation in manifest["surface"]["operations"].as_array().unwrap() {
            let argv = operation["argv"].as_array().unwrap();
            let Some(index) = argv
                .iter()
                .position(|token| token.as_str() == Some("--requests-json"))
            else {
                continue;
            };
            let requests_json = argv[index + 1].as_str().unwrap();
            let inspected = loom::mcp::inspect_transcript_requests(requests_json)
                .unwrap_or_else(|error| panic!("{}: {error:#}", operation["id"].as_str().unwrap()));
            transcripts += 1;
            requests += inspected.len();
            for (expected_index, request) in inspected.into_iter().enumerate() {
                assert_eq!(request.index, expected_index);
                match request.kind {
                    InspectedMcpRequestKind::Initialize => initialize += 1,
                    InspectedMcpRequestKind::ToolsList => tools_list += 1,
                    InspectedMcpRequestKind::Ping => ping += 1,
                    InspectedMcpRequestKind::UnknownTool { name } => {
                        assert_eq!(name, "loom_nope");
                        unknown_probe += 1;
                    }
                    InspectedMcpRequestKind::ToolCall {
                        effect,
                        nested_argv,
                        ..
                    } => match effect {
                        McpTranscriptEffect::Read => {
                            assert!(nested_argv.is_none());
                            reads += 1;
                        }
                        McpTranscriptEffect::ObserveArgv => {
                            assert_eq!(
                                nested_argv
                                    .as_ref()
                                    .and_then(|argv| argv.first())
                                    .map(String::as_str),
                                Some("loom")
                            );
                            observes += 1;
                        }
                        McpTranscriptEffect::ApplyFragment => {
                            assert!(nested_argv.is_none());
                            applies += 1;
                        }
                    },
                }
            }
        }
    }

    assert_eq!(transcripts, 10);
    assert_eq!(requests, 48);
    assert_eq!(initialize, 10);
    assert_eq!(tools_list, 5);
    assert_eq!(ping, 2);
    assert_eq!(unknown_probe, 2);
    assert_eq!(reads, 7);
    assert_eq!(observes, 11);
    assert_eq!(applies, 11);
}

fn inspect_one(request: Value) -> anyhow::Result<Vec<loom::mcp::InspectedMcpRequest>> {
    loom::mcp::inspect_transcript_requests(&json!([request]).to_string())
}

fn tool_request(name: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments}
    })
}

#[test]
fn transcript_inspector_rejects_envelope_identity_and_tool_argument_escapes() {
    let duplicate = json!([
        {"jsonrpc":"2.0","id":1,"method":"ping","params":{}},
        {"jsonrpc":"2.0","id":1,"method":"ping","params":{}}
    ]);
    assert!(
        loom::mcp::inspect_transcript_requests(&duplicate.to_string())
            .unwrap_err()
            .to_string()
            .contains("repeats an earlier id")
    );

    for (request, expected) in [
        (
            json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{},"extra":true}),
            "unknown field",
        ),
        (
            json!({"jsonrpc":"2.0","id":1,"method":"notifications/initialized","params":{}}),
            "not allowed",
        ),
        (
            json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{"root":"/tmp"}}),
            "empty object",
        ),
        (tool_request("loom_impact", json!({})), "target"),
        (
            tool_request("loom_impact", json!({"target":"serve","root":"/tmp"})),
            "unknown field",
        ),
        (
            tool_request("loom_impact", json!({"target":"serve","depth":0})),
            "minimum",
        ),
        (
            tool_request("loom_impact", json!({"target":"serve","depth":11})),
            "maximum",
        ),
        (
            tool_request("loom_next", json!({"lane":"build"})),
            "empty or ratify-lane",
        ),
        (
            tool_request("loom_nope", json!({"command":["loom","status"]})),
            "empty object",
        ),
        (
            tool_request("arbitrary_tool", json!({})),
            "not an allowed probe",
        ),
    ] {
        let error = inspect_one(request).unwrap_err().to_string();
        assert!(error.contains(expected), "expected {expected:?}: {error}");
    }
}

#[test]
fn transcript_inspector_returns_only_strict_bare_loom_observe_argv() {
    let request = tool_request(
        "loom_observe",
        json!({"command":["loom","intent","ratify","fixture","--json"]}),
    );
    let inspected = inspect_one(request).unwrap();
    match &inspected[0].kind {
        InspectedMcpRequestKind::ToolCall {
            effect: McpTranscriptEffect::ObserveArgv,
            nested_argv: Some(argv),
            ..
        } => assert_eq!(argv, &["loom", "intent", "ratify", "fixture", "--json"]),
        other => panic!("unexpected inspection: {other:?}"),
    }

    for arguments in [
        json!({"command":[]}),
        json!({"command":["sh","-c","git push"]}),
        json!({"command":["loom",1]}),
        json!({"command":["loom","status"],"timeout":1}),
        json!({"command":["loom","status"],"for_behavior":"fixture"}),
    ] {
        assert!(inspect_one(tool_request("loom_observe", arguments)).is_err());
    }
}

#[test]
fn transcript_inspector_allows_only_needed_adjudication_apply_fragments() {
    let valid = json!({"fragment":{"adjudications":[{
        "finding":"finding-id",
        "verdict":"needed",
        "reason":"Reviewed only in the detached proof graph."
    }]}});
    let inspected = inspect_one(tool_request("loom_apply", valid)).unwrap();
    assert!(matches!(
        inspected[0].kind,
        InspectedMcpRequestKind::ToolCall {
            effect: McpTranscriptEffect::ApplyFragment,
            ..
        }
    ));

    for fragment in [
        json!({"intents":[]}),
        json!({"adjudications":[]}),
        json!({"adjudications":[{"finding":"f","verdict":"waived","reason":"x"}]}),
        json!({"adjudications":[{"finding":"f","verdict":"needed","reason":"x","authority":"human"}]}),
        json!({"adjudications":[{"finding":"","verdict":"needed","reason":"x"}]}),
    ] {
        assert!(inspect_one(tool_request("loom_apply", json!({"fragment": fragment}))).is_err());
    }
}

#[test]
fn transcript_cli_emits_one_json_document() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));
    let requests = json!([
        {"jsonrpc":"2.0","id":"init","method":"initialize","params":{}},
        {"jsonrpc":"2.0","id":"ping","method":"ping","params":{}}
    ]);
    let output = loom_command()
        .arg("--graph")
        .arg(tmp.path())
        .arg("mcp")
        .arg("transcript")
        .arg("--requests-json")
        .arg(requests.to_string())
        .arg("--json")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "transcript failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["request_count"], 2);
    assert_eq!(report["response_count"], 2);
    assert_eq!(report["responses"][0]["id"], "init");
    assert_eq!(report["responses"][1]["id"], "ping");
    assert_eq!(report["responses"][1]["result"], json!({}));
}

#[test]
fn initialize_refuses_an_unsupported_client_protocol_version() {
    let accepted = loom::mcp::handle(
        None,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        }),
    )
    .unwrap();
    assert_eq!(accepted["result"]["protocolVersion"], "2024-11-05");
    assert!(accepted.get("error").is_none());

    let omitted = loom::mcp::handle(
        None,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {}}),
    )
    .unwrap();
    assert_eq!(omitted["result"]["protocolVersion"], "2024-11-05");

    let rejected = loom::mcp::handle(
        None,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "initialize",
            "params": { "protocolVersion": "1999-01-01" }
        }),
    )
    .unwrap();
    assert_eq!(rejected["error"]["code"], -32602);
    let message = rejected["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("1999-01-01") && message.contains("2024-11-05"),
        "mismatch must name both versions: {message}"
    );
    assert!(rejected.get("result").is_none());
}

// --- Argument-contract parity -------------------------------------------
//
// Contract 1 says a tool and its CLI twin cannot diverge. That was proved for
// the ANSWER (same function, same numbers) but not for the QUESTION: the
// default depth and timeout were typed once into the clap declaration and
// again into the hand-written JSON Schema, including in prose the schema shows
// a driver. Two hand-maintained copies of one contract drift silently, so the
// values now come from one constant and these tests hold the surfaces to it.

/// The default a clap argument declares, as the string clap would apply.
fn clap_default(subcommand: &str, arg: &str) -> String {
    use clap::CommandFactory;
    let command = loom::cli::Cli::command();
    let sub = command
        .find_subcommand(subcommand)
        .unwrap_or_else(|| panic!("`loom {subcommand}` exists"));
    let argument = sub
        .get_arguments()
        .find(|a| a.get_id() == arg)
        .unwrap_or_else(|| panic!("`loom {subcommand}` declares `{arg}`"));
    let values = argument.get_default_values();
    assert_eq!(
        values.len(),
        1,
        "`loom {subcommand} --{arg}` must declare exactly one default"
    );
    values[0].to_string_lossy().to_string()
}

/// The published inputSchema for one tool, as a driver receives it.
fn tool_schema(name: &str) -> Value {
    let listed = loom::mcp::handle(
        None,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
    )
    .expect("tools/list is a request");
    listed["result"]["tools"]
        .as_array()
        .expect("tools is an array")
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("{name} is published"))["inputSchema"]
        .clone()
}

#[test]
fn impact_depth_is_one_contract_on_both_surfaces() {
    let schema = tool_schema("loom_impact");
    let depth = &schema["properties"]["depth"];

    assert_eq!(
        clap_default("impact", "depth"),
        loom::callgraph::DEFAULT_IMPACT_DEPTH.to_string(),
        "`loom impact --depth` must default to the shared constant"
    );
    assert_eq!(
        depth["maximum"].as_u64().expect("depth declares a maximum"),
        loom::callgraph::MAX_IMPACT_DEPTH as u64,
        "the tool schema's ceiling must be the enforced limit"
    );
    assert_eq!(
        depth["minimum"].as_u64().expect("depth declares a minimum"),
        1,
        "a zero-hop impact walk answers nothing"
    );

    // The prose a driver reads is part of the contract: a description still
    // saying "default 3" after the constant moved is the same divergence,
    // just harder to catch.
    let described = depth["description"].as_str().expect("depth is described");
    assert!(
        described.contains(&format!(
            "default {}",
            loom::callgraph::DEFAULT_IMPACT_DEPTH
        )),
        "the tool description must state the real default: {described}"
    );
}

#[test]
fn observe_timeout_is_one_contract_on_both_surfaces() {
    let schema = tool_schema("loom_observe");
    let timeout = &schema["properties"]["timeout"];

    assert_eq!(
        clap_default("observe", "timeout"),
        loom::runner::DEFAULT_OBSERVE_TIMEOUT_SECS.to_string(),
        "`loom observe --timeout` must default to the shared constant"
    );
    let described = timeout["description"]
        .as_str()
        .expect("timeout is described");
    assert!(
        described.contains(&format!(
            "default {}",
            loom::runner::DEFAULT_OBSERVE_TIMEOUT_SECS
        )),
        "the tool description must state the real default: {described}"
    );
}

/// The depth ceiling is enforced inside the function both surfaces call, not
/// in either argument parser — so the tool cannot accept a walk the CLI would
/// refuse. Proved through the tool path, which has no parser bound of its own.
#[test]
fn the_impact_ceiling_is_enforced_behind_both_surfaces() {
    let tmp = Tmp::new();
    drop(seeded(&tmp));
    let over = loom::callgraph::MAX_IMPACT_DEPTH as u64 + 1;

    let response = loom::mcp::handle(
        Some(tmp.path()),
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "loom_impact",
                "arguments": { "target": "add_item", "depth": over }
            }
        }),
    )
    .expect("tools/call is a request");

    let rendered = serde_json::to_string(&response).unwrap();
    assert!(
        rendered.contains("max_impact_depth"),
        "an over-deep walk must name the limit that refused it: {rendered}"
    );
}

/// Every enforced limit must be listed by `loom limits`; a bound a caller can
/// hit but not discover is the drift this registry exists to prevent.
#[test]
fn the_new_bounds_are_discoverable_through_loom_limits() {
    let named: Vec<&str> = loom::limits::all().iter().map(|l| l.name).collect();
    for expected in ["max_impact_depth", "observe_timeout_secs"] {
        assert!(
            named.contains(&expected),
            "`loom limits` must list {expected}: {named:?}"
        );
    }
}
