//! Evidence policy — the honesty economy as portable configuration.
//!
//! Plane: configuration. The two knobs that were hardcoded engine constants —
//! the review-confidence cutoff and where a human gate sits — travel in the
//! export as portable meta (same as `thresholds` and `layer_order`), so a
//! repo's tuned honesty economy survives clone/import. Absence means the code
//! seed's shipped defaults; a present key is parsed strictly (a typo fails
//! loudly, never silently re-defaults).
//!
//! The third leg the intent names, the *acceptable evidence forms*, is already
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

/// Meta key carrying the JSON-encoded ratification-delegation registry
/// (allowlisted in `store::PORTABLE_META_KEYS`).
pub const RATIFY_POLICIES_META_KEY: &str = "ratify_policies";

/// Origins that may appear on an Intent's immutable provenance facet.
pub const INTENT_ORIGINS: &[&str] = &["human", "llm", "drive", "import"];

/// The code seed's default review-confidence floor. A verdict recorded strictly
/// below this is not settled truth: it routes to the review queue for a stronger
/// re-inspection instead of standing as fact. This is the value that used to be
/// the hardcoded `REVIEW_CONFIDENCE_FLOOR` constant.
pub const DEFAULT_REVIEW_CONFIDENCE_FLOOR: f64 = 0.7;

/// Portable evidence policy: the confidence cutoff and human-gate placement,
/// read from the graph's config at write time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvidencePolicy {
    /// Verdicts strictly below this confidence route to review rather than
    /// standing as settled truth.
    pub review_confidence_floor: f64,
    /// Owner lanes whose work packets carry a human gate: the driver must get
    /// human sign-off before that write. Empty (the default) gates no lane, so
    /// the shipped behavior is unchanged until a repo opts a lane in.
    pub human_gated_roles: Vec<String>,
}

impl Default for EvidencePolicy {
    fn default() -> Self {
        Self {
            review_confidence_floor: DEFAULT_REVIEW_CONFIDENCE_FLOOR,
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

// ---- ratification-delegation registry --------------------------------------
//
// A ratify policy is a named delegation: the human declares a scope once
// ("refactor/hardening does not need my approval") and `loom intent ratify
// --by-policy <name>` may ratify under it without a per-intent challenge. The
// record always attributes to `policy:<name>` — never to a human reviewing
// per-intent — and the declaration's `source` must cite the recorded human act
// (a finding id or journal ref) behind the delegation.

/// One named ratification delegation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatifyPolicy {
    /// Policy name, used as `loom intent ratify --by-policy <name>`.
    pub name: String,
    /// The delegated scope, in the human's words.
    pub description: String,
    /// The recorded human act behind the delegation: a finding id or journal ref.
    pub source: String,
    /// Who declared it — always `human` (the recorded act, never an agent).
    pub declared_by: String,
    /// When the delegation was recorded (the source's date, or declaration time).
    pub declared_at: String,
}

/// Read the registry; absent meta means no delegations exist.
pub fn ratify_policies(store: &Store) -> Result<Vec<RatifyPolicy>> {
    match store.get_meta(RATIFY_POLICIES_META_KEY)? {
        Some(json) => Ok(serde_json::from_str(&json)?),
        None => Ok(Vec::new()),
    }
}

/// Resolve one policy by exact name.
pub fn ratify_policy(store: &Store, name: &str) -> Result<RatifyPolicy> {
    ratify_policies(store)?
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no ratify policy named '{name}' — declare it first with `loom policy ratify-add`"
            )
        })
}

/// Declare a delegation. Idempotent by name: a re-declaration with the same
/// name and source changes nothing.
pub fn add_ratify_policy(store: &Store, p: &RatifyPolicy) -> Result<()> {
    let mut all = ratify_policies(store)?;
    if let Some(existing) = all.iter().find(|x| x.name == p.name) {
        if existing.source == p.source && existing.description == p.description {
            return Ok(()); // same delegation, already declared
        }
        bail!(
            "ratify policy '{}' already exists with a different source — retire or rename",
            p.name
        );
    }
    all.push(p.clone());
    store.set_meta(RATIFY_POLICIES_META_KEY, &serde_json::to_string(&all)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_config_is_the_seed_default() {
        let p = EvidencePolicy::default();
        assert_eq!(p.review_confidence_floor, DEFAULT_REVIEW_CONFIDENCE_FLOOR);
        assert!(p.human_gated_roles.is_empty());
        assert!(!p.gates_role("quality"));
    }

    #[test]
    fn roundtrips_through_json() {
        let p = EvidencePolicy {
            review_confidence_floor: 0.85,
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
                human_gated_roles: Vec::new(),
            };
            assert!(p.validate().is_err(), "floor {bad} must be rejected");
        }
    }

    #[test]
    fn unknown_gated_role_is_rejected() {
        let p = EvidencePolicy {
            review_confidence_floor: 0.7,
            human_gated_roles: vec!["wizard".into()],
        };
        assert!(p.validate().is_err());
    }
}
