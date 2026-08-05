//! `loom apply` — one atomic batch of graph mutations.
//!
//! Plane: orchestration. Collapses the many single-mutation CLI calls an agent
//! makes per work session (intent add ×N, edge implement ×N, edge relate, edge
//! verdict ×N, finding verdict ×N, vocab add ×N, intent tag ×N) into ONE
//! transaction, so maintaining the graph costs one call, not N — the churn the
//! whole exercise is aimed at.
//!
//! Every mutation goes through the SAME write boundary the individual commands
//! use: the intent gates in `create_intent`, the edge-kind registry and lane
//! gate in `add_edge`, the evidence gates (INV-4/6) plus the asserted/derived
//! wall (INV-5) in `record_verdict`, and the shared `adjudicate_finding` /
//! `tag_intent` gates for finding verdicts and tags. Nothing here re-implements
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
use anyhow::{anyhow, bail, Context};
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

/// One batch. Every section is optional and applied in dependency order — vocab
/// first, then intents, then groundings/relationships/verdicts/adjudications,
/// and tags last — so a later section can reference an intent or term (by name)
/// that an earlier section in the same batch created.
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
    #[serde(default)]
    adjudications: Vec<AdjudicationSpec>,
    #[serde(default)]
    vocab: Vec<VocabSpec>,
    #[serde(default)]
    tags: Vec<TagSpec>,
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

/// A durable adjudication on a materialized finding (a smell, scan finding, or
/// asserted manual finding), addressed by id or unique id-prefix. Mirrors
/// `loom finding verdict`: the same expanded verdict gate and substantive
/// reason check run in `adjudicate_finding`, so a batch can never accept what
/// the CLI would reject. The finding must already exist.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdjudicationSpec {
    finding: String,
    verdict: String,
    reason: String,
    /// Where the judgment was made from — same contract as `--evidence` on
    /// `loom finding verdict`. Defaulted so open verdicts stay a two-field spec.
    #[serde(default)]
    evidence: String,
}

/// A vocabulary term to register (idempotent), mirroring `loom vocab add`.
/// `why` is the optional term description.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VocabSpec {
    term: String,
    #[serde(default)]
    why: String,
}

/// Tag an intent (by name or id) with registered vocab terms, mirroring
/// `loom intent tag add`. The same gate (term must be registered) runs in
/// `tag_intent`; list the term in a `vocab` entry earlier in the same batch to
/// register and apply it in one call — the "arm the duplicate detector" flow.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TagSpec {
    intent: String,
    terms: Vec<String>,
}

/// What a batch did. Emitted as the JSON result; `intent_ids` hands the agent
/// the ids of intents it just created so a follow-up need not re-resolve them.
#[derive(Debug, Default, Serialize)]
struct ApplyReport {
    intents_added: usize,
    groundings: usize,
    relationships: usize,
    verdicts: usize,
    adjudications: usize,
    vocab: usize,
    tags: usize,
    intent_ids: std::collections::BTreeMap<String, String>,
}

