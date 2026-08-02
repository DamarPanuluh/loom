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
        kind: EdgeKind::Questions,
        from: Question,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "product question awaiting human answer for an intent",
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
    EdgeKindSpec {
        kind: EdgeKind::Documents,
        from: WikiPage,
        to: Intent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "a wiki page draws on (documents) this intent",
    },
    EdgeKindSpec {
        kind: EdgeKind::DependsOn,
        from: Intent,
        to: UpstreamIntent,
        truth_classes: &[Asserted],
        owner: OwnerRole::Builder,
        description: "local intent depends on upstream (federated) intent",
    },
    EdgeKindSpec {
        kind: EdgeKind::Exemplar,
        from: Pattern,
        to: CodeFile,
        truth_classes: &[Asserted],
        owner: OwnerRole::Analyzer,
        description: "reviewed live code exemplar of a ratified pattern",
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

/// The allowed truth classes of each node kind — the node-side companion to the
/// edge-kind [`REGISTRY`]. Most node kinds are asserted-only. `Finding` is the
/// hard-cut exception: programmatic producers use derived findings, while
/// evidence-backed LLM/tool observations use asserted findings through the same
/// listing and triage path.
pub fn node_allows_truth_class(node_type: NodeType, truth_class: TruthClass) -> bool {
    match node_type {
        NodeType::Finding => matches!(truth_class, TruthClass::Asserted | TruthClass::Derived),
        _ => truth_class == TruthClass::Asserted,
    }
}

/// The default truth class for display/schema policy. Constructors still stamp
/// their explicit class after checking [`node_allows_truth_class`].
pub fn node_default_truth_class(node_type: NodeType) -> TruthClass {
    match node_type {
        NodeType::Finding => TruthClass::Derived,
        _ => TruthClass::Asserted,
    }
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
    fn node_truth_class_policy_is_total() {
        // Every NodeType allows at least one truth class, and the display default
        // is always one of its allowed classes.
        for nt in NodeType::ALL {
            let allowed: Vec<_> = TruthClass::ALL
                .iter()
                .copied()
                .filter(|tc| node_allows_truth_class(*nt, *tc))
                .collect();
            assert!(!allowed.is_empty(), "node kind {nt} must allow truth");
            assert!(
                allowed.contains(&node_default_truth_class(*nt)),
                "node kind {nt} default must be allowed"
            );
        }
    }

    #[test]
    fn only_finding_allows_asserted_and_derived_nodes() {
        for nt in NodeType::ALL {
            let allows_asserted = node_allows_truth_class(*nt, TruthClass::Asserted);
            let allows_derived = node_allows_truth_class(*nt, TruthClass::Derived);
            if *nt == NodeType::Finding {
                assert!(allows_asserted, "Finding accepts asserted observations");
                assert!(allows_derived, "Finding accepts derived producers");
                assert_eq!(node_default_truth_class(*nt), TruthClass::Derived);
            } else {
                assert!(allows_asserted, "{nt} is asserted-only");
                assert!(!allows_derived, "{nt} must not accept derived nodes");
                assert_eq!(node_default_truth_class(*nt), TruthClass::Asserted);
            }
        }
        assert!(!node_allows_truth_class(
            NodeType::Question,
            TruthClass::Derived
        ));
    }
}
