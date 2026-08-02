//! MCP server — loom in-band, so an LLM pulls context instead of shelling out.
//!
//! Plane: transport. Every tool here is a thin wrapper over the SAME function
//! the CLI calls; this module owns no truth and makes no decisions. If a tool
//! and its CLI twin can ever disagree, the wrapper is wrong.
//!
//! Contract: newline-delimited JSON-RPC 2.0 over stdio (the MCP stdio
//! transport). Hand-rolled rather than pulled from a crate — the surface is
//! `initialize` / `tools/list` / `tools/call` / `ping`, it is stable, and loom
//! keeps its dependency list short enough to audit.
//!
//! Why in-band matters: shelling out makes every context pull a decision the
//! model has to remember to take. A partner interrupts; a CLI waits.

use crate::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// The MCP revision this server speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// One callable tool: its schema and the function behind it.
struct Tool {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    call: fn(Option<&Path>, &Value) -> Result<Value>,
}

fn no_args() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "loom_status",
            description:
                "Where this graph stands: the maturity ladder, the compass (the single next \
                 move), per-lane queue depths, and code ownership. Call this first in a session \
                 and after any change, the way you would read a build status.",
            schema: no_args,
            call: |graph, _args| {
                let store = crate::commands::open_read(graph)?;
                crate::commands::status_value(&store)
            },
        },
        Tool {
            name: "loom_next",
            description:
                "The next correct unit of work, compiled into a prompt contract: the role to \
                 adopt, what is allowed and forbidden, the evidence required, the exact \
                 write-back, and the files to read first. Omit `lane` to take whatever the \
                 compass points at. Every packet carries a packet_id.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "lane": {
                            "type": "string",
                            "description": "Optional lane to serve from (see loom_status.queues).",
                            "enum": crate::lane::Lane::LADDER
                                .iter()
                                .filter(|l| l.serves_items())
                                .map(|l| l.as_str())
                                .collect::<Vec<_>>(),
                        }
                    },
                    "additionalProperties": false
                })
            },
            call: |graph, args| {
                let store = crate::commands::open_read(graph)?;
                let lane = args.get("lane").and_then(|v| v.as_str());
                Ok(serde_json::to_value(crate::commands::next_output(
                    &store, lane,
                )?)?)
            },
        },
        Tool {
            name: "loom_context",
            description:
                "Read-only context for an intent, a registered file path, or a free-text query: \
                 what the behavior is, where it is grounded, what proves it, which rules govern \
                 it, and what has gone stale. Pull this BEFORE editing code you did not write.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "description": "Intent id/name/prefix, registered codefile path, or free text."
                        }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                })
            },
            call: |graph, args| {
                let target = args
                    .get("target")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("`target` is required"))?;
                let store = crate::commands::open_read(graph)?;
                Ok(serde_json::to_value(crate::commands::served_context(
                    &store, target,
                )?)?)
            },
        },
        Tool {
            name: "loom_impact",
            description:
                "What a change here could reach: the symbols that transitively call a target,                  nearest first, from the real call graph. Exact and heuristic resolutions are                  reported separately and never blended. Call this BEFORE editing a symbol you                  did not write.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "A symbol name or a registered codefile path." },
                        "depth": {
                            "type": "integer",
                            "description": "Call hops to walk back (default 3).",
                            "minimum": 1,
                            "maximum": 10
                        }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                })
            },
            call: |graph, args| {
                let target = args
                    .get("target")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("`target` is required"))?;
                let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
                let store = crate::commands::open_read(graph)?;
                crate::commands::impact_report(&store, target, depth)
            },
        },
        Tool {
            name: "loom_observe",
            description:
                "Run a command loom watches and keep what it saw. Prefix the test command you                  were going to run anyway: the run becomes a re-checkable record over the files                  it covered, and with `for_behavior` it binds to that behavior's proof and is                  graded. loom reports the outcome — you cannot supply one.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "argv, unquoted — loom quotes it for the shell.",
                            "minItems": 1
                        },
                        "for_behavior": {
                            "type": "string",
                            "description": "Intent id/name this run is evidence about."
                        },
                        "timeout": {
                            "type": "integer",
                            "description": "Seconds before giving up (default 900). A timeout records as blocked, never as a failure."
                        }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                })
            },
            call: |graph, args| {
                let command: Vec<String> = args
                    .get("command")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if command.is_empty() {
                    anyhow::bail!("`command` must be a non-empty argv array");
                }
                let target = args.get("for_behavior").and_then(|v| v.as_str());
                let timeout = args.get("timeout").and_then(|v| v.as_u64()).unwrap_or(900);
                crate::commands::observe_run(graph, target, timeout, &command)
            },
        },
        Tool {
            name: "loom_absorb",
            description:
                "Read the working tree and return the graph mutations it implies: new symbols in                  owned files, symbols whose callers all belong to one behavior, locators naming                  code that moved, files nothing owns. Observes only — nothing is written, and                  items loom cannot derive say what they need from you.",
            schema: no_args,
            call: |graph, _args| {
                let store = crate::commands::open_read(graph)?;
                let root = store.root().to_path_buf();
                let items = crate::absorb::observe(&store, &root)?;
                let ready = items.iter().filter(|i| i.needs.is_empty()).count();
                Ok(json!({
                    "items": serde_json::to_value(&items)?,
                    "ready": ready,
                    "needs_you": items.len() - ready,
                }))
            },
        },
        Tool {
            name: "loom_journal",
            description:
                "The append-only record of what happened to this graph: ratifications,                  rejections, observed runs, journey runs, drive turns. Read it to establish what                  actually occurred before changing anything — it is the one plane nothing                  rewrites.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Most recent entries to return (default 20).",
                            "minimum": 1
                        },
                        "event": {
                            "type": "string",
                            "description": "Optional event kind to filter by."
                        }
                    },
                    "additionalProperties": false
                })
            },
            call: |graph, args| {
                let store = crate::commands::open_read(graph)?;
                let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let event = args.get("event").and_then(|v| v.as_str());
                let mut entries = crate::journal::read(store.root())?;
                if let Some(kind) = event {
                    entries.retain(|e| e.event == kind);
                }
                let tail = entries.split_off(entries.len().saturating_sub(limit));
                Ok(serde_json::to_value(tail)?)
            },
        },
        Tool {
            name: "loom_apply",
            description:
                "Apply a batch of graph writes atomically: intents, groundings, relationships,                  verdicts, adjudications, vocabulary, tags. Every item goes through the same                  gates as its CLI equivalent — a batch cannot accept what a single write would                  refuse — and any failure rolls back the whole batch.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "fragment": {
                            "type": "object",
                            "description": "The apply fragment, same shape as the file `loom apply` reads."
                        }
                    },
                    "required": ["fragment"],
                    "additionalProperties": false
                })
            },
            call: |graph, args| {
                let fragment = args
                    .get("fragment")
                    .ok_or_else(|| anyhow::anyhow!("`fragment` is required"))?;
                crate::commands::apply_value(graph, fragment)
            },
        },
    ]
}

