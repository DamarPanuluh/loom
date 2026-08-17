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
use anyhow::{bail, Context};
use serde_json::{json, Value};
use std::collections::BTreeSet;
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

/// One non-executing classification of a request in an authored MCP
/// transcript. Release policy consumes this instead of re-parsing JSON-RPC or
/// guessing which tool arguments can hide another executable capability.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectedMcpRequest {
    pub index: usize,
    pub id: Value,
    pub kind: InspectedMcpRequestKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InspectedMcpRequestKind {
    Initialize,
    ToolsList,
    Ping,
    /// The one reviewed negative protocol probe. It is never dispatched to a
    /// tool and therefore carries no executable capability.
    UnknownTool {
        name: String,
    },
    ToolCall {
        tool: String,
        effect: McpTranscriptEffect,
        arguments: Value,
        /// Present only for loom_observe. The caller must recursively inspect
        /// this exact argv before authorizing the enclosing transcript.
        nested_argv: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTranscriptEffect {
    Read,
    ObserveArgv,
    ApplyFragment,
}

fn no_args() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "loom_status",
            description:
                "Where this Journey-root graph stands: authored, derived, grounded, surfaced, \
                 and proven maturity rungs; the compass (the single next move); per-lane queue \
                 depths; and code ownership. Call this first in a session and after any change.",
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
                 write-back, and the files to read first. Journey-root work is explicitly served \
                 through `derive` and `surface` lanes. Omit `lane` to take whatever the compass \
                 points at. Every packet carries a packet_id.",
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
                "Read-only context for an authored Journey, technical Intent, registered file path, \
                 or free-text query: its linked projections, grounding, proof, questions, rules, and \
                 staleness. For Journey projection gaps, pull loom_next with lane `derive` or \
                 `surface`. Pull context BEFORE editing code you did not write.",
            schema: || {
                json!({
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "description": "Journey or Intent id/name/prefix, registered codefile path, or free text."
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
                let command = args
                    .get("command")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow::anyhow!("`command` must be a non-empty argv array"))?
                    .iter()
                    .map(|token| {
                        token.as_str().map(str::to_string).ok_or_else(|| {
                            anyhow::anyhow!("`command` must be a non-empty argv array")
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
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
                "The append-only record of what happened to this graph: Journey derivation                  acceptance, exemptions, ratifications, rejections, and observed compiled-profile                  runs. Read it to establish what actually occurred before changing anything — it is                  the one plane nothing rewrites.",
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
                let rows: Vec<serde_json::Value> = tail
                    .into_iter()
                    .map(|entry| {
                        let origin = entry.origin;
                        let mut value = serde_json::to_value(entry)?;
                        value["origin"] = serde_json::to_value(origin)?;
                        Ok::<_, anyhow::Error>(value)
                    })
                    .collect::<crate::Result<_>>()?;
                Ok(serde_json::Value::Array(rows))
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

/// Drive a complete, validated JSON-RPC transcript through the real MCP serve
/// loop and return its responses in wire order as one JSON document.
///
/// This is a client adapter around [`serve`], not a second request dispatcher:
/// request classification, tool execution, session continuity, and response
/// ordering therefore remain owned by the same loop as `loom mcp serve`.
pub fn transcript(graph: Option<&Path>, requests_json: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(requests_json).context("--requests-json must be valid JSON")?;
    let requests = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("--requests-json must be a JSON array"))?;
    inspect_transcript_value(&value)?;

    let mut input = Vec::new();
    for request in requests {
        serde_json::to_writer(&mut input, request)?;
        input.push(b'\n');
    }

    let mut output = Vec::new();
    serve(graph, std::io::Cursor::new(input), &mut output)?;
    let output = std::str::from_utf8(&output).context("MCP transcript emitted non-UTF-8 output")?;
    let responses = output
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<Value>(line)
                .with_context(|| format!("MCP response {} was not valid JSON", index + 1))
        })
        .collect::<Result<Vec<_>>>()?;

    if responses.len() != requests.len() {
        bail!(
            "MCP transcript returned {} responses for {} requests",
            responses.len(),
            requests.len()
        );
    }

    Ok(json!({
        "protocol": "json-rpc-2.0",
        "request_count": requests.len(),
        "response_count": responses.len(),
        "responses": responses,
        "session_completed": true,
    }))
}

/// Parse and strictly classify an authored MCP transcript without executing a
/// request, opening a graph, or resolving a command. The real transcript
/// adapter calls the same inspector before dispatch so policy and execution
/// cannot disagree about the accepted envelope or tool arguments.
#[doc(hidden)]
pub fn inspect_transcript_requests(requests_json: &str) -> Result<Vec<InspectedMcpRequest>> {
    let value: Value =
        serde_json::from_str(requests_json).context("--requests-json must be valid JSON")?;
    inspect_transcript_value(&value)
}

fn inspect_transcript_value(value: &Value) -> Result<Vec<InspectedMcpRequest>> {
    let requests = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("--requests-json must be a JSON array"))?;
    if requests.is_empty() {
        bail!("--requests-json must contain at least one request");
    }
    let mut ids = BTreeSet::new();
    let mut inspected = Vec::with_capacity(requests.len());
    for (index, request) in requests.iter().enumerate() {
        let item = inspect_transcript_request(request, index)?;
        let id_key = serde_json::to_string(&item.id)?;
        if !ids.insert(id_key) {
            bail!("request {} repeats an earlier id", index + 1);
        }
        inspected.push(item);
    }
    Ok(inspected)
}

fn inspect_transcript_request(request: &Value, index: usize) -> Result<InspectedMcpRequest> {
    let number = index + 1;
    let object = request
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("request {number} must be a JSON object"))?;
    require_exact_keys(
        object,
        &["id", "jsonrpc", "method", "params"],
        &[],
        &format!("request {number}"),
    )?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        bail!("request {number} must declare jsonrpc \"2.0\"");
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .filter(|method| !method.is_empty())
        .ok_or_else(|| anyhow::anyhow!("request {number} must have a non-empty string method"))?;
    let id = object
        .get("id")
        .ok_or_else(|| anyhow::anyhow!("request {number} must have an id"))?;
    if !(id.is_string() || id.is_number()) {
        bail!("request {number} id must be a string or number");
    }
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    let kind = match method {
        "initialize" => {
            require_empty_object(&params, &format!("request {number} initialize params"))?;
            InspectedMcpRequestKind::Initialize
        }
        "tools/list" => {
            require_empty_object(&params, &format!("request {number} tools/list params"))?;
            InspectedMcpRequestKind::ToolsList
        }
        "ping" => {
            require_empty_object(&params, &format!("request {number} ping params"))?;
            InspectedMcpRequestKind::Ping
        }
        "tools/call" => inspect_tool_call(&params, number)?,
        other => bail!("request {number} method '{other}' is not allowed in a transcript"),
    };
    Ok(InspectedMcpRequest {
        index,
        id: id.clone(),
        kind,
    })
}

fn inspect_tool_call(params: &Value, number: usize) -> Result<InspectedMcpRequestKind> {
    let object = params
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("request {number} tools/call params must be an object"))?;
    require_exact_keys(
        object,
        &["arguments", "name"],
        &["arguments", "name"],
        &format!("request {number} tools/call params"),
    )?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow::anyhow!("request {number} tool name must be a non-empty string"))?;
    let arguments = object
        .get("arguments")
        .ok_or_else(|| anyhow::anyhow!("request {number} tool arguments are required"))?;
    if name == "loom_nope" {
        require_empty_object(arguments, &format!("request {number} loom_nope arguments"))?;
        return Ok(InspectedMcpRequestKind::UnknownTool { name: name.into() });
    }
    let tool = tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| {
            anyhow::anyhow!("request {number} unknown tool '{name}' is not an allowed probe")
        })?;
    validate_schema_value(
        arguments,
        &(tool.schema)(),
        &format!("request {number} {name} arguments"),
    )?;
    let (effect, nested_argv) = match name {
        "loom_next" => {
            let args = arguments.as_object().ok_or_else(|| {
                anyhow::anyhow!("request {number} loom_next arguments must be an object")
            })?;
            if !args.is_empty()
                && !(args.len() == 1 && args.get("lane").and_then(Value::as_str) == Some("ratify"))
            {
                bail!("request {number} loom_next admits only current empty or ratify-lane probes");
            }
            (McpTranscriptEffect::Read, None)
        }
        "loom_observe" => {
            let args = arguments.as_object().ok_or_else(|| {
                anyhow::anyhow!("request {number} loom_observe arguments must be an object")
            })?;
            if args.len() != 1 {
                bail!("request {number} loom_observe admits only an exact command argv");
            }
            let argv = args
                .get("command")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    anyhow::anyhow!("request {number} loom_observe command must be an array")
                })?
                .iter()
                .map(|token| {
                    token
                        .as_str()
                        .filter(|token| !token.is_empty() && !token.contains('\0'))
                        .map(ToString::to_string)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "request {number} loom_observe argv must contain nonempty strings"
                            )
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            if argv.first().map(String::as_str) != Some("loom") {
                bail!("request {number} loom_observe may invoke only bare Loom argv");
            }
            (McpTranscriptEffect::ObserveArgv, Some(argv))
        }
        "loom_apply" => {
            validate_adjudications_only_apply(arguments, number)?;
            (McpTranscriptEffect::ApplyFragment, None)
        }
        _ => (McpTranscriptEffect::Read, None),
    };
    Ok(InspectedMcpRequestKind::ToolCall {
        tool: name.into(),
        effect,
        arguments: arguments.clone(),
        nested_argv,
    })
}

