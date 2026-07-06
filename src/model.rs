//! Graph model — the core vocabulary of loom v2.
//!
//! Plane: this module is pure data + type system. It owns the node/edge/facet
//! shapes and the canonical enums. It performs NO storage. The store layer
//! (`crate::store`) persists these; the travel layer serializes them.
//!
//! Contract: every enum here has a stable canonical string (`as_str`) used in
//! both SQLite `CHECK` constraints and the deterministic JSON export. The two
//! must never drift — `as_str`/`parse` are the single source of truth.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Whether a criterion / evidence / reason field is a non-substantive
/// placeholder — either empty after trimming, or a whole-field filler token a
/// driver left unfilled. The write_back templates hand the driver `…` as the
/// hole, so a verbatim copy that forgets to fill it must be rejected at the
/// honesty boundary, not silently recorded as earned evidence.
///
/// This checks the WHOLE field, not a substring: real evidence may legitimately
/// contain an ellipsis (e.g. a truncated command-output excerpt), so only a
/// field that IS the placeholder is rejected.
pub fn is_placeholder(s: &str) -> bool {
    // Strip whitespace and quote/backtick wrappers only — NOT angle brackets,
    // so a whole-field `<reason>` hole stays detectable below.
    let raw = s
        .trim()
        .trim_matches(|c: char| matches!(c, '\'' | '"' | '`'))
        .trim();
    if raw.is_empty() {
        return true;
    }
    // A field that IS `<…>` (or `[…]`) is an unfilled write_back hole:
    // `<symbol>`, `<reason>`, `<what was built>`, `<passing|failing|independent>`.
    if (raw.starts_with('<') && raw.ends_with('>')) || (raw.starts_with('[') && raw.ends_with(']'))
    {
        return true;
    }
    matches!(
        raw.to_ascii_lowercase().as_str(),
        "…" | "..."
            | ". . ."
            | "todo"
            | "tbd"
            | "tba"
            | "n/a"
            | "na"
            | "none"
            | "-"
            | "--"
            | "."
            | "?"
            | "???"
            | "xxx"
            | "fixme"
            | "placeholder"
    )
}

#[cfg(test)]
mod placeholder_tests {
    use super::is_placeholder;

    #[test]
    fn rejects_whole_field_placeholders() {
        for p in [
            "",
            "  ",
            "…",
            "...",
            "<...>",
            "TODO",
            "tbd",
            "n/a",
            "-",
            ".",
            "???",
            "'…'",
            "<reason>",
            "<what was built>",
            "<symbol>",
            "[fill me]",
        ] {
            assert!(is_placeholder(p), "should reject placeholder {p:?}");
        }
    }

    #[test]
    fn accepts_substantive_text_even_with_ellipsis() {
        // Real evidence that merely CONTAINS an ellipsis (truncated output) is fine.
        for s in [
            "src/store/edges.rs:110 gates empty evidence",
            "test output: assertion failed at line 42 …",
            "no auth check before delete_user()",
        ] {
            assert!(!is_placeholder(s), "should accept substantive {s:?}");
        }
    }
}

/// Error returned when a string fails to parse into a model enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}

impl fmt::Display for ParseEnumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {}: '{}'", self.kind, self.value)
    }
}

impl std::error::Error for ParseEnumError {}

/// Macro: define a string-backed enum with canonical `as_str`, `FromStr`,
/// `Display`, serde (as string), and an `ALL` slice. This keeps the canonical
/// spelling in exactly one place per variant.
macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(&self) -> &'static str {
                match self {
                    $($name::$variant => $s),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok($name::$variant),)+
                    other => Err(ParseEnumError {
                        kind: stringify!($name),
                        value: other.to_string(),
                    }),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
                ser.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                let s = String::deserialize(de)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

str_enum! {
    /// The kinds of node the graph can hold. Two cornerstones (Intent, CodeFile)
    /// and their supporting families plus cross-cutting nodes.
    NodeType {
        Intent => "intent",
        CodeFile => "codefile",
        QualityRule => "quality_rule",
        CodeRule => "code_rule",
        Validation => "validation",
        Hypothesis => "hypothesis",
        Finding => "finding",
        InterfaceSurface => "interface_surface",
        Note => "note",
        InboxItem => "inbox_item",
        TaskRecord => "task_record",
        Proposal => "proposal",
        JourneyCoverage => "journey_coverage",
        JourneyInvariantPoint => "journey_invariant_point",
        WikiPage => "wiki_page",
        UpstreamIntent => "upstream_intent",
    }
}

str_enum! {
    /// How a stored edge's truth becomes true. `statistical` is intentionally
    /// absent: statistical signals are computed feeds (`DebtCluster`), never
    /// stored edges. (See graph-model.md, INV-3.)
    TruthClass {
        Derived => "derived",
        Asserted => "asserted",
    }
}

