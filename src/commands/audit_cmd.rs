//! `loom audit attest-burst` — seal a typed batch authorization before or
//! during a judgment burst without rewriting fact timestamps. Retrospective
//! closure is accepted ONLY for a human authority vouching over a trusted,
//! digest-bound `batch_intent` record that predates the burst's final fact and
//! carries a recorded HumanDecision for the exact subject set; anything else
//! is refused before any envelope append or fact stamp.

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
            human_decision,
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
                human_decision: human_decision.as_deref(),
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
    human_decision: Option<&'a str>,
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
    // The seal itself is a human authority act — the same gate as ratification.
    // A machine cannot pass it: with a host answer it is mediated recording;
    // without one the interactive typed challenge demands a real person at the
    // terminal. This is what makes a forged `batch_intent` journal record
    // useless: the seal is the human-gated act that vouches for the burst.
    let decision =
        super::ratification_decision("attest-burst", p.human_decision.map(String::from))?;
    let mut envelope = crate::batch_auth::BatchAuthorization::seal(
        batch_claim,
        op,
        subjects.clone(),
        p.authority,
        p.executor,
        p.criterion,
        p.evidence.to_vec(),
    )?
    .with_human_decision(decision);
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
            "batch authorization refused: {}. A seal written after the final fact is accepted only when a human vouches and the evidence is a trusted digest-bound `batch_intent` record — written by the human-gated batch path with a recorded HumanDecision for this exact subject set, predating the burst (`loom intent ratify --all`); a self-asserted human authority citing an unrelated, machine-written, or forged record is never accepted",
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
