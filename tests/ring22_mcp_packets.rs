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
    let cf = store
        .add_node(NodeType::CodeFile, "src/cart.rs", "", "", json!({}))
        .unwrap();
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
