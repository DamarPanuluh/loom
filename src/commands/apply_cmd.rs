//! `loom apply` — one atomic batch of graph mutations.
//!
//! Plane: orchestration. Collapses the many single-mutation CLI calls an agent
//! makes per work session (intent add ×N, edge implement ×N, edge relate, edge
//! verdict ×N) into ONE transaction, so maintaining the graph costs one call,
//! not N — the churn the whole exercise is aimed at.
//!
//! Every mutation goes through the SAME store write boundary the individual
//! commands use: the intent gates in `create_intent`, the edge-kind registry and
//! lane gate in `add_edge`, and the evidence gates (INV-4/6) plus the
//! asserted/derived wall (INV-5) in `record_verdict`. Nothing here re-implements
//! a check, so a batch can never accept what the per-verb command would reject.
//!
//! Atomicity: the whole batch runs inside one `store.begin()` transaction. A
//! single rejected item drops the transaction, rolling every prior mutation in
//! the batch back — the graph never absorbs half a batch (the two-phase-import
//! discipline). Output is emitted only AFTER commit, so a printed result never
//! describes mutations that rolled back.

use super::intent::{create_intent, IntentAddArgs};
use super::{open, pulse, verdict_status};
use crate::model::{EdgeKind, GroundingRole, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::{workitem, Result};
use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use std::path::Path;

fn default_level() -> String {
    "feature".into()
}
fn default_lifecycle() -> String {
    "planned".into()
}
fn default_role() -> String {
    "realizes".into()
}
fn default_confidence() -> f64 {
    0.9
}

/// One batch. Every section is optional and applied in order — intents first,
/// so groundings/relationships/verdicts later in the same batch can reference an
/// intent this batch just created (by name).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyTx {
    #[serde(default)]
    intents: Vec<IntentSpec>,
    #[serde(default)]
    groundings: Vec<GroundingSpec>,
    #[serde(default)]
    relationships: Vec<RelationSpec>,
    #[serde(default)]
    verdicts: Vec<VerdictSpec>,
}

/// Mirrors `loom intent add` (same defaults: level=feature, lifecycle=planned).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentSpec {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_level")]
    level: String,
    #[serde(default = "default_lifecycle")]
    lifecycle: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    aspect: Option<String>,
    #[serde(default)]
    allow_symbol_name: bool,
}

/// An `implements` edge (grounds an intent in a codefile). Idempotent, matching
/// `loom edge call`/`explore`: an existing grounding is reused rather than
/// duplicated. `locator`/`role` are applied only when the edge is newly created
/// (role changes on a settled edge must go through `loom edge set-role`, which
/// ripples); `verdict`, if present, records a verdict on the edge.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GroundingSpec {
    intent: String,
    codefile: String,
    #[serde(default)]
    locator: Option<String>,
    #[serde(default = "default_role")]
    role: String,
    #[serde(default)]
    verdict: Option<VerdictBody>,
}

/// A relationship edge between two intents (`kind`: hierarchy | requires |
/// scenario-of | variant-of | triggers | sequence | relates). Idempotent; an
/// optional `verdict` mirrors `loom edge explore`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationSpec {
    kind: String,
    from: String,
    to: String,
    #[serde(default)]
    verdict: Option<VerdictBody>,
}

/// A verdict on an existing edge, addressed by id or unique id-prefix. Declares
/// its fields directly (not a flattened `VerdictBody`, which `deny_unknown_fields`
/// forbids) so a typo'd key is a clear error, not a silent drop.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerdictSpec {
    edge: String,
    verdict: String,
    #[serde(default)]
    criterion: String,
    #[serde(default)]
    evidence: String,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

