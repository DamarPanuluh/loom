//! Canonical JSON — one key ordering, so two hashes of the same value agree.
//!
//! Plane: pure data. Knows nothing about storage, the graph, or the CLI.
//!
//! This rule existed five times, under four names, in five modules —
//! `journey::spec`, `journey_runtime::values`, `commands::journey::derive`,
//! `release::section_08` and `completeness` — and every copy fed a content hash
//! that another module compared against. Five bodies had to agree exactly or
//! two subsystems that both called their output "canonical" would produce
//! different bytes for the same value, with nothing in the crate to catch it.

use serde_json::Value;
use std::collections::BTreeMap;

/// Recursively sort every object's keys, preserving array order.
///
/// Object key order is not semantic in JSON but IS semantic to a byte hash, so
/// anything hashed or compared byte-wise goes through here first. Arrays keep
/// their order: position is meaning in an array.
pub fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(object) => {
            let sorted: BTreeMap<String, Value> = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_keys_sort_at_every_depth() {
        let v = serde_json::json!({"b": 1, "a": {"z": 2, "y": 3}});
        assert_eq!(
            serde_json::to_string(&canonicalize(v)).unwrap(),
            r#"{"a":{"y":3,"z":2},"b":1}"#
        );
    }

    #[test]
    fn array_order_is_meaning_and_is_preserved() {
        let v = serde_json::json!([{"b": 1, "a": 2}, "z", "y"]);
        assert_eq!(
            serde_json::to_string(&canonicalize(v)).unwrap(),
            r#"[{"a":2,"b":1},"z","y"]"#
        );
    }

    #[test]
    fn two_orderings_of_one_value_canonicalize_identically() {
        let a = serde_json::json!({"x": {"p": 1, "q": 2}, "w": [3, 4]});
        let b = serde_json::json!({"w": [3, 4], "x": {"q": 2, "p": 1}});
        assert_eq!(canonicalize(a), canonicalize(b));
    }
}
