//! Evidence policy — the honesty economy as portable configuration.
//!
//! Plane: configuration. The knobs for the review-confidence cutoff, bounded
//! adversarial frontier, and human-gate placement travel in the export as
//! portable meta (same as `thresholds` and `layer_order`), so a repo's tuned
//! honesty economy survives clone/import. Absence means the code seed's shipped
//! defaults; a present key is parsed strictly (a typo fails loudly, never
//! silently re-defaults).
//!
//! The remaining leg the intent names, the *acceptable evidence forms*, is already
//! declared per work lane on [`workitem::PromptContract.required_evidence`]
//! (populated in [`crate::workitem::contracts`]), not on `crate::truth` axes —
//! so it needs no constant here. This module carries only the values that were
//! genuinely hardcoded.

use crate::store::Store;
use crate::Result;
use anyhow::bail;
use serde::{Deserialize, Serialize};

/// Meta key carrying the JSON-encoded policy (allowlisted in
/// `store::PORTABLE_META_KEYS`).
pub const EVIDENCE_POLICY_META_KEY: &str = "evidence_policy";

/// Origins that may appear on an Intent's immutable provenance facet.
pub const INTENT_ORIGINS: &[&str] = &["human", "llm", "drive", "import"];

/// The code seed's default review-confidence floor. A verdict recorded strictly
/// below this is not settled truth: it routes to the review queue for a stronger
/// re-inspection instead of standing as fact. This is the value that used to be
/// the hardcoded `REVIEW_CONFIDENCE_FLOOR` constant.
pub const DEFAULT_REVIEW_CONFIDENCE_FLOOR: f64 = 0.7;
pub const DEFAULT_ADVERSARIAL_REVIEW_FRONTIER: usize = 5;
pub const MAX_ADVERSARIAL_REVIEW_FRONTIER: usize = 100;

/// Portable evidence policy: both Review selectors and human-gate placement,
/// read from the graph's config at write time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvidencePolicy {
    /// Verdicts strictly below this confidence route to review rather than
    /// standing as settled truth.
    pub review_confidence_floor: f64,
    /// Number of highest-risk settled edge claims kept in the adversarial
    /// frontier. Selection happens before already-reviewed claims are removed,
    /// so this bounds standing review debt rather than walking the whole graph.
    /// Zero disables adversarial review.
    pub adversarial_review_frontier: usize,
    /// Owner lanes whose work packets carry a human gate: the driver must get
    /// human sign-off before that write. Empty (the default) gates no lane, so
    /// the shipped behavior is unchanged until a repo opts a lane in.
    pub human_gated_roles: Vec<String>,
}

impl Default for EvidencePolicy {
    fn default() -> Self {
        Self {
            review_confidence_floor: DEFAULT_REVIEW_CONFIDENCE_FLOOR,
            adversarial_review_frontier: DEFAULT_ADVERSARIAL_REVIEW_FRONTIER,
            human_gated_roles: Vec::new(),
        }
    }
}

/// Owner lanes a human gate can meaningfully apply to — the lanes `loom next`
/// stamps onto a work packet's `owner_role`.
pub const GATEABLE_ROLES: &[&str] = &["builder", "analyzer", "fixer", "validator", "quality"];

impl EvidencePolicy {
    /// Whether an owner lane is human-gated under this policy.
    pub fn gates_role(&self, role: &str) -> bool {
        self.human_gated_roles.iter().any(|r| r == role)
    }

    /// Reject a policy that would silently break routing: the floor must be a
    /// finite fraction in `[0.0, 1.0]`, and every gated role must name a real
    /// lane. A `-1` or `2` floor, or a typo'd role, fails loudly here.
    pub fn validate(&self) -> Result<()> {
        if !self.review_confidence_floor.is_finite()
            || !(0.0..=1.0).contains(&self.review_confidence_floor)
        {
            bail!(
                "review_confidence_floor must be a finite value in [0.0, 1.0], got {}",
                self.review_confidence_floor
            );
        }
        if self.adversarial_review_frontier > MAX_ADVERSARIAL_REVIEW_FRONTIER {
            bail!(
                "adversarial_review_frontier must be in [0, {}], got {}",
                MAX_ADVERSARIAL_REVIEW_FRONTIER,
                self.adversarial_review_frontier
            );
        }
        for r in &self.human_gated_roles {
            if !GATEABLE_ROLES.contains(&r.as_str()) {
                bail!(
                    "unknown human-gated role '{r}'; valid lanes: {}",
                    GATEABLE_ROLES.join(", ")
                );
            }
        }
        Ok(())
    }
}

