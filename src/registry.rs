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
        description: "ordered step in journey/saga",
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
        // The only kind allowing both: derived when sync extracts it,
        // asserted when declared by human/LLM judgment.
        truth_classes: &[Derived, Asserted],
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
];

/// Look up the spec for an edge kind. Infallible by construction once
/// `registry_is_total` passes.
pub fn spec(kind: EdgeKind) -> &'static EdgeKindSpec {
    REGISTRY
        .iter()
        .find(|s| s.kind == kind)
        .expect("edge-kind registry is total (guarded by test)")
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
    fn only_exposes_allows_both_truth_classes() {
        for s in REGISTRY {
            if s.kind == EdgeKind::Exposes {
                assert_eq!(s.truth_classes.len(), 2);
            } else {
                assert_eq!(
                    s.truth_classes.len(),
                    1,
                    "only `exposes` may allow both truth classes; {} has {}",
                    s.kind,
                    s.truth_classes.len()
                );
            }
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
}