/// Serve MCP over stdio until stdin closes.
///
/// The transport is the caller's, not this function's: `loom mcp serve` hands
/// it stdin/stdout, and a test hands it a pair of in-memory buffers. That is
/// deliberate — the loop's real behavior (a parse error answered as -32700, a
/// panicking tool answered as -32603 without killing the session, EOF ending
/// the session cleanly) is only provable if the loop itself can be driven.
pub fn serve_stdio(graph: Option<&Path>) -> Result<()> {
    let stdin = std::io::stdin();
    serve(graph, stdin.lock(), std::io::stdout())
}

/// Serve MCP over one reader/writer pair until the reader reaches EOF.
pub fn serve<R: BufRead, W: Write>(
    graph: Option<&Path>,
    mut reader: R,
    mut writer: W,
) -> Result<()> {
    let graph = graph.map(PathBuf::from);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // Raw bytes, not `lines()`: a non-UTF-8 line must not abort the server.
        // Decode lossily and let the JSON parser reject it as a parse error.
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break; // EOF
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(line) {
            Ok(request) => {
                let id = request.get("id").cloned().unwrap_or(Value::Null);
                // A panic in one tool must not take down the session. Catch it
                // and answer with an internal error so the loop survives.
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle(graph.as_deref(), &request)
                })) {
                    Ok(response) => response,
                    Err(_) => Some(error_response(
                        id,
                        -32603,
                        "internal error: the request handler panicked",
                    )),
                }
            }
            // -32700 parse error. `id` is unknowable here, so it is null.
            Err(e) => Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {e}"),
            )),
        };
        if let Some(response) = response {
            writeln!(writer, "{}", serde_json::to_string(&response)?)?;
            writer.flush()?;
        }
    }
    Ok(())
}

