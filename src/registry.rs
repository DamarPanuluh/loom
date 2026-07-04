//! Edge-kind registry — the type system for edges.
//!
//! Plane: pure data + validation. SQLite foreign keys cannot express
//! "endpoint type depends on edge kind", so this registry enforces that in code
//! at the write boundary. `crate::store` consults it before every edge write;
//! `loom doctor` re-validates every stored edge against it.
//!
//! Contract: every `EdgeKind` MUST have exactly one registry entry. The
//! `registry_is_total` test guards that. Endpoint types, allowed truth classes,
//! and the owning role all live here, in one place.

use crate::model::{EdgeKind, NodeType, TruthClass};

/// The lane that owns writes for an edge kind. Role gates (ring 3) enforce this;
/// `sync` is the special non-LLM writer for derived edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerRole {
    Builder,
    Analyzer,
    Fixer,
    Validator,
    Quality,
    Sync,
}

impl OwnerRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            OwnerRole::Builder => "builder",
            OwnerRole::Analyzer => "analyzer",
            OwnerRole::Fixer => "fixer",
            OwnerRole::Validator => "validator",
            OwnerRole::Quality => "quality",
            OwnerRole::Sync => "sync",
        }
    }
}

/// One entry in the edge-kind registry: the legal shape of an edge kind.
#[derive(Debug, Clone)]
pub struct EdgeKindSpec {
    pub kind: EdgeKind,
    pub from: NodeType,
    pub to: NodeType,
    /// Allowed truth classes. One value for most kinds; both for `exposes`
    /// (derived when sync-extracted, asserted when human/LLM-declared).
    pub truth_classes: &'static [TruthClass],
    pub owner: OwnerRole,
    pub description: &'static str,
}

impl EdgeKindSpec {
    pub fn allows_truth_class(&self, tc: TruthClass) -> bool {
        self.truth_classes.contains(&tc)
    }
}

use NodeType::*;
use TruthClass::*;

/// The complete edge-kind registry. The single source of edge typing.
pub const REGISTRY: &[EdgeKindSpec] = &[
    EdgeKindSpec {
        kind: EdgeKind::Hierarchy,
        from: Intent,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "part-of decomposition",
    },
    EdgeKindSpec {
        kind: EdgeKind::Requires,
        from: Intent,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "this behavior depends on another",
    },
    EdgeKindSpec {
        kind: EdgeKind::ScenarioOf,
        from: Intent,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "child scenario to parent capability",
    },
    EdgeKindSpec {
        kind: EdgeKind::VariantOf,
        from: Intent,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "variant to base behavior",
    },
    EdgeKindSpec {
        kind: EdgeKind::Triggers,
        from: Intent,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "when condition occurs, response must hold",
    },
    EdgeKindSpec {
        kind: EdgeKind::Sequence,
        from: Intent,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "ordered step in a journey",
    },
    EdgeKindSpec {
        kind: EdgeKind::Implements,
        from: Intent,
        to: CodeFile,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "behavior realized at file/locator",
    },
    EdgeKindSpec {
        kind: EdgeKind::Validates,
        from: Validation,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Validator,
        description: "proof checks behavior",
    },
    EdgeKindSpec {
        kind: EdgeKind::Governs,
        from: QualityRule,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Quality,
        description: "norm measured against behavior",
    },
    EdgeKindSpec {
        kind: EdgeKind::Targets,
        from: Hypothesis,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Analyzer,
        description: "hypothesis concerns intent",
    },
    EdgeKindSpec {
        kind: EdgeKind::Flags,
        from: Finding,
        to: CodeFile,
        truth_classes: &[Derived],
        owner: OwnerRole::Sync,
        description: "finding concerns codefile",
    },
    EdgeKindSpec {
        kind: EdgeKind::Assesses,
        from: Finding,
        to: CodeRule,
        truth_classes: &[Derived],
        owner: OwnerRole::Sync,
        description: "finding is occurrence of code rule",
    },
    EdgeKindSpec {
        kind: EdgeKind::Exposes,
        from: InterfaceSurface,
        to: CodeFile,
        // Asserted only: an interface surface is declared by human/LLM judgment.
        // Deriving surfaces from code is not implemented (there is no derived
        // `exposes` producer — M-10); if it returns it must ship a deterministic
        // producer AND widen both this list and the edge uniqueness constraint.
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "code exposes a surface",
    },
    EdgeKindSpec {
        kind: EdgeKind::Calls,
        from: Validation,
        to: InterfaceSurface,
        truth_classes: &[Asserted],
        owner: OwnerRole::Validator,
        description: "proof exercises a surface",
    },
    EdgeKindSpec {
        kind: EdgeKind::Relates,
        from: Intent,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Analyzer,
        description: "manual relationship, kind TBD",
    },
    EdgeKindSpec {
        kind: EdgeKind::Covers,
        from: JourneyCoverage,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "a flow that needs a journey proof covers this intent",
    },
    EdgeKindSpec {
        kind: EdgeKind::Asserts,
        from: JourneyInvariantPoint,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "an internal domain invariant point marks this intent",
    },
];