/// The verdict payload shared by inline and standalone verdicts. `verdict` is
/// the `ground | issue | independent` verb; the evidence gate is enforced by
/// `record_verdict`, so a placeholder criterion/evidence on a ground/issue is
/// rejected there — and rolls the whole batch back.
#[derive(Debug, Deserialize)]
struct VerdictBody {
    verdict: String,
    #[serde(default)]
    criterion: String,
    #[serde(default)]
    evidence: String,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

/// What a batch did. Emitted as the JSON result; `intent_ids` hands the agent
/// the ids of intents it just created so a follow-up need not re-resolve them.
#[derive(Debug, Default, Serialize)]
struct ApplyReport {
    intents_added: usize,
    groundings: usize,
    relationships: usize,
    verdicts: usize,
    intent_ids: std::collections::BTreeMap<String, String>,
}

pub(crate) fn apply(graph: Option<&Path>, file: &Path, json: bool) -> Result<()> {
    let spec = read_apply(file)?;
    let store = open(graph)?;

    // Atomic: build the batch inside one transaction and commit only if every
    // item passed its gate. Any error drops `tx`, rolling the whole batch back.
    let report = {
        let tx = store.begin()?;
        let report = apply_tx(&store, &spec)?;
        tx.commit()?;
        report
    };

    // Keep the committed portable artifact fresh as a byproduct (same rule as
    // sync): only an already-tracked, now-drifted export is rewritten.
    let reexported = crate::travel::refresh_export_if_tracked(&store)?;

    let mut payload = serde_json::to_value(&report)?;
    payload["reexported"] = serde_json::json!(reexported);
    pulse::emit(&store, json, payload, "loom sync", || {
        println!(
            "applied: {} intent(s), {} grounding(s), {} relationship(s), {} verdict(s)",
            report.intents_added, report.groundings, report.relationships, report.verdicts
        );
        for (name, id) in &report.intent_ids {
            println!("  + intent '{}' [{}]", name, &id[..8.min(id.len())]);
        }
        if reexported {
            println!(
                "  refreshed {} (portable export kept fresh)",
                crate::GRAPH_EXPORT
            );
        }
        Ok(())
    })
}

/// Apply every section, in dependency order, against `store` (assumed already
/// inside a transaction). Errors are contextualized with the offending item so a
/// rolled-back batch tells the agent exactly which entry to fix.
fn apply_tx(store: &Store, spec: &ApplyTx) -> Result<ApplyReport> {
    let mut report = ApplyReport::default();

    for i in &spec.intents {
        let args = IntentAddArgs {
            name: i.name.clone(),
            description: i.description.clone(),
            level: i.level.clone(),
            lifecycle: i.lifecycle.clone(),
            visibility: i.visibility.clone(),
            layer: i.layer.clone(),
            aspect: i.aspect.clone(),
            allow_symbol_name: i.allow_symbol_name,
        };
        let node = create_intent(store, &args).with_context(|| format!("intent '{}'", i.name))?;
        report.intent_ids.insert(node.name.clone(), node.id.clone());
        report.intents_added += 1;
    }

    for g in &spec.groundings {
        let intent = store
            .resolve_node(&g.intent, Some(NodeType::Intent))
            .with_context(|| format!("grounding intent '{}'", g.intent))?;
        let codefile = store
            .resolve_node(&g.codefile, Some(NodeType::CodeFile))
            .with_context(|| format!("grounding codefile '{}'", g.codefile))?;
        let role: GroundingRole = g.role.parse().map_err(|_| {
            anyhow!(
                "unknown role '{}' (use realizes|consumes|configures|verifies)",
                g.role
            )
        })?;
        let existing = store
            .edges_with(
                Some(EdgeKind::Implements),
                Some(&intent.id),
                Some(&codefile.id),
            )?
            .into_iter()
            .next();
        let (edge, created) = match existing {
            Some(e) => (e, false),
            None => (
                store.add_edge(
                    EdgeKind::Implements,
                    &intent.id,
                    &codefile.id,
                    TruthClass::Asserted,
                )?,
                true,
            ),
        };
        if let Some(loc) = &g.locator {
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "locator",
                loc,
                TruthClass::Asserted,
            )?;
        }
        if created && role != GroundingRole::Realizes {
            store.set_grounding_role(&edge.id, role)?;
        }
        report.groundings += 1;
        if let Some(v) = &g.verdict {
            record_body(store, &edge.id, v)
                .with_context(|| format!("grounding verdict for '{}'", g.intent))?;
            report.verdicts += 1;
        }
    }

    for r in &spec.relationships {
        let kind = workitem::relationship_kind(&r.kind)
            .ok_or_else(|| anyhow!("unknown relationship kind '{}'", r.kind))?;
        let from = store
            .resolve_node(&r.from, Some(NodeType::Intent))
            .with_context(|| format!("relationship from '{}'", r.from))?;
        let to = store
            .resolve_node(&r.to, Some(NodeType::Intent))
            .with_context(|| format!("relationship to '{}'", r.to))?;
        let existing = store
            .edges_with(Some(kind), Some(&from.id), Some(&to.id))?
            .into_iter()
            .next();
        let edge = match existing {
            Some(e) => e,
            None => store.add_edge(kind, &from.id, &to.id, TruthClass::Asserted)?,
        };
        report.relationships += 1;
        if let Some(v) = &r.verdict {
            record_body(store, &edge.id, v)
                .with_context(|| format!("relationship verdict '{}' → '{}'", r.from, r.to))?;
            report.verdicts += 1;
        }
    }

    for v in &spec.verdicts {
        let edge = store
            .resolve_edge(&v.edge)
            .with_context(|| format!("verdict target '{}'", v.edge))?;
        record_parts(
            store,
            &edge.id,
            &v.verdict,
            &v.criterion,
            &v.evidence,
            v.confidence,
        )
        .with_context(|| format!("verdict on edge '{}'", v.edge))?;
        report.verdicts += 1;
    }

    Ok(report)
}

/// Record one inline verdict (from a grounding/relationship) through the store's
/// evidence-gated boundary.
fn record_body(store: &Store, edge_id: &str, v: &VerdictBody) -> Result<()> {
    record_parts(
        store,
        edge_id,
        &v.verdict,
        &v.criterion,
        &v.evidence,
        v.confidence,
    )
}

/// The single verdict path: `verdict_status` maps the verb, `record_verdict`
/// enforces INV-4/6 (evidence) and INV-5 (asserted-only). Shared by inline and
/// standalone verdicts so both obey identical gates.
fn record_parts(
    store: &Store,
    edge_id: &str,
    verdict: &str,
    criterion: &str,
    evidence: &str,
    confidence: f64,
) -> Result<()> {
    let status = verdict_status(verdict)?;
    // `record_verdict` is the single gate + idempotence point: it validates
    // evidence/confidence, then no-ops if the edge already holds this exact
    // verdict (so a re-applied batch never churns exported timestamps).
    store.record_verdict(edge_id, status, criterion, evidence, confidence, "llm")?;
    Ok(())
}

fn read_apply(path: &Path) -> Result<ApplyTx> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    // Format follows the file extension so error messages point at the right
    // grammar; JSON is the default (the export dialect), YAML for .yaml/.yml.
    match path.extension().and_then(|e| e.to_str()) {
        Some("yaml") | Some("yml") => serde_yaml::from_str(&text).with_context(|| {
            format!(
                "parsing apply batch {} (YAML: keys intents/groundings/relationships/verdicts)",
                path.display()
            )
        }),
        _ => serde_json::from_str(&text).with_context(|| {
            format!(
                "parsing apply batch {} (JSON: keys intents/groundings/relationships/verdicts)",
                path.display()
            )
        }),
    }
}