/// Handle one JSON-RPC request, transport-free. Public so the protocol can be
/// exercised without a subprocess and stdio plumbing.
///
/// Returns `None` for notifications (no `id`), which must not be answered.
pub fn handle(graph: Option<&Path>, request: &Value) -> Option<Value> {
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = request.get("id").cloned();
    let Some(id) = id else {
        // A notification. `notifications/initialized` is the expected one; any
        // other is ignored rather than answered, per JSON-RPC.
        return None;
    };
    let params = request.get("params").cloned().unwrap_or(json!({}));

    match method {
        "initialize" => Some(ok_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") },
                "instructions":
                    "loom holds a falsifiable graph of what this codebase should do, where that \
                     lives, and how it is proven. Call loom_status to orient, loom_next for the \
                     next correct work, loom_context before editing unfamiliar code. loom records \
                     truth it can re-check; it will refuse a claim you cannot anchor.",
            }),
        )),
        "ping" => Some(ok_response(id, json!({}))),
        "tools/list" => {
            let list: Vec<Value> = tools()
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": (t.schema)(),
                    })
                })
                .collect();
            Some(ok_response(id, json!({ "tools": list })))
        }
        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let Some(tool) = tools().into_iter().find(|t| t.name == name) else {
                return Some(error_response(
                    id,
                    -32602,
                    &format!("unknown tool '{name}'"),
                ));
            };
            // A tool that fails reports the failure as tool CONTENT with
            // `isError`, not as a protocol error: the model should see why loom
            // refused (an unanchored claim, an unresolvable target) and adapt,
            // which a transport-level error would hide.
            Some(match (tool.call)(graph, &args) {
                Ok(value) => ok_response(id, tool_content(&value, false)),
                Err(e) => ok_response(id, tool_content(&json!({ "error": e.to_string() }), true)),
            })
        }
        other => Some(error_response(
            id,
            -32601,
            &format!("method '{other}' is not implemented"),
        )),
    }
}

fn tool_content(value: &Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(value).unwrap_or_else(|e| e.to_string()),
        }],
        "isError": is_error,
    })
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_never_answered() {
        let notification = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle(None, &notification).is_none());
    }

    #[test]
    fn initialize_advertises_tools_and_names_the_server() {
        let r = handle(
            None,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
        )
        .expect("initialize is a request");
        assert_eq!(r["result"]["serverInfo"]["name"], "loom");
        assert!(r["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn every_tool_lists_a_name_description_and_schema() {
        let r = handle(
            None,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .expect("tools/list is a request");
        let listed = r["result"]["tools"].as_array().unwrap();
        assert!(!listed.is_empty());
        for t in listed {
            assert!(t["name"].as_str().is_some_and(|n| n.starts_with("loom_")));
            assert!(t["description"].as_str().is_some_and(|d| d.len() > 40));
            assert_eq!(t["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn unknown_methods_and_tools_are_rejected_by_code() {
        let r = handle(
            None,
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" }),
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(
            None,
            &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                     "params": { "name": "loom_nope" } }),
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }
}
