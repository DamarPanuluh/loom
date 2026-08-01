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

/// Backwards-compatible re-export; statement grammar now owns this predicate.
pub use crate::grammar::is_placeholder;

/// The short display prefix of an id — the `[abcd1234]` form every command
/// echoes. Char-boundary safe: raw `&id[..8]` panics on an id shorter than 8
/// bytes or when byte 8 lands inside a multibyte char; this takes up to the
/// first 8 characters instead.
pub fn short(id: &str) -> &str {
    let end = id.char_indices().nth(8).map(|(i, _)| i).unwrap_or(id.len());
    &id[..end]
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
        Question => "question",
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
        Questions => "questions",
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

str_enum! {
    /// What KIND of assertion a fact makes about its subject. One row per
    /// (subject, claim), so a fact has exactly one current state.
    ///
    /// - `Verdict` — an edge verdict: passing / failing / independent / blocked.
    /// - `Observation` — an observation about a node (finding intake).
    /// - `Adjudication` — a finding verdict: needed / justified / rejected / …
    /// - `Ratification` — wantedness: does the authority want this behavior?
    Claim {
        Verdict => "verdict",
        Observation => "observation",
        Adjudication => "adjudication",
        Ratification => "ratification",
    }
}

str_enum! {
    /// How strongly a fact is anchored — the strength lattice, ordered
    /// `Verified > Cited > Claimed > Expired`.
    ///
    /// This is the organizing idea of the evidence spine: loom records what an
    /// agent asserts, but only counts what loom can independently RE-CHECK. A
    /// `Claimed` fact is a real record and a real part of the audit trail; it
    /// simply never satisfies a maturity rung, so it never settles and stays in
    /// its lane's queue.
    ///
    /// - `Verified` — loom ran something and observed the result itself.
    /// - `Cited` — the fact cites spans or journal entries that still resolve.
    /// - `Claimed` — prose only. Recorded, never counted.
    /// - `Expired` — every anchor this fact had has since broken.
    Verification {
        Verified => "verified",
        Cited => "cited",
        Claimed => "claimed",
        Expired => "expired",
    }
}

impl Verification {
    /// Rank on the strength lattice; higher is stronger.
    pub fn rank(self) -> u8 {
        match self {
            Verification::Expired => 0,
            Verification::Claimed => 1,
            Verification::Cited => 2,
            Verification::Verified => 3,
        }
    }

    /// Whether a fact at this strength may satisfy a maturity rung.
    pub fn counts(self) -> bool {
        self.rank() >= Verification::Cited.rank()
    }
}

str_enum! {
    /// The form of one piece of evidence.
    ///
    /// - `Run` — a command loom executed. Never accepted from a caller.
    /// - `Span` — a cited file span, fingerprinted at assert time.
    /// - `Journal` — a `journal:<id>` reference into the append-only journal.
    /// - `Claim` — free prose.
    EvidenceKind {
        Run => "run",
        Span => "span",
        Journal => "journal",
        Claim => "claim",
    }
}

str_enum! {
    /// Which of loom's own probes produced a `Run`.
    ///
    /// - `Command` / `Journey` — a validation command or journey replay.
    /// - `Prescreen` — a quality rule's patterns scanned over the grounded
    ///   files. This is how an ABSENCE ("no hardcoded secrets here") becomes
    ///   re-checkable: loom ran the scan itself and found nothing.
    /// - `Locator` — a grounding locator re-resolved against live symbols.
    /// - `Seam` — a consumer/config/verify grounding: the seam it names is
    ///   still present in the file. Content may churn freely underneath it;
    ///   only the seam leaving re-opens the claim, because the claim was never
    ///   that the behavior lives here.
    /// - `Detector` — a structural finding's own predicate, re-evaluated.
    RunProducer {
        Command => "command",
        Journey => "journey",
        Prescreen => "prescreen",
        Locator => "locator",
        Seam => "seam",
        Detector => "detector",
    }
}

str_enum! {
    /// Why a fact was re-opened. Replaces the prose `stale_cause` string whose
    /// routing class downstream code recovered by substring matching.
    StaleCause {
        RunCoveredFileChanged => "run_covered_file_changed",
        RunCommandChanged => "run_command_changed",
        SpanRewritten => "span_rewritten",
        SeamGone => "seam_gone",
        ScopeFileChanged => "scope_file_changed",
        SpanFileDeleted => "span_file_deleted",
        JournalMissing => "journal_missing",
        SubjectRedefined => "subject_redefined",
        RoleChanged => "role_changed",
        Rehomed => "rehomed",
        AnchorMissing => "anchor_missing",
    }
}

str_enum! {
    /// What re-opening this fact will cost — the router's cost class.
    ///
    /// - `Reconfirm` — anchors still hold; confirm the unchanged claim.
    /// - `Reinspect` — an anchor was rewritten; inspect afresh.
    /// - `Reanchor` — the fact has no live anchor at all; find one.
    Rework {
        Reconfirm => "reconfirm",
        Reinspect => "reinspect",
        Reanchor => "reanchor",
    }
}

impl StaleCause {
    /// The cost class this cause implies.
    pub fn rework(self) -> Rework {
        match self {
            StaleCause::AnchorMissing | StaleCause::JournalMissing => Rework::Reanchor,
            StaleCause::SpanRewritten
            | StaleCause::SeamGone
            | StaleCause::SpanFileDeleted
            | StaleCause::SubjectRedefined
            | StaleCause::RoleChanged
            | StaleCause::Rehomed => Rework::Reinspect,
            StaleCause::RunCoveredFileChanged
            | StaleCause::RunCommandChanged
            // The recorded justification still exists; only the file around it
            // moved. Re-reading it is cheap.
            | StaleCause::ScopeFileChanged => Rework::Reconfirm,
        }
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
    /// (a human/LLM chose to record them). `Finding` is the shared finding node:
    /// programmatic producers create derived findings, while manual evidence-backed
    /// observations enter as asserted findings.
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
    /// PROJECTIONS of this edge's `verdict` fact — read-only. `evidence` is
    /// gone entirely: an edge no longer carries a prose justification, because
    /// prose is one kind of evidence among several and the weakest one. Read
    /// the fact and its evidence rows instead.
    #[serde(default)]
    pub criterion: String,
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
