use crate::model::{Node, NodeType, TargetKind};
use crate::store::Store;
use crate::Result;
use serde::{Deserialize, Serialize};

/// A code finding plus its durable adjudication state.
#[derive(Debug, Clone, Serialize)]
pub struct FindingView {
    pub node: Node,
    pub state: String,
    pub reason: String,
    pub stale: bool,
}

/// A finding's judgment, reassembled from the fact (verdict + reason) and the
/// derived stamp (what the world looked like when it was judged). Split so the
/// judgment travels through the write boundary while its bookkeeping stays
/// derived — a stamp cannot be used to forge a verdict.
struct Adjudication {
    verdict: String,
    reason: String,
    hash: String,
    /// Metric observed when the verdict was recorded (loc, complexity, …).
    /// Absent on pre-banding adjudications — those fall back to hash-only stale.
    metric: Option<u64>,
}

#[derive(Deserialize, Default)]
struct AdjudicationStamp {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    metric: Option<u64>,
}

/// Read a finding's adjudication: the fact carries the judgment, the derived
/// stamp carries the staleness band.
fn adjudication(store: &Store, node_id: &str) -> Result<Option<Adjudication>> {
    let Some(view) = store.fact(
        &crate::store::Subject::Node(node_id.to_string()),
        crate::model::Claim::Adjudication,
    )?
    else {
        return Ok(None);
    };
    let stamp: AdjudicationStamp = store
        .get_facet(node_id, TargetKind::Node, "adjudication_stamp")?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    Ok(Some(Adjudication {
        verdict: view.fact.state,
        reason: view.fact.criterion,
        hash: stamp.hash,
        metric: stamp.metric,
    }))
}

/// Resolving adjudications (`justified`/`rejected`/`deferred`/`duplicate`/`resolved`) stay
/// settled across content-hash churn unless the finding's metric worsens by more
/// than this relative band (or absolute floor). `needed`/`blocked` still reopen
/// on any hash change — those are open work.
const RESOLVING_METRIC_BAND: f64 = 0.10;
const RESOLVING_METRIC_FLOOR: u64 = 50;

fn resolving_verdict(verdict: &str) -> bool {
    matches!(
        verdict,
        "justified" | "rejected" | "deferred" | "duplicate" | "resolved"
    )
}

/// Whether a resolving adjudication should reopen: metric grew past the band,
/// or (legacy) hash changed with no stamped metric.
fn resolving_is_stale(
    adj: &Adjudication,
    current_hash: Option<&String>,
    current_metric: Option<u64>,
) -> bool {
    let hash_changed = !adj.hash.is_empty() && current_hash != Some(&adj.hash);
    if !hash_changed {
        return false;
    }
    match (adj.metric, current_metric) {
        (Some(recorded), Some(now)) => {
            let floor =
                RESOLVING_METRIC_FLOOR.max((recorded as f64 * RESOLVING_METRIC_BAND).ceil() as u64);
            now > recorded.saturating_add(floor)
        }
        // Legacy adjudications without a metric: keep hash-only stale (safe).
        _ => true,
    }
}

/// The deterministic Finding det_key for a smell identity. `sync` materializes
/// smell findings under this key; `loom smells` joins live smells against
/// durable adjudications through it.
pub fn smell_det_key(identity: &str) -> String {
    format!("smell:{identity}")
}

/// Durable adjudication `(verdict, reason)` recorded for a node id, if any.
/// Reads the asserted `adjudication` facet directly, so it also resolves for
/// ids whose derived node has not been rebuilt yet.
pub fn adjudication_of(store: &Store, node_id: &str) -> Result<Option<(String, String)>> {
    let Some(adj) = adjudication(store, node_id)? else {
        return Ok(None);
    };
    if !matches!(
        adj.verdict.as_str(),
        "needed" | "justified" | "rejected" | "deferred" | "blocked" | "duplicate" | "resolved"
    ) {
        return Ok(None);
    }
    Ok(Some((adj.verdict, adj.reason)))
}

/// Whether a live smell carries a durable resolving adjudication — an outcome
/// that no longer counts as open. `needed`/`blocked` remain open work, as does
/// an untriaged smell.
pub fn smell_has_resolving_adjudication(store: &Store, identity: &str) -> Result<bool> {
    let id = Store::derived_node_id(NodeType::Finding, &smell_det_key(identity));
    Ok(matches!(
        adjudication_of(store, &id)?,
        Some((v, _)) if matches!(v.as_str(), "justified" | "rejected" | "deferred" | "duplicate" | "resolved")
    ))
}

// ---- findings (derived flags + durable adjudication) ------------------------

pub fn findings_view(store: &Store) -> Result<Vec<FindingView>> {
    let mut out = Vec::new();
    for node in store.list_nodes(Some(NodeType::Finding), usize::MAX)? {
        let Some(adj) = adjudication(store, &node.id)? else {
            out.push(FindingView {
                node,
                state: "untriaged".into(),
                reason: String::new(),
                stale: false,
            });
            continue;
        };
        if !matches!(
            adj.verdict.as_str(),
            "needed" | "justified" | "rejected" | "deferred" | "blocked" | "duplicate" | "resolved"
        ) {
            out.push(FindingView {
                node,
                state: "untriaged".into(),
                reason: String::new(),
                stale: false,
            });
            continue;
        }
        let current_hash = store.finding_codefile_hash(&node.id)?;
        let current_metric = store.finding_metric(&node.id)?;
        let stale = if resolving_verdict(&adj.verdict) {
            resolving_is_stale(&adj, current_hash.as_ref(), current_metric)
        } else {
            // Open work (needed/blocked): any codefile edit reopens triage.
            !adj.hash.is_empty() && current_hash.as_ref() != Some(&adj.hash)
        };
        out.push(FindingView {
            node,
            state: adj.verdict,
            reason: adj.reason,
            stale,
        });
    }
    out.sort_by(|a, b| a.state.cmp(&b.state).then(a.node.name.cmp(&b.node.name)));
    Ok(out)
}

pub fn untriaged_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.state == "untriaged")
        .collect())
}

/// Findings adjudicated `needed` whose judgment still stands (not stale).
/// Routing splits these by named-repair owner: code edits go to fix,
/// proof-reruns to validate, undeclared coupling to analyze. Stale `needed`
/// findings are excluded — the file changed, so they are back in triage for
/// re-adjudication, and one finding must never sit in two queues.
pub fn needed_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.state == "needed" && !fv.stale)
        .collect())
}

pub fn stale_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.stale)
        .collect())
}

pub fn triage_findings(store: &Store) -> Result<Vec<FindingView>> {
    Ok(findings_view(store)?
        .into_iter()
        .filter(|fv| fv.state == "untriaged" || fv.stale)
        .collect())
}
