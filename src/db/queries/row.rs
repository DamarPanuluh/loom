//! Row / value extraction helpers shared by every query submodule.

use grafeo::{QueryResult, Value};
use std::collections::HashMap;

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