/// Read the configured policy; absent meta means the shipped defaults.
pub fn load(store: &Store) -> Result<EvidencePolicy> {
    let policy = match store.get_meta(EVIDENCE_POLICY_META_KEY)? {
        Some(json) => serde_json::from_str(&json)?,
        None => EvidencePolicy::default(),
    };
    policy.validate()?;
    Ok(policy)
}

/// Persist the policy as portable meta.
pub fn save(store: &Store, p: &EvidencePolicy) -> Result<()> {
    p.validate()?;
    store.set_meta(EVIDENCE_POLICY_META_KEY, &serde_json::to_string(p)?)
}

/// Drop the config so the policy reverts to "absent = shipped default" rather
/// than a pinned snapshot of today's values.
pub fn clear(store: &Store) -> Result<()> {
    store.remove_meta(EVIDENCE_POLICY_META_KEY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_config_is_the_seed_default() {
        let p = EvidencePolicy::default();
        assert_eq!(p.review_confidence_floor, DEFAULT_REVIEW_CONFIDENCE_FLOOR);
        assert_eq!(
            p.adversarial_review_frontier,
            DEFAULT_ADVERSARIAL_REVIEW_FRONTIER
        );
        assert!(p.human_gated_roles.is_empty());
        assert!(!p.gates_role("quality"));
    }

    #[test]
    fn roundtrips_through_json() {
        let p = EvidencePolicy {
            review_confidence_floor: 0.85,
            adversarial_review_frontier: 8,
            human_gated_roles: vec!["quality".into()],
        };
        let back: EvidencePolicy =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(p, back);
        assert!(back.gates_role("quality"));
    }

    #[test]
    fn unknown_key_fails_loudly() {
        // deny_unknown_fields: a typo must not silently re-default.
        let bad = r#"{"review_confidence_floor":0.7,"typo":1}"#;
        assert!(serde_json::from_str::<EvidencePolicy>(bad).is_err());
    }

    #[test]
    fn default_and_configured_values_validate() {
        assert!(EvidencePolicy::default().validate().is_ok());
        assert!(EvidencePolicy {
            review_confidence_floor: 0.5,
            adversarial_review_frontier: 0,
            human_gated_roles: vec!["quality".into(), "validator".into()],
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn out_of_range_floor_is_rejected() {
        for bad in [-1.0, 2.0, f64::NAN, f64::INFINITY] {
            let p = EvidencePolicy {
                review_confidence_floor: bad,
                adversarial_review_frontier: 5,
                human_gated_roles: Vec::new(),
            };
            assert!(p.validate().is_err(), "floor {bad} must be rejected");
        }
    }

    #[test]
    fn unknown_gated_role_is_rejected() {
        let p = EvidencePolicy {
            review_confidence_floor: 0.7,
            adversarial_review_frontier: 5,
            human_gated_roles: vec!["wizard".into()],
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn v12_policy_without_frontier_gets_the_v13_default() {
        let legacy = r#"{"review_confidence_floor":0.8,"human_gated_roles":[]}"#;
        let parsed: EvidencePolicy = serde_json::from_str(legacy).unwrap();
        assert_eq!(
            parsed.adversarial_review_frontier,
            DEFAULT_ADVERSARIAL_REVIEW_FRONTIER
        );
    }

    #[test]
    fn frontier_is_bounded_but_zero_can_disable_it() {
        let mut policy = EvidencePolicy {
            adversarial_review_frontier: 0,
            ..EvidencePolicy::default()
        };
        assert!(policy.validate().is_ok());
        policy.adversarial_review_frontier = MAX_ADVERSARIAL_REVIEW_FRONTIER + 1;
        assert!(policy.validate().is_err());
    }
}
