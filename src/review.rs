//! Review — one deep module for confidence re-inspection and adversarial challenge.
//!
//! Callers ask for the ordered pending candidates or record one challenge.
//! The implementation owns frontier selection, risk ordering, target-revision
//! freshness, reviewer independence, and counterexample routing.

use crate::evidence::Evidence;
use crate::model::{
    Claim, Edge, EdgeKind, InspectionStatus, Node, NodeType, TargetKind, TruthClass,
};
use crate::store::{Assertion, FactView, Store, Subject};
use crate::Result;
use anyhow::bail;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVariant {
    LowConfidence,
    Adversarial,
}

impl ReviewVariant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LowConfidence => "low_confidence",
            Self::Adversarial => "adversarial",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub edge: Edge,
    pub variant: ReviewVariant,
    pub owner_role: String,
    pub reason: String,
    pub target_verdict_fact_id: String,
    pub risk_score: Option<f64>,
    pub prefer_profile_not: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub low_confidence: usize,
    pub adversarial_pending: usize,
    pub inconclusive: usize,
    pub same_profile_warnings: usize,
    pub unknown_profile_warnings: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndependenceWarning {
    pub code: String,
    pub edge_id: String,
    pub challenge_fact_id: String,
    pub detail: String,
    pub blocking: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChallengeOutcome {
    Survived,
    Counterexample,
    Inconclusive,
}

impl ChallengeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Survived => "survived",
            Self::Counterexample => "counterexample",
            Self::Inconclusive => "inconclusive",
        }
    }
}

impl FromStr for ChallengeOutcome {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "survived" => Ok(Self::Survived),
            "counterexample" => Ok(Self::Counterexample),
            "inconclusive" => Ok(Self::Inconclusive),
            other => bail!(
                "unknown challenge outcome '{other}' (use survived|counterexample|inconclusive)"
            ),
        }
    }
}

pub struct ChallengeAttempt<'a> {
    pub edge: &'a str,
    pub outcome: ChallengeOutcome,
    pub hypothesis: &'a str,
    pub evidence: &'a str,
    pub impact: Option<&'a str>,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordedChallenge {
    pub challenge: crate::evidence::Fact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finding: Option<Node>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub independence_warning: Option<String>,
}

