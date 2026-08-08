//! `loom audit attest-burst` — seal a typed batch authorization before or
//! during a judgment burst without rewriting fact timestamps. Retrospective
//! closure is accepted ONLY for a human authority vouching over a trusted,
//! digest-bound `batch_intent` record that predates the burst's final fact and
//! carries a recorded HumanDecision for the exact subject set; anything else
//! is refused before any envelope append or fact stamp.

use super::{open, pulse};
use crate::cli::{AuditCmd, AuditIncidentCmd};
use crate::model::Claim;
use crate::Result;
use anyhow::{bail, Context};
use std::path::Path;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: AuditCmd, json: bool) -> Result<()> {
    match cmd {
        AuditCmd::Incident { cmd } => incident(graph, cmd, json),
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

fn incident(graph: Option<&Path>, cmd: AuditIncidentCmd, json: bool) -> Result<()> {
    match cmd {
        AuditIncidentCmd::Accept {
            subject,
            claim,
            reason,
            human_decision,
        } => accept_incident(graph, &subject, &claim, &reason, human_decision, json),
        AuditIncidentCmd::List => list_incidents(graph, None, None, json),
        AuditIncidentCmd::Show { subject, claim } => {
            list_incidents(graph, Some(&subject), Some(&claim), json)
        }
    }
}

fn accept_incident(
    graph: Option<&Path>,
    subject: &str,
    claim: &str,
    reason: &str,
    human_decision: Option<String>,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let (actor, minute) = parse_burst_subject(subject)?;
    let claim = crate::batch_auth::BatchClaim::parse(claim)?;
    let Some(bucket) = crate::audit::JudgmentBurstBucket::for_key(&store, &actor, &minute, claim)?
    else {
        bail!(
            "no live judgment burst of ≥{} {} facts for '{}' (found 0)",
            crate::audit::BURST_THRESHOLD,
            claim.as_str(),
            subject
        );
    };
    if bucket.subjects.len() < crate::audit::BURST_THRESHOLD {
        bail!(
            "no live judgment burst of ≥{} {} facts for '{}' (found {})",
            crate::audit::BURST_THRESHOLD,
            claim.as_str(),
            subject,
            bucket.subjects.len()
        );
    }

    // Exact repeats are a read of the existing human disposition, not a new
    // decision. Imported records remain disclosure only and never satisfy it.
    if let Some((entry, accepted)) =
        crate::audit::incident_entries(&store)?
            .into_iter()
            .find(|(entry, incident)| {
                entry.origin == crate::journal::Origin::Local && incident.matches(&bucket)
            })
    {
        return print_incident_acceptance(&entry, &accepted, true, json);
    }

    if let Some(batch_id) = crate::batch_auth::covering_envelope(
        &store,
        &bucket.subjects,
        bucket.claim,
        &bucket.actor,
        &bucket.minute,
        &bucket.batch_ids,
        bucket.latest_assertion_millis,
    )? {
        bail!(
            "burst '{}' is already covered by batch authorization {}; historical-incident acceptance would misstate it",
            subject,
            batch_id
        );
    }

    let decision =
        super::ratification_decision(&format!("accept audit incident {subject}"), human_decision)?;
    let accepted = crate::audit::AuditIncident::accept(&bucket, reason, decision)?;
    let entry = store.append_journal(
        crate::audit::INCIDENT_EVENT,
        &accepted.incident_digest,
        serde_json::to_value(&accepted)?,
    )?;
    print_incident_acceptance(&entry, &accepted, false, json)
}

fn print_incident_acceptance(
    entry: &crate::journal::Entry,
    accepted: &crate::audit::AuditIncident,
    idempotent: bool,
    json: bool,
) -> Result<()> {
    let value = serde_json::json!({
        "journal_id": entry.id,
        "subject": accepted.subject,
        "claim": accepted.claim,
        "subjects": accepted.subjects.len(),
        "subject_digest": accepted.subject_digest,
        "incident_digest": accepted.incident_digest,
        "disposition": accepted.disposition,
        "reason": accepted.reason,
        "human_decision": accepted.human_decision,
        "idempotent": idempotent,
        "authorization_granted": false,
        "warning": "accepted as documented history; the underlying judgments remain retrospectively unauthorized",
    });
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "accepted historical incident {} ({}, {} judgments){}",
            accepted.subject,
            accepted.claim.as_str(),
            accepted.subjects.len(),
            if idempotent {
                " — already recorded"
            } else {
                ""
            }
        );
        println!("  journal: {}", entry.id);
        println!("  incident digest: {}", accepted.incident_digest);
        println!("  authorization granted: no");
    }
    Ok(())
}

fn list_incidents(
    graph: Option<&Path>,
    subject: Option<&str>,
    claim: Option<&str>,
    json: bool,
) -> Result<()> {
    let store = super::open_read(graph)?;
    let claim = claim
        .map(crate::batch_auth::BatchClaim::parse)
        .transpose()?;
    let mut rows = Vec::new();
    for (entry, incident) in crate::audit::incident_entries(&store)? {
        if subject.is_some_and(|wanted| wanted != incident.subject)
            || claim.is_some_and(|wanted| wanted != incident.claim)
        {
            continue;
        }
        let active = crate::audit::JudgmentBurstBucket::for_key(
            &store,
            &incident.actor,
            &incident.minute,
            incident.claim,
        )?
        .is_some_and(|bucket| incident.matches(&bucket));
        rows.push(serde_json::json!({
            "journal_id": entry.id,
            "recorded_at": entry.ts,
            "executor": entry.actor,
            "executor_profile": entry.profile,
            "origin": entry.origin,
            "subject": incident.subject,
            "claim": incident.claim,
            "subjects": incident.subjects,
            "subject_digest": incident.subject_digest,
            "incident_digest": incident.incident_digest,
            "disposition": incident.disposition,
            "reason": incident.reason,
            "human_decision": incident.human_decision,
            "active": active,
            "suppresses_blocking_audit": active && entry.origin == crate::journal::Origin::Local,
            "authorization_granted": false,
        }));
    }
    if subject.is_some() && rows.is_empty() {
        bail!("no disclosed audit incident matches the requested subject and claim");
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if rows.is_empty() {
        println!("no disclosed historical audit incidents");
    } else {
        for row in &rows {
            println!(
                "{} [{}] — {} (authorization: none)",
                row["subject"].as_str().unwrap_or(""),
                row["claim"].as_str().unwrap_or(""),
                row["disposition"].as_str().unwrap_or("")
            );
            println!("  reason: {}", row["reason"].as_str().unwrap_or(""));
            println!("  journal: {}", row["journal_id"].as_str().unwrap_or(""));
        }
    }
    Ok(())
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
    let entry = crate::batch_auth::append_envelope(&store, &envelope)?;
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
