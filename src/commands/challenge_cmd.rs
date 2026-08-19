//! `loom challenge` — thin CLI adapter over the deep Review module.

use super::*;
use crate::cli::ChallengeCmd;
use crate::model::Claim;
use std::str::FromStr;

pub(crate) fn dispatch(graph: Option<&Path>, cmd: ChallengeCmd, json: bool) -> Result<()> {
    match cmd {
        ChallengeCmd::Record {
            edge,
            outcome,
            hypothesis,
            evidence,
            impact,
            confidence,
        } => record(
            graph,
            &edge,
            &outcome,
            &hypothesis,
            &evidence,
            impact.as_deref(),
            confidence,
            json,
        ),
        ChallengeCmd::Show { edge } => show(graph, &edge, json),
        ChallengeCmd::List {
            state,
            limit,
            offset,
        } => list(graph, state.as_deref(), limit, offset, json),
    }
}

#[allow(clippy::too_many_arguments)]
fn record(
    graph: Option<&Path>,
    edge: &str,
    outcome: &str,
    hypothesis: &str,
    evidence: &str,
    impact: Option<&str>,
    confidence: f64,
    json: bool,
) -> Result<()> {
    let store = open(graph)?;
    let outcome = crate::review::ChallengeOutcome::from_str(outcome)?;
    let recorded = crate::review::record(
        &store,
        crate::review::ChallengeAttempt {
            edge,
            outcome,
            hypothesis,
            evidence,
            impact,
            confidence,
        },
    )?;
    let next = if recorded.finding.is_some() {
        "loom next --mode triage"
    } else {
        "loom status"
    };
    let line = format!(
        "challenge [{}] recorded as {}{}",
        crate::model::short(&recorded.challenge.id),
        recorded.challenge.state,
        recorded
            .finding
            .as_ref()
            .map(|finding| format!(
                "; finding [{}] captured for triage",
                crate::model::short(&finding.id)
            ))
            .unwrap_or_default()
    );
    pulse::emit_line(
        &store,
        json,
        serde_json::json!({
            "challenge": recorded.challenge,
            "finding": recorded.finding,
            "independence_warning": recorded.independence_warning,
        }),
        next,
        line,
    )
}

fn show(graph: Option<&Path>, edge: &str, json: bool) -> Result<()> {
    let store = open_read(graph)?;
    let edge = store.resolve_edge(edge)?;
    let view = store
        .fact(
            &crate::store::Subject::Edge(edge.id.clone()),
            Claim::Challenge,
        )?
        .ok_or_else(|| anyhow!("edge '{}' has no recorded challenge", edge.id))?;
    let current = crate::review::current_challenge(&store, &edge.id)?.is_some();
    let warning = {
        let prior = store
            .fact(
                &crate::store::Subject::Edge(edge.id.clone()),
                Claim::Verdict,
            )?
            .and_then(|fact| fact.fact.asserted_profile);
        match (prior.as_deref(), view.fact.asserted_profile.as_deref()) {
            (Some(a), Some(b)) if a == b => Some("challenge_same_profile"),
            (Some(_), Some(_)) => None,
            _ => Some("challenge_profile_unknown"),
        }
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "edge": edge,
                "challenge": view.fact,
                "evidence": view.evidence,
                "current": current,
                "independence_warning": warning,
            }))?
        );
    } else {
        println!(
            "challenge [{}] {}  current={}  confidence={:.2}",
            crate::model::short(&view.fact.id),
            view.fact.state,
            current,
            view.fact.confidence
        );
        println!("  hypothesis: {}", view.fact.criterion);
        if let Some(warning) = warning {
            println!("  warning: {warning} (non-blocking)");
        }
    }
    Ok(())
}

fn list(
    graph: Option<&Path>,
    state: Option<&str>,
    limit: usize,
    offset: usize,
    json: bool,
) -> Result<()> {
    if let Some(state) = state {
        crate::review::ChallengeOutcome::from_str(state)?;
    }
    let store = open_read(graph)?;
    let mut rows = Vec::new();
    for fact in store
        .all_facts()?
        .into_iter()
        .filter(|fact| fact.claim == Claim::Challenge)
        .filter(|fact| state.map(|wanted| fact.state == wanted).unwrap_or(true))
        .skip(offset)
        .take(limit)
    {
        let current = crate::review::current_challenge(&store, &fact.subject_id)?.is_some();
        rows.push(serde_json::json!({
            "challenge": fact,
            "current": current,
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if rows.is_empty() {
        println!("no challenges");
    } else {
        for row in rows {
            let fact = &row["challenge"];
            println!(
                "[{}] {:<14} edge={} current={}",
                crate::model::short(fact["id"].as_str().unwrap_or("")),
                fact["state"].as_str().unwrap_or(""),
                crate::model::short(fact["subject_id"].as_str().unwrap_or("")),
                row["current"].as_bool().unwrap_or(false),
            );
        }
    }
    Ok(())
}