/// Record one adversarial attempt through the sole asserted-fact boundary.
/// The target snapshot is minted by `assert_fact`; a counterexample's Finding
/// is created in the same transaction and the challenged Verdict is never
/// projected or rewritten.
pub fn record(store: &Store, attempt: ChallengeAttempt<'_>) -> Result<RecordedChallenge> {
    let edge = store.resolve_edge(attempt.edge)?;
    if crate::model::is_placeholder(attempt.hypothesis) {
        bail!("challenge requires a substantive --hypothesis");
    }
    if crate::model::is_placeholder(attempt.evidence) {
        bail!("challenge requires substantive --evidence describing what was attempted");
    }
    if !(0.0..=1.0).contains(&attempt.confidence) || !attempt.confidence.is_finite() {
        bail!("challenge confidence must be between 0.0 and 1.0");
    }
    if attempt.outcome == ChallengeOutcome::Counterexample
        && attempt
            .impact
            .map(crate::model::is_placeholder)
            .unwrap_or(true)
    {
        bail!("a counterexample requires substantive --impact");
    }

    let evidence_text = match attempt.impact {
        Some(impact) => format!("{}\nimpact: {}", attempt.evidence.trim(), impact.trim()),
        None => attempt.evidence.trim().to_string(),
    };
    let cited = crate::evidence::cite(store.root(), &evidence_text)?;
    if !cited.iter().any(|row| {
        matches!(
            row,
            crate::evidence::CitedEvidence::Span(_) | crate::evidence::CitedEvidence::Journal(_)
        )
    }) {
        bail!(
            "challenge --evidence must include at least one live file:line or journal:id citation"
        );
    }

    if let Some(existing) = current_challenge(store, &edge.id)? {
        let same_prose = existing
            .evidence
            .iter()
            .any(|row| matches!(&row.payload, Evidence::Claim { text } if text == &evidence_text));
        if existing.fact.state == attempt.outcome.as_str()
            && existing.fact.criterion == attempt.hypothesis.trim()
            && existing.fact.confidence.to_bits() == attempt.confidence.to_bits()
            && same_prose
        {
            return Ok(RecordedChallenge {
                finding: existing_finding(store, &existing.fact.id)?,
                independence_warning: independence_warning(store, &edge.id, &existing.fact)?,
                challenge: existing.fact,
            });
        }
        bail!(
            "edge '{}' already has a current challenge for this Verdict revision; it reopens only when that Verdict or its evidence changes",
            edge.id
        );
    }

    let is_pending = pending(store)?.into_iter().any(|candidate| {
        candidate.variant == ReviewVariant::Adversarial && candidate.edge.id == edge.id
    });
    if !is_pending {
        bail!(
            "edge '{}' is not an unchallenged claim in the current adversarial frontier",
            edge.id
        );
    }

    let target_verdict = verdict_fact(store, &edge.id)?
        .ok_or_else(|| anyhow::anyhow!("edge '{}' has no Verdict fact", edge.id))?;
    let identity = store.execution_identity();
    let actor = identity.actor();
    let warning = profile_warning(
        target_verdict.fact.asserted_profile.as_deref(),
        identity.profile(),
    );

    let tx = store.maybe_tx()?;
    let challenge = store.assert_fact(
        Assertion::new(
            Subject::Edge(edge.id.clone()),
            Claim::Challenge,
            attempt.outcome.as_str(),
            &actor,
        )
        .criterion(attempt.hypothesis.trim())
        .confidence(attempt.confidence)
        .cited(cited),
    )?;
    let finding = if attempt.outcome == ChallengeOutcome::Counterexample {
        Some(capture_counterexample(
            store,
            &edge,
            &target_verdict.fact.id,
            &challenge.fact.id,
            &attempt,
            &evidence_text,
        )?)
    } else {
        None
    };
    if let Some(tx) = tx {
        tx.commit()?;
    }

    store.append_journal(
        "challenge_recorded",
        &challenge.fact.id,
        serde_json::json!({
            "edge_id": edge.id,
            "target_verdict_fact_id": target_verdict.fact.id,
            "outcome": attempt.outcome,
            "hypothesis": attempt.hypothesis,
            "finding_id": finding.as_ref().map(|node| node.id.as_str()),
            "independence_warning": warning,
        }),
    )?;
    if let Some(code) = warning {
        store.append_journal(
            code,
            &challenge.fact.id,
            serde_json::json!({
                "edge_id": edge.id,
                "prior_profile": target_verdict.fact.asserted_profile,
                "reviewer_profile": identity.profile(),
                "blocking": false,
            }),
        )?;
    }

    Ok(RecordedChallenge {
        challenge: challenge.fact,
        finding,
        independence_warning: warning.map(str::to_string),
    })
}

