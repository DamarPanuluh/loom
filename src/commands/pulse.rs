//! Shared post-write pulse for mutating command handlers.
//!
//! Plane: CLI write-back UX. A graph mutation must not strand the driver at a
//! confirmation line: JSON receives a single enriched object, while text keeps
//! the command's human summary and appends one explicit next move.

use crate::store::Store;
use crate::{workitem, Result};
use serde_json::{Map, Value};

/// Emit a mutation result and the next reorientation step.
///
/// In JSON mode this prints exactly one pretty object: the caller's payload
/// fields plus `next_step` and a fresh `graph_state` pulse. In text mode the
/// caller-owned human renderer runs, then a single `next: ...` line is appended.
pub(crate) fn emit<F>(
    store: &Store,
    json: bool,
    payload: Value,
    next_step: &str,
    human: F,
) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    if json {
        let mut object = match payload {
            Value::Object(map) => map,
            other => {
                let mut map = Map::new();
                map.insert("result".to_string(), other);
                map
            }
        };
        object.insert(
            "next_step".to_string(),
            Value::String(next_step.to_string()),
        );
        object.insert(
            "graph_state".to_string(),
            serde_json::to_value(workitem::graph_state(store)?)?,
        );
        println!("{}", serde_json::to_string_pretty(&Value::Object(object))?);
    } else {
        human()?;
        println!("next: {next_step}");
    }
    Ok(())
}

/// Convenience wrapper for the common single-line text confirmation case.
pub(crate) fn emit_line(
    store: &Store,
    json: bool,
    payload: Value,
    next_step: &str,
    line: impl Into<String>,
) -> Result<()> {
    let line = line.into();
    emit(store, json, payload, next_step, || {
        println!("{line}");
        Ok(())
    })
}
