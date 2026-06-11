//! Row / value extraction helpers shared by every query submodule.

use grafeo::{QueryResult, Value};
use std::collections::HashMap;

/// Read a native-list property into Vec<String>. Legacy tolerance: pre-v5
/// graphs stored these as JSON-encoded strings — a String value is parsed as
/// JSON so reads stay correct mid-migration (doctor + `loom migrate` converge
/// the storage; this fallback never writes anything back).
pub fn list_val(v: &Value) -> Vec<String> {
    match v {
        Value::List(items) => items
            .iter()
            .map(|x| match x {
                Value::String(s) => s.to_string(),
                other => format!("{other:?}"),
            })
            .collect(),
        Value::String(s) if !s.trim().is_empty() => {
            serde_json::from_str(s.as_ref()).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// A native-list parameter value from string items.
pub fn list_param(items: &[String]) -> Value {
    Value::List(
        items
            .iter()
            .map(|s| Value::String(s.clone().into()))
            .collect::<Vec<_>>()
            .into(),
    )
}

/// Build a `$name` parameter map from string pairs — the write path for
/// free-text fields (see `LoomDb::execute_with_params`).
pub fn sparams(pairs: &[(&str, &str)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::from(*v)))
        .collect()
}

pub fn str_val(v: &Value) -> String {
    match v {
        Value::String(s)  => s.to_string(),
        Value::Bool(b)    => b.to_string(),
        Value::Int64(n)   => n.to_string(),
        Value::Float64(f) => f.to_string(),
        Value::Null       => String::new(),
        _                 => String::new(),
    }
}

pub fn f64_val(v: &Value) -> f64 {
    match v {
        Value::Float64(f) => *f,
        Value::Int64(n)   => *n as f64,
        _                 => 0.0,
    }
}

pub fn i64_val(v: &Value) -> i64 {
    match v {
        Value::Int64(n)   => *n,
        Value::Float64(f) => *f as i64,
        _                 => 0,
    }
}

/// Build a column-name → index lookup from a QueryResult.
pub fn col_map(result: &QueryResult) -> HashMap<&str, usize> {
    result
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| (name.as_str(), i))
        .collect()
}

/// Get a value from a row by column name, using the column map.
pub fn get<'a>(row: &'a [Value], cols: &HashMap<&str, usize>, name: &str) -> &'a Value {
    static NULL: Value = Value::Null;
    cols.get(name)
        .and_then(|&i| row.get(i))
        .unwrap_or(&NULL)
}