/// The complete Review roster in serving order. Low-confidence re-inspection
/// remains first; adversarial work is the unclosed portion of the fixed risk
/// frontier behind it.
pub fn pending(store: &Store) -> Result<Vec<Candidate>> {
    let policy = crate::policy::load(store)?;
    let mut green = store.live_edges_by_status(
        TruthClass::Asserted,
        &[InspectionStatus::Passing, InspectionStatus::Independent],
    )?;

    let mut low = Vec::new();
    for edge in &green {
        if edge.confidence <= 0.0 || edge.confidence >= policy.review_confidence_floor {
            continue;
        }
        let Some(verdict) = verdict_fact(store, &edge.id)? else {
            continue;
        };
        low.push(Candidate {
            edge: edge.clone(),
            variant: ReviewVariant::LowConfidence,
            owner_role: crate::registry::spec(edge.kind).owner.as_str().into(),
            reason: format!(
                "verdict recorded with confidence {:.2} (< {}) — re-inspect independently",
                edge.confidence, policy.review_confidence_floor
            ),
            target_verdict_fact_id: verdict.fact.id,
            risk_score: None,
            prefer_profile_not: verdict.fact.asserted_profile,
        });
    }
    low.sort_by(|a, b| {
        a.edge
            .confidence
            .partial_cmp(&b.edge.confidence)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.edge.id.cmp(&b.edge.id))
    });

    if policy.adversarial_review_frontier == 0 {
        return Ok(low);
    }

    let risk: BTreeMap<String, f64> = crate::risk::rank(store)?
        .into_iter()
        .map(|candidate| (candidate.intent_id, candidate.score))
        .collect();
    let mut ranked = Vec::new();
    for edge in green.drain(..) {
        if edge.confidence < policy.review_confidence_floor {
            continue;
        }
        let Some(verdict) = verdict_fact(store, &edge.id)? else {
            continue;
        };
        if !verdict.fact.verification.counts() {
            continue;
        }
        let intents = adjacent_live_intents(store, &edge)?;
        if intents.is_empty() {
            continue;
        }
        let score = intents
            .iter()
            .filter_map(|intent| risk.get(&intent.id).copied())
            .fold(0.0_f64, f64::max);
        let critical = is_critical(store, &edge, &intents)?;
        ranked.push((critical, score, verdict, edge));
    }
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal))
            .then_with(|| {
                a.2.fact
                    .verification
                    .rank()
                    .cmp(&b.2.fact.verification.rank())
            })
            .then_with(|| {
                a.3.confidence
                    .partial_cmp(&b.3.confidence)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| a.3.id.cmp(&b.3.id))
    });
    ranked.truncate(policy.adversarial_review_frontier);

    for (critical, score, verdict, edge) in ranked {
        if current_challenge(store, &edge.id)?.is_some() {
            continue;
        }
        low.push(Candidate {
            reason: format!(
                "{}risk frontier claim (score {:.4}) has no adversarial attempt against its current verdict",
                if critical { "critical " } else { "" },
                score
            ),
            edge,
            variant: ReviewVariant::Adversarial,
            owner_role: "analyzer".into(),
            target_verdict_fact_id: verdict.fact.id,
            risk_score: Some(score),
            prefer_profile_not: verdict.fact.asserted_profile,
        });
    }
    Ok(low)
}

pub fn summary(store: &Store) -> Result<Summary> {
    let pending = pending(store)?;
    let mut summary = Summary {
        low_confidence: pending
            .iter()
            .filter(|candidate| candidate.variant == ReviewVariant::LowConfidence)
            .count(),
        adversarial_pending: pending
            .iter()
            .filter(|candidate| candidate.variant == ReviewVariant::Adversarial)
            .count(),
        ..Summary::default()
    };
    for fact in store
        .all_facts()?
        .into_iter()
        .filter(|fact| fact.claim == Claim::Challenge)
    {
        let Some(challenge) = store.fact_by_id(&fact.id)? else {
            continue;
        };
        if !snapshot_holds(&challenge) {
            continue;
        }
        if fact.state == "inconclusive" {
            summary.inconclusive += 1;
        }
        let prior_profile =
            verdict_fact(store, &fact.subject_id)?.and_then(|view| view.fact.asserted_profile);
        match (prior_profile.as_deref(), fact.asserted_profile.as_deref()) {
            (Some(prior), Some(reviewer)) if prior == reviewer => {
                summary.same_profile_warnings += 1
            }
            (Some(_), Some(_)) => {}
            _ => summary.unknown_profile_warnings += 1,
        }
    }
    Ok(summary)
}

/// Reviewer-independence warnings are deliberately outside `audit::backlog`:
/// they are visible in `loom audit` and status but never create Audit-lane debt.
pub fn independence_warnings(store: &Store) -> Result<Vec<IndependenceWarning>> {
    let mut warnings = Vec::new();
    for fact in store
        .all_facts()?
        .into_iter()
        .filter(|fact| fact.claim == Claim::Challenge)
    {
        let Some(challenge) = store.fact_by_id(&fact.id)? else {
            continue;
        };
        if !snapshot_holds(&challenge) {
            continue;
        }
        let prior =
            verdict_fact(store, &fact.subject_id)?.and_then(|view| view.fact.asserted_profile);
        let Some(code) = profile_warning(prior.as_deref(), fact.asserted_profile.as_deref()) else {
            continue;
        };
        let detail = match code {
            "challenge_same_profile" => format!(
                "edge {} was challenged by the same executor profile '{}' that asserted its Verdict",
                fact.subject_id,
                fact.asserted_profile.as_deref().unwrap_or("")
            ),
            _ => format!(
                "edge {} challenge independence cannot be established because a Verdict or reviewer profile is missing",
                fact.subject_id
            ),
        };
        warnings.push(IndependenceWarning {
            code: code.into(),
            edge_id: fact.subject_id,
            challenge_fact_id: fact.id,
            detail,
            blocking: false,
        });
    }
    warnings.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
    Ok(warnings)
}

