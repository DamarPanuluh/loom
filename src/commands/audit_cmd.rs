//! `loom audit attest-burst` — seal a typed batch authorization before or
//! during a judgment burst without rewriting fact timestamps. Retrospective
//! closure is refused before any envelope append or fact stamp.

use super::{open, pulse};
use crate::cli::AuditCmd;
use crate::model::Claim;
use crate::Result;
use anyhow::{bail, Context};
use std::path::Path;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: AuditCmd, json: bool) -> Result<()> {
    match cmd {
        AuditCmd::AttestBurst {
            subject,
            claim,
            criterion,
            evidence,
            authority,
            executor,
            routing_class,
            operation,
        } => attest_burst(
            graph,
            BurstAttest {
                subject: &subject,
                claim: &claim,
                criterion: &criterion,
                evidence: &evidence,
                authority: &authority,
                executor: &executor,
                routing_class: routing_class.as_deref(),
                operation: operation.as_deref(),
            },
            json,
        ),
    }
}

/// The parsed CLI inputs of one burst attestation, bundled so the handler's
/// signature stays readable.
struct BurstAttest<'a> {
    subject: &'a str,
    claim: &'a str,
    criterion: &'a str,
    evidence: &'a [String],
    authority: &'a str,
    executor: &'a str,
    routing_class: Option<&'a str>,
    operation: Option<&'a str>,
}

fn attest_burst(graph: Option<&Path>, p: BurstAttest<'_>, json: bool) -> Result<()> {
    let store = open(graph)?;
    let (actor, minute) = parse_burst_subject(p.subject)?;
    let batch_claim = crate::batch_auth::BatchClaim::parse(p.claim)?;
    let fact_claim = Claim::from(batch_claim);
    let bucket = crate::audit::JudgmentBurstBucket::for_key(&store, &actor, &minute, batch_claim)?;
    let Some(bucket) = bucket else {
        bail!(
            "no live judgment burst of ≥{} {} facts for '{}' (found 0)",
            crate::audit::BURST_THRESHOLD,
            p.claim,
            p.subject,
        );
    };
    let subjects = bucket.subjects.clone();
    if subjects.len() < crate::audit::BURST_THRESHOLD {
        bail!(
            "no live judgment burst of ≥{} {} facts for '{}' (found {})",
            crate::audit::BURST_THRESHOLD,
            p.claim,
            p.subject,
            subjects.len()
        );
    }
    let op = p.operation.unwrap_or(match batch_claim {
        crate::batch_auth::BatchClaim::Ratification => "ratify",
        crate::batch_auth::BatchClaim::Adjudication => "verdict",
    });
    let mut envelope = crate::batch_auth::BatchAuthorization::seal(
        batch_claim,
        op,
        subjects.clone(),
        p.authority,
        p.executor,
        p.criterion,
        p.evidence.to_vec(),
    )?;
    if let Some(class) = p.routing_class {
        envelope = envelope.with_routing_class(class);
    }
    envelope = envelope.with_time_bounds(format!("{minute}:00.000Z"), format!("{minute}:59.999Z"));
    // Validate before journaling or stamping so retrospective closure is atomic:
    // an attest command after the final fact refuses with no orphan envelope and
    // no partial batch_id changes.
    let envelope_ts = crate::journal::now_iso();
    crate::batch_auth::validate_cover(
        &store,
        &envelope,
        crate::batch_auth::CoverContext {
            envelope_ts: &envelope_ts,
            envelope_origin: crate::journal::Origin::Local,
            subjects: &subjects,
            claim: batch_claim,
            burst_minute: &minute,
            latest_assertion_millis: bucket.latest_assertion_millis,
        },
    )
    .map_err(|e| {
        anyhow::anyhow!(
            "batch authorization refused: {}. attest-burst must run before or during the batch; it cannot retrospectively close an existing burst",
            e.as_str()
        )
    })?;
    let entry = crate::batch_auth::append_envelope(store.root(), &envelope)?;
    let stamped = store.stamp_batch_ids(&subjects, fact_claim, &entry.id)?;
    // Confirm the burst is closed.
    let still = crate::audit::run(&store)?
        .into_iter()
        .filter(|f| f.kind == "judgment_burst")
        .filter(|f| match &f.subject {
            crate::audit::AuditSubject::Graph(id) => id == p.subject,
            _ => false,
        })
        .count();
    if still > 0 {
        bail!(
            "envelope {} was written and {stamped} fact(s) stamped, but audit still \
             reports the burst — check authority/executor alignment with asserted_by='{actor}'",
            entry.id
        );
    }
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "batch_id": entry.id,
            "subject": p.subject,
            "subjects": subjects.len(),
            "stamped": stamped,
            "subject_digest": envelope.subject_digest,
        }),
        "loom status",
        format!(
            "sealed batch authorization {} over {} subjects",
            entry.id,
            subjects.len()
        ),
    )?;
    Ok(())
}

fn parse_burst_subject(subject: &str) -> Result<(String, String)> {
    let (actor, minute) = subject.rsplit_once('@').with_context(|| {
        format!("burst subject must look like actor@YYYY-MM-DDTHH:MM (got '{subject}')")
    })?;
    let minute = crate::journal::minute_key(minute)
        .or_else(|| crate::journal::minute_key(&format!("{minute}:00.000Z")))
        .with_context(|| {
            format!("burst minute must be ISO or epoch milliseconds (got '{minute}')")
        })?;
    Ok((actor.to_string(), minute))
}