fn validate_adjudications_only_apply(arguments: &Value, number: usize) -> Result<()> {
    let fragment = arguments
        .get("fragment")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("request {number} loom_apply fragment must be an object"))?;
    require_exact_keys(
        fragment,
        &["adjudications"],
        &["adjudications"],
        &format!("request {number} loom_apply fragment"),
    )?;
    let adjudications = fragment
        .get("adjudications")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("request {number} loom_apply adjudications must be a nonempty array")
        })?;
    for (index, item) in adjudications.iter().enumerate() {
        let item = item.as_object().ok_or_else(|| {
            anyhow::anyhow!(
                "request {number} loom_apply adjudication {} must be an object",
                index + 1
            )
        })?;
        require_exact_keys(
            item,
            &["finding", "reason", "verdict"],
            &["finding", "reason", "verdict"],
            &format!("request {number} loom_apply adjudication {}", index + 1),
        )?;
        for field in ["finding", "reason"] {
            if item
                .get(field)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                bail!(
                    "request {number} loom_apply adjudication {} has invalid {field}",
                    index + 1
                );
            }
        }
        if item.get("verdict").and_then(Value::as_str) != Some("needed") {
            bail!(
                "request {number} loom_apply adjudication {} must use verdict needed",
                index + 1
            );
        }
    }
    Ok(())
}