pub fn current_challenge(store: &Store, edge_id: &str) -> Result<Option<FactView>> {
    let view = store.fact(&Subject::Edge(edge_id.to_string()), Claim::Challenge)?;
    Ok(view.filter(snapshot_holds))
}

fn verdict_fact(store: &Store, edge_id: &str) -> Result<Option<FactView>> {
    store.fact(&Subject::Edge(edge_id.to_string()), Claim::Verdict)
}

fn snapshot_holds(view: &FactView) -> bool {
    view.fact.verification.counts()
        && view
            .evidence
            .iter()
            .any(|row| row.holds && matches!(row.payload, Evidence::FactSnapshot { .. }))
}

fn adjacent_live_intents(store: &Store, edge: &Edge) -> Result<Vec<Node>> {
    let mut intents = Vec::new();
    for id in [&edge.from_id, &edge.to_id] {
        if let Some(node) = store.get_node(id)? {
            if node.node_type == NodeType::Intent && node.status != "deprecated" {
                intents.push(node);
            }
        }
    }
    Ok(intents)
}

fn is_critical(store: &Store, edge: &Edge, intents: &[Node]) -> Result<bool> {
    if edge.kind == EdgeKind::Governs
        && store
            .get_node(&edge.from_id)?
            .and_then(|node| {
                node.body
                    .get("severity")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .as_deref()
            == Some("error")
    {
        return Ok(true);
    }
    for intent in intents {
        if matches!(
            store
                .get_facet(&intent.id, TargetKind::Node, "level")?
                .as_deref(),
            Some("system" | "cross_cutting")
        ) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn profile_warning(prior: Option<&str>, reviewer: Option<&str>) -> Option<&'static str> {
    match (prior, reviewer) {
        (Some(prior), Some(reviewer)) if prior == reviewer => Some("challenge_same_profile"),
        (Some(_), Some(_)) => None,
        _ => Some("challenge_profile_unknown"),
    }
}

fn independence_warning(
    store: &Store,
    edge_id: &str,
    challenge: &crate::evidence::Fact,
) -> Result<Option<String>> {
    let prior = verdict_fact(store, edge_id)?.and_then(|view| view.fact.asserted_profile);
    Ok(
        profile_warning(prior.as_deref(), challenge.asserted_profile.as_deref())
            .map(str::to_string),
    )
}

fn capture_counterexample(
    store: &Store,
    edge: &Edge,
    target_verdict_fact_id: &str,
    challenge_fact_id: &str,
    attempt: &ChallengeAttempt<'_>,
    evidence: &str,
) -> Result<Node> {
    let hypothesis = attempt.hypothesis.trim();
    let impact = attempt.impact.unwrap_or_default().trim();
    let body = serde_json::json!({
        "kind": "code_audit",
        "source": "adversarial_review",
        "evidence": evidence,
        "impact": impact,
        "confidence": attempt.confidence,
        "link": edge.id,
        "challenged_edge_id": edge.id,
        "target_verdict_fact_id": target_verdict_fact_id,
        "challenge_fact_id": challenge_fact_id,
        "hypothesis": hypothesis,
    });
    store.add_node(
        NodeType::Finding,
        &format!("Counterexample to {} claim: {}", edge.kind, hypothesis),
        impact,
        "code_audit",
        body,
    )
}

fn existing_finding(store: &Store, challenge_fact_id: &str) -> Result<Option<Node>> {
    Ok(store
        .list_nodes(Some(NodeType::Finding), usize::MAX)?
        .into_iter()
        .find(|node| {
            node.body
                .get("challenge_fact_id")
                .and_then(|value| value.as_str())
                == Some(challenge_fact_id)
        }))
}