/// Apply an already-parsed batch and return what it did.
///
/// The shared core: the CLI reads a file, the MCP tool receives a JSON object,
/// and both land here — so a batch delivered in-band goes through exactly the
/// same gates, in exactly the same transaction, as one from disk.
pub(crate) fn apply_value(
    graph: Option<&Path>,
    fragment: &serde_json::Value,
) -> Result<serde_json::Value> {
    let spec: ApplyTx = serde_json::from_value(fragment.clone())
        .context("parsing apply batch (keys: intents/groundings/relationships/verdicts/adjudications/vocab/tags)")?;
    let store = open(graph)?;
    let report = {
        let tx = store.begin()?;
        let report = apply_tx(&store, &spec)?;
        tx.commit()?;
        report
    };
    let mut payload = serde_json::to_value(&report)?;
    // The batch is committed and durable. A failure refreshing the tracked
    // export is a stale artifact (a `loom sync` away), never a reason to report
    // the durable write as failed — surface it in the payload instead.
    match crate::travel::refresh_export_if_tracked(&store) {
        Ok(reexported) => payload["reexported"] = serde_json::json!(reexported),
        Err(e) => {
            payload["reexported"] = serde_json::json!(false);
            payload["export_refresh_error"] = serde_json::json!(e.to_string());
        }
    }
    Ok(payload)
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
    // sync): only an already-tracked, now-drifted export is rewritten. The
    // batch is already durable, so a refresh failure is surfaced, not fatal.
    let (reexported, export_error) = match crate::travel::refresh_export_if_tracked(&store) {
        Ok(v) => (v, None),
        Err(e) => (false, Some(e.to_string())),
    };

    let mut payload = serde_json::to_value(&report)?;
    payload["reexported"] = serde_json::json!(reexported);
    if let Some(err) = &export_error {
        payload["export_refresh_error"] = serde_json::json!(err);
    }
    pulse::emit(&store, json, payload, "loom sync", || {
        println!(
            "applied: {} intent(s), {} grounding(s), {} relationship(s), {} verdict(s), {} adjudication(s), {} vocab term(s), {} tag(s)",
            report.intents_added,
            report.groundings,
            report.relationships,
            report.verdicts,
            report.adjudications,
            report.vocab,
            report.tags
        );
        for (name, id) in &report.intent_ids {
            println!("  + intent '{}' [{}]", name, crate::model::short(id));
        }
        if reexported {
            println!(
                "  refreshed {} (portable export kept fresh)",
                crate::GRAPH_EXPORT
            );
        }
        if let Some(err) = &export_error {
            println!(
                "  warning: batch is durable but the tracked export could not be refreshed ({err}) — run `loom export`"
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
    for v in &spec.vocab {
        store
            .add_vocab_term(&v.term, &v.why)
            .with_context(|| format!("vocab term '{}'", v.term))?;
        report.vocab += 1;
    }

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
        let mut update_existing_locator = false;
        if let Some(edge) = &existing {
            let current_role = store.grounding_role(&edge.id)?;
            let current_locator = store.get_facet(&edge.id, TargetKind::Edge, "locator")?;
            let locator_changed = g
                .locator
                .as_deref()
                .is_some_and(|new| current_locator.as_deref() != Some(new));
            if current_role != role
                || (locator_changed && edge.status != crate::model::InspectionStatus::Uninspected)
            {
                bail!(
                    "edge exists for intent '{}' and codefile '{}' as {} [{}] — \
                     use `loom edge set-role {}` / `loom edge set-locator {}` or remove it first",
                    intent.name,
                    codefile.name,
                    current_role,
                    crate::model::short(&edge.id),
                    crate::model::short(&edge.id),
                    crate::model::short(&edge.id),
                );
            }
            update_existing_locator = locator_changed;
        }
        if let Some(loc) = &g.locator {
            if role == GroundingRole::Realizes
                && !crate::runner::grounding_locator_resolves(store.root(), &codefile.name, loc)
            {
                bail!(
                    "locator must resolve to a live symbol in '{}' (no match for '{}'); \
                     use a symbol name, or 'module …' for whole-file scope",
                    codefile.name,
                    loc
                );
            }
        }
        let created = existing.is_none();
        let edge = match existing {
            Some(edge) => edge,
            None => store.add_edge(
                EdgeKind::Implements,
                &intent.id,
                &codefile.id,
                TruthClass::Asserted,
            )?,
        };
        if created || update_existing_locator {
            if let Some(loc) = &g.locator {
                store.set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "locator",
                    loc,
                    TruthClass::Asserted,
                )?;
            }
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
    let adj_batch_id = if spec.adjudications.len() > 1 {
        let mut subjects = Vec::new();
        for a in &spec.adjudications {
            let finding = store
                .resolve_finding(&a.finding)
                .with_context(|| format!("adjudication for finding '{}'", a.finding))?;
            subjects.push(finding.id);
        }
        let digest = crate::batch_auth::subject_digest(&subjects);
        let criterion = format!(
            "apply batch adjudications ({}) — shared sealed set",
            subjects.len()
        );
        let pre = crate::journal::append(
            store.root(),
            "batch_apply",
            &digest,
            serde_json::json!({
                "operation": "adjudicate",
                "subjects": subjects,
                "routing_class": "mechanical_apply",
            }),
        )?;
        let now = crate::journal::now_iso();
        let envelope = crate::batch_auth::BatchAuthorization::seal(
            crate::batch_auth::BatchClaim::Adjudication,
            "verdict",
            subjects,
            "llm",
            "llm",
            criterion,
            vec![format!("journal:{}", pre.id)],
        )?
        .with_command_id(format!("apply-adjudications:{}", spec.adjudications.len()))
        .with_time_bounds(&now, &now)
        .with_routing_class("mechanical_apply");
        let entry = crate::batch_auth::append_envelope(store.root(), &envelope)?;
        Some(entry.id)
    } else {
        None
    };
    for a in &spec.adjudications {
        super::diagnostics_cmd::adjudicate_finding_batch(
            store,
            &a.finding,
            &a.verdict,
            &a.reason,
            &a.evidence,
            adj_batch_id.as_deref(),
        )
        .with_context(|| format!("adjudication for finding '{}'", a.finding))?;
        report.adjudications += 1;
    }
    for t in &spec.tags {
        for term in &t.terms {
            super::intent::tag_intent(store, &t.intent, term)
                .with_context(|| format!("tag '{}' on intent '{}'", term, t.intent))?;
            report.tags += 1;
        }
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
        Some("yaml") | Some("yml") => serde_norway::from_str(&text).with_context(|| {
            format!(
                "parsing apply batch {} (YAML: keys intents/groundings/relationships/verdicts/adjudications/vocab/tags)",
                path.display()
            )
        }),
        _ => serde_json::from_str(&text).with_context(|| {
            format!(
                "parsing apply batch {} (JSON: keys intents/groundings/relationships/verdicts/adjudications/vocab/tags)",
                path.display()
            )
        }),
    }
}