str_enum! {
    /// The inspection state of an edge. `current` is the resting state of a
    /// derived edge; the rest form the asserted verdict lifecycle.
    InspectionStatus {
        Current => "current",
        Uninspected => "uninspected",
        Passing => "passing",
        Failing => "failing",
        Independent => "independent",
        NeedsReverification => "needs_reverification",
        Blocked => "blocked",
    }
}

str_enum! {
    /// Prescriptive lifecycle of an Intent.
    IntentLifecycle {
        Planned => "planned",
        Implemented => "implemented",
        NeedsChange => "needs_change",
        Deprecated => "deprecated",
    }
}

str_enum! {
    /// The proof mechanism of a Validation.
    ValidationType {
        Test => "test",
        Assertion => "assertion",
        Benchmark => "benchmark",
        ManualCheck => "manual_check",
        Journey => "journey",
        Scenario => "scenario",
        Contract => "contract",
    }
}

str_enum! {
    /// The kind of edge relationship. Endpoint types and allowed truth classes
    /// are enforced by the edge-kind registry (`crate::registry`).
    EdgeKind {
        Hierarchy => "hierarchy",
        Requires => "requires",
        ScenarioOf => "scenario_of",
        VariantOf => "variant_of",
        Triggers => "triggers",
        Sequence => "sequence",
        Implements => "implements",
        Validates => "validates",
        Governs => "governs",
        Targets => "targets",
        Flags => "flags",
        Assesses => "assesses",
        Exposes => "exposes",
        Calls => "calls",
        Relates => "relates",
        Covers => "covers",
        Asserts => "asserts",
        Documents => "documents",
        DependsOn => "depends_on",
    }
}

str_enum! {
    /// The claim a grounding (`implements`) edge makes about its file. Stored as
    /// the `role` edge facet; a missing facet reads as `Realizes` (the historical
    /// default, so pre-role graphs keep their exact semantics).
    ///
    /// Only `Realizes` bears ownership: a file grounded solely by
    /// `consumes`/`configures`/`verifies` edges is still unowned and stays in the
    /// coverage queue. The distinction is what keeps a consumer surface (a page
    /// that calls a backend route, a test that exercises a behavior) from
    /// silently satisfying the coverage gate for behavior that lives elsewhere.
    GroundingRole {
        Realizes => "realizes",
        Consumes => "consumes",
        Configures => "configures",
        Verifies => "verifies",
    }
}

/// A node row. Type-specific structured fields live in `body` (JSON); queryable
/// attributes live as facets. `status` carries the per-type lifecycle string
/// (Intent lifecycle, Validation last_result, Hypothesis status, …).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: String,
    /// How this node's existence becomes true. Most nodes are `asserted`
    /// (a human/LLM chose to record them). `Finding` nodes are `derived` —
    /// recomputed by sync and wiped/rebuilt deterministically (INV-2).
    #[serde(default = "default_asserted")]
    pub truth_class: TruthClass,
    #[serde(default = "empty_object")]
    pub body: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

fn default_asserted() -> TruthClass {
    TruthClass::Asserted
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// An edge row in the unified edge table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub id: String,
    pub from_id: String,
    pub to_id: String,
    pub kind: EdgeKind,
    pub truth_class: TruthClass,
    pub status: InspectionStatus,
    #[serde(default)]
    pub criterion: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default = "empty_array")]
    pub depends_on: serde_json::Value,
    #[serde(default)]
    pub inspected_by: String,
    pub created_at: String,
    pub updated_at: String,
}

fn empty_array() -> serde_json::Value {
    serde_json::Value::Array(Vec::new())
}

str_enum! {
    /// Whether a facet/tag attaches to a node or an edge.
    TargetKind {
        Node => "node",
        Edge => "edge",
    }
}

/// A typed key=value attribute on a node or edge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Facet {
    pub target_id: String,
    pub target_kind: TargetKind,
    pub key: String,
    pub value: String,
    pub truth_class: TruthClass,
}

/// A membership tag on a node or edge, drawn from the tag vocabulary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tag {
    pub target_id: String,
    pub target_kind: TargetKind,
    pub term: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_roundtrip_is_stable() {
        for nt in NodeType::ALL {
            assert_eq!(NodeType::from_str(nt.as_str()).unwrap(), *nt);
        }
        for ek in EdgeKind::ALL {
            assert_eq!(EdgeKind::from_str(ek.as_str()).unwrap(), *ek);
        }
        for st in InspectionStatus::ALL {
            assert_eq!(InspectionStatus::from_str(st.as_str()).unwrap(), *st);
        }
    }

    #[test]
    fn truth_class_has_no_statistical_variant() {
        // INV-3 guard at the type level: statistical is never a stored truth class.
        assert_eq!(TruthClass::ALL.len(), 2);
        assert!(TruthClass::from_str("statistical").is_err());
    }

    #[test]
    fn unknown_enum_value_errors() {
        assert!(NodeType::from_str("nonsense").is_err());
    }
}
