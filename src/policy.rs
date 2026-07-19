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

/// Meta key carrying ratification policies. Policies are portable, scoped
/// configuration over Intent facets; they are deliberately not graph nodes.
pub const RATIFICATION_POLICIES_META_KEY: &str = "ratification_policies";

/// Origins that may appear on an Intent's immutable provenance facet.
pub const INTENT_ORIGINS: &[&str] = &["human", "llm", "drive", "import"];

/// A human-authored delegation of ratification scope. Empty filters mean
/// "any value" for that facet; supplied values are ORed within a facet and
/// ANDed across facets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatificationPolicy {
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub origins: Vec<String>,
    #[serde(default)]
    pub levels: Vec<String>,
    #[serde(default)]
    pub lifecycles: Vec<String>,
    /// UTC timestamp of the terminal-gated policy write. This is the date
    /// cited in machine-attributed policy ratification evidence.
    pub human_authored_at: String,
}

impl RatificationPolicy {
    pub fn matches(&self, origin: Option<&str>, level: Option<&str>, lifecycle: &str) -> bool {
        self.enabled
            && (self.origins.is_empty()
                || origin.is_some_and(|v| self.origins.iter().any(|x| x == v)))
            && (self.levels.is_empty() || level.is_some_and(|v| self.levels.iter().any(|x| x == v)))
            && (self.lifecycles.is_empty()
                || self.lifecycles.iter().any(|value| value == lifecycle))
    }
}

/// Portable collection of human-authored policy scopes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RatificationPolicies {
    pub policies: Vec<RatificationPolicy>,
}

impl RatificationPolicies {
    pub fn validate(&self) -> Result<()> {
        let mut names = std::collections::BTreeSet::new();
        for policy in &self.policies {
            if policy.name.trim().is_empty() {
                bail!("ratification policy name must not be empty");
            }
            if !names.insert(policy.name.as_str()) {
                bail!("duplicate ratification policy '{}'", policy.name);
            }
            if policy.human_authored_at.trim().is_empty() {
                bail!(
                    "ratification policy '{}' is missing its human-authored timestamp",
                    policy.name
                );
            }
            for origin in &policy.origins {
                if !INTENT_ORIGINS.contains(&origin.as_str()) {
                    bail!(
                        "unknown policy origin '{origin}'; valid origins: {}",
                        INTENT_ORIGINS.join(", ")
                    );
                }
            }
            for level in &policy.levels {
                if !crate::grammar::LEVELS.contains(&level.as_str()) {
                    bail!(
                        "unknown policy level '{level}'; valid levels: {}",
                        crate::grammar::LEVELS.join(", ")
                    );
                }
            }
            for lifecycle in &policy.lifecycles {
                if !crate::grammar::ACTIVE_LIFECYCLES.contains(&lifecycle.as_str()) {
                    bail!(
                        "unknown policy lifecycle '{lifecycle}'; valid active lifecycles: {}",
                        crate::grammar::ACTIVE_LIFECYCLES.join(", ")
                    );
                }
            }
        }
        Ok(())
    }

    pub fn named(&self, name: &str) -> Option<&RatificationPolicy> {
        self.policies.iter().find(|policy| policy.name == name)
    }
}

/// Apply an already-authorized policy batch. The caller supplies the
/// CLI-observed presence proof; this function deliberately does not claim a
/// human reviewed individual intents. Each resulting ratification is visibly
/// machine-attributed to the policy.
pub fn apply_ratification_policy(
    store: &Store,
    policy: &RatificationPolicy,
    presence: &str,
) -> Result<Vec<crate::model::Node>> {
    use crate::model::{NodeType, TargetKind};

    if !policy.enabled {
        bail!("ratification policy '{}' is disabled", policy.name);
    }
    let date = policy
        .human_authored_at
        .split('T')
        .next()
        .unwrap_or(&policy.human_authored_at);
    let evidence = format!("by policy '{}' (human-authored {date})", policy.name);
    let mut ratified = Vec::new();
    for intent in store.list_nodes(Some(NodeType::Intent), usize::MAX)? {
        if intent.status == "deprecated"
            || store
                .get_facet(&intent.id, TargetKind::Node, "ratification")?
                .as_deref()
                == Some("ratified")
        {
            continue;
        }
        let origin = store.get_facet(&intent.id, TargetKind::Node, "origin")?;
        let level = store.get_facet(&intent.id, TargetKind::Node, "level")?;
        if policy.matches(origin.as_deref(), level.as_deref(), &intent.status) {
            store.ratify_intent_by_policy(&intent.id, &evidence, presence, &policy.name)?;
            ratified.push(intent);
        }
    }
    Ok(ratified)
}

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

/// Read the portable ratification-policy collection; absent configuration means
/// no delegated scope exists.
pub fn load_ratification_policies(store: &Store) -> Result<RatificationPolicies> {
    let policies = match store.get_meta(RATIFICATION_POLICIES_META_KEY)? {
        Some(json) => serde_json::from_str(&json)?,
        None => RatificationPolicies::default(),
    };
    policies.validate()?;
    Ok(policies)
}

/// Persist the complete portable ratification-policy collection.
pub fn save_ratification_policies(store: &Store, policies: &RatificationPolicies) -> Result<()> {
    policies.validate()?;
    store.set_meta(
        RATIFICATION_POLICIES_META_KEY,
        &serde_json::to_string(policies)?,
    )
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