/// Look up the spec for an edge kind. Infallible by construction once
/// `registry_is_total` passes.
pub fn spec(kind: EdgeKind) -> &'static EdgeKindSpec {
    REGISTRY
        .iter()
        .find(|s| s.kind == kind)
        .expect("edge-kind registry is total (guarded by test)")
}

/// The declared truth class of each node kind — the node-side companion to the
/// edge-kind [`REGISTRY`]. A fact kind's *class membership* is data here, not a
/// hardcoded literal at each write site: `Store::add_node` /
/// `Store::add_derived_node` read this at write time both to stamp the column
/// and to reject the wrong constructor. The mapping may reassign a kind between
/// classes, but the class *semantics* (derived = sync-rebuilt with a
/// deterministic id + sentinel timestamp; asserted = a pinned judgment) live in
/// the constructors, never here.
pub const NODE_TRUTH_CLASSES: &[(NodeType, TruthClass)] = &[
    (Intent, Asserted),
    (CodeFile, Asserted),
    (QualityRule, Asserted),
    (CodeRule, Asserted),
    (Validation, Asserted),
    (Hypothesis, Asserted),
    (Finding, Derived),
    (InterfaceSurface, Asserted),
    (Note, Asserted),
    (InboxItem, Asserted),
    (TaskRecord, Asserted),
    (Proposal, Asserted),
    (JourneyCoverage, Asserted),
    (JourneyInvariantPoint, Asserted),
];

/// The declared truth class of a node kind. Infallible by construction once
/// `node_registry_is_total` passes.
pub fn node_truth_class(node_type: NodeType) -> TruthClass {
    NODE_TRUTH_CLASSES
        .iter()
        .find(|(t, _)| *t == node_type)
        .map(|(_, tc)| *tc)
        .expect("node truth-class mapping is total (guarded by test)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_total() {
        // Every EdgeKind has exactly one registry entry.
        for ek in EdgeKind::ALL {
            let matches: Vec<_> = REGISTRY.iter().filter(|s| s.kind == *ek).collect();
            assert_eq!(
                matches.len(),
                1,
                "edge kind {ek} must have exactly one spec"
            );
        }
        assert_eq!(REGISTRY.len(), EdgeKind::ALL.len());
    }

    #[test]
    fn every_kind_allows_exactly_one_truth_class() {
        // With derived-`exposes` extraction unbuilt, every edge kind resolves to
        // a single truth class, so the `(from,to,kind)` edge uniqueness can never
        // need to hold two classes of the same relationship (H-5).
        for s in REGISTRY {
            assert_eq!(
                s.truth_classes.len(),
                1,
                "{} must allow exactly one truth class",
                s.kind
            );
        }
    }

    #[test]
    fn derived_edges_owned_by_sync() {
        // INV-5 at the registry level: derived-only kinds are sync-owned.
        for s in REGISTRY {
            if s.truth_classes == [TruthClass::Derived] {
                assert_eq!(s.owner, OwnerRole::Sync, "{} is derived-only", s.kind);
            }
        }
    }

    #[test]
    fn node_registry_is_total() {
        // Every NodeType has exactly one declared truth class.
        for nt in NodeType::ALL {
            let matches: Vec<_> = NODE_TRUTH_CLASSES.iter().filter(|(t, _)| t == nt).collect();
            assert_eq!(
                matches.len(),
                1,
                "node kind {nt} must have exactly one truth class"
            );
        }
        assert_eq!(NODE_TRUTH_CLASSES.len(), NodeType::ALL.len());
    }

    #[test]
    fn only_finding_is_derived() {
        // The class split the two node constructors enforce: Finding is the sole
        // sync-rebuilt node kind; everything else is a pinned judgment.
        for (nt, tc) in NODE_TRUTH_CLASSES {
            let expect = if *nt == NodeType::Finding {
                TruthClass::Derived
            } else {
                TruthClass::Asserted
            };
            assert_eq!(*tc, expect, "{nt} truth class");
        }
    }
}