fn require_empty_object(value: &Value, context: &str) -> Result<()> {
    if !value.as_object().is_some_and(serde_json::Map::is_empty) {
        bail!("{context} must be an empty object");
    }
    Ok(())
}

fn require_exact_keys(
    object: &serde_json::Map<String, Value>,
    allowed: &[&str],
    required: &[&str],
    context: &str,
) -> Result<()> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            bail!("{context} contains unknown field '{key}'");
        }
    }
    for key in required {
        if !object.contains_key(*key) {
            bail!("{context} is missing required field '{key}'");
        }
    }
    Ok(())
}

fn validate_schema_value(value: &Value, schema: &Value, context: &str) -> Result<()> {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let object = value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("{context} must be an object"))?;
            let properties = schema
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
                for key in object.keys() {
                    if !properties.contains_key(key) {
                        bail!("{context} contains unknown field '{key}'");
                    }
                }
            }
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for key in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(key) {
                        bail!("{context} is missing required field '{key}'");
                    }
                }
            }
            for (key, child) in object {
                if let Some(child_schema) = properties.get(key) {
                    validate_schema_value(child, child_schema, &format!("{context}.{key}"))?;
                }
            }
        }
        Some("array") => {
            let values = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("{context} must be an array"))?;
            if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64) {
                if values.len() < minimum as usize {
                    bail!("{context} must contain at least {minimum} item(s)");
                }
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in values.iter().enumerate() {
                    validate_schema_value(item, item_schema, &format!("{context}[{index}]"))?;
                }
            }
        }
        Some("string") => {
            let text = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{context} must be a string"))?;
            if let Some(values) = schema.get("enum").and_then(Value::as_array) {
                if !values.iter().any(|value| value.as_str() == Some(text)) {
                    bail!("{context} is outside the declared enum");
                }
            }
        }
        Some("integer") => {
            let integer = value
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("{context} must be an integer"))?;
            if schema
                .get("minimum")
                .and_then(Value::as_i64)
                .is_some_and(|minimum| integer < minimum)
            {
                bail!("{context} is below the declared minimum");
            }
            if schema
                .get("maximum")
                .and_then(Value::as_i64)
                .is_some_and(|maximum| integer > maximum)
            {
                bail!("{context} is above the declared maximum");
            }
        }
        Some(other) => bail!("{context} uses unsupported schema type '{other}'"),
        None => {}
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
        "initialize" => {
            if let Some(requested) = params
                .get("protocolVersion")
                .and_then(|value| value.as_str())
            {
                if requested != PROTOCOL_VERSION {
                    return Some(error_response(
                        id,
                        -32602,
                        &format!(
                            "unsupported protocol version '{requested}'; loom speaks {PROTOCOL_VERSION}"
                        ),
                    ));
                }
            }
            Some(ok_response(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "loom", "version": env!("CARGO_PKG_VERSION") },
                    "instructions":
                        "loom roots product meaning in authored Journeys, derives technical Intents, \
                         surfaces real target-repository CLIs, and proves compiled profiles through \
                         those surfaces. Call loom_status to orient, loom_next for the next correct \
                         work (including derive/surface), and loom_context before editing unfamiliar \
                         code. loom records truth it can re-check and refuses unanchored claims.",
                }),
            ))
        }
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
