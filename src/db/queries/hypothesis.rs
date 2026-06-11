//! Hypothesis node queries — the pre-decision plane. The TARGETS *edge* lives
//! in `targets.rs`.

use anyhow::Result;
use grafeo::Value;
use std::collections::HashMap;

use crate::db::schema::{esc, label, prop};
use crate::db::LoomDb;
use crate::types::Hypothesis;

use super::row::{col_map, get, str_val};

const SELECT: &str = "h.id, h.name, h.claim, h.proposal, h.predicted_outcome, \
                      h.status, h.author, h.evidence, h.inspected_by, \
                      h.last_inspected, h.created_at, h.updated_at";

pub fn insert_hypothesis(db: &dyn LoomDb, h: &Hypothesis) -> Result<()> {
    // Param-bound: claim/proposal/outcome/evidence are agent-written prose.
    let q = format!(
        "INSERT (:{lbl} {{{id}: $id, {name}: $name, {claim}: $claim, {proposal}: $proposal, \
         {outcome}: $outcome, {status}: $status, {author}: $author, {evidence}: $evidence, \
         {by}: $by, {last}: $last, {created}: $created, {updated}: $updated}})",
        lbl = label::HYPOTHESIS,
        id = prop::ID,
        name = prop::NAME,
        claim = prop::CLAIM,
        proposal = prop::PROPOSAL,
        outcome = prop::PREDICTED_OUTCOME,
        status = prop::STATUS,
        author = prop::AUTHOR,
        evidence = prop::EVIDENCE,
        by = prop::INSPECTED_BY,
        last = prop::LAST_INSPECTED,
        created = prop::CREATED_AT,
        updated = prop::UPDATED_AT,
    );
    db.execute_with_params(&q, super::row::sparams(&[
        ("id", &h.id), ("name", &h.name), ("claim", &h.claim),
        ("proposal", &h.proposal), ("outcome", &h.predicted_outcome),
        ("status", &h.status), ("author", &h.author), ("evidence", &h.evidence),
        ("by", &h.inspected_by), ("last", &h.last_inspected),
        ("created", &h.created_at), ("updated", &h.updated_at),
    ]))?;
    Ok(())
}

pub fn get_hypothesis(db: &dyn LoomDb, id: &str) -> Result<Option<Hypothesis>> {
    let q = format!(
        "MATCH (h:{lbl} {{id: '{id}'}}) RETURN {SELECT}",
        lbl = label::HYPOTHESIS,
        id = esc(id),
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    Ok(result.rows().first().map(|row| row_to_hypothesis(row, &cols)))
}

/// Resolve a hypothesis key (exact id, exact name, or unique name fragment) —
/// mirrors `resolve_intent`/`resolve_validation` for consistent UX.
pub fn resolve_hypothesis(db: &dyn LoomDb, key: &str) -> Result<String> {
    let hs = list_hypotheses(db, None)?;
    if hs.iter().any(|h| h.id == key) {
        return Ok(key.to_string());
    }
    let kl = key.to_lowercase();
    let exact: Vec<_> = hs.iter().filter(|h| h.name.to_lowercase() == kl).collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    let subs: Vec<_> = hs.iter().filter(|h| h.name.to_lowercase().contains(&kl)).collect();
    match subs.len() {
        1 => Ok(subs[0].id.clone()),
        0 => anyhow::bail!(
            "No hypothesis matches '{}' (by id, name, or fragment). Run `loom hypothesis list`.",
            key
        ),
        _ => anyhow::bail!(
            "'{}' is ambiguous — matches {} hypotheses. Use the id (`loom hypothesis list`).",
            key, subs.len()
        ),
    }
}

/// All hypotheses, optionally filtered by status (in Rust — node property
/// inline-matching is fine, but the status vocabulary check belongs here).
pub fn list_hypotheses(db: &dyn LoomDb, status: Option<&str>) -> Result<Vec<Hypothesis>> {
    let q = format!(
        "MATCH (h:{lbl}) RETURN {SELECT} ORDER BY h.created_at",
        lbl = label::HYPOTHESIS,
    );
    let result = db.execute(&q)?;
    let cols = col_map(&result);
    let mut hs: Vec<Hypothesis> =
        result.rows().iter().map(|row| row_to_hypothesis(row, &cols)).collect();
    if let Some(s) = status {
        hs.retain(|h| h.status == s);
    }
    Ok(hs)
}

/// `loom next --mode triage`: hypotheses needing analyzer attention, ranked by
/// the combined centrality of their target intents — the blast radius of the
/// proposal. Two kinds of item, distinguished by the hypothesis status:
///   proposed  → never proven: prove it.
///   supported → its support went STALE: `loom sync` flipped ≥1 TARGETS edge
///               to needs_reverification because target code changed — the
///               evidence is about old code, re-prove or refute.
/// An untargeted proposal still surfaces (base score 1.0), just last.
/// Optional work like discovery/review: triage never blocks `phase=complete`.
pub fn triage_candidates(db: &dyn LoomDb) -> Result<Vec<(Hypothesis, f64)>> {
    let hs = list_hypotheses(db, None)?;
    if hs.is_empty() {
        return Ok(Vec::new());
    }
    let mut targets_by_h: std::collections::HashMap<String, Vec<crate::types::TargetsEdge>> =
        std::collections::HashMap::new();
    for t in super::targets::list_all_targets(db)? {
        targets_by_h.entry(t.hypothesis_id.clone()).or_default().push(t);
    }
    let degrees = super::scoring::all_intent_degrees(db)?;
    let mut out: Vec<(Hypothesis, f64)> = Vec::new();
    for h in hs {
        let targets = targets_by_h.get(h.id.as_str());
        let due = match h.status.as_str() {
            "proposed" => true,
            "supported" => targets.is_some_and(|ts| {
                ts.iter().any(|t| t.inspection_status == "needs_reverification")
            }),
            _ => false,
        };
        if !due {
            continue;
        }
        let reach: i64 = targets
            .map(|ts| ts.iter().map(|t| degrees.get(&t.intent_id).copied().unwrap_or(0)).sum())
            .unwrap_or(0);
        out.push((h, 1.0 + reach as f64));
    }
    // Highest blast radius first; oldest breaks ties (nothing rots).
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.created_at.cmp(&b.0.created_at))
    });
    Ok(out)
}

/// Record the proof verdict (supported | refuted) with its evidence and
/// provenance. Writes the transition note (the recurrence memory). Returns
/// false when the hypothesis doesn't exist.
pub fn update_hypothesis_verdict(
    db: &dyn LoomDb,
    id: &str,
    status: &str,
    evidence: &str,
    inspected_by: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_hypothesis(db, id)? else {
        return Ok(false);
    };
    db.execute_with_params(
        &format!(
            "MATCH (h:{lbl} {{id: $id}}) \
             SET h.{st} = $status, h.{ev} = $evidence, h.{by} = $by, \
                 h.{last} = $now, h.{upd} = $now",
            lbl = label::HYPOTHESIS,
            st = prop::STATUS,
            ev = prop::EVIDENCE,
            by = prop::INSPECTED_BY,
            last = prop::LAST_INSPECTED,
            upd = prop::UPDATED_AT,
        ),
        super::row::sparams(&[
            ("id", id), ("status", status), ("evidence", evidence),
            ("by", inspected_by), ("now", now),
        ]),
    )?;
    super::note::record_transition(db, "hypothesis", id, &prev.status, status, inspected_by, now)?;
    Ok(true)
}

/// Set the decision status (adopted | rejected) and record the transition.
/// The decision's WHY travels as a separate decision note written by the
/// command layer. Returns false when the hypothesis doesn't exist.
pub fn set_hypothesis_status(
    db: &dyn LoomDb,
    id: &str,
    status: &str,
    author: &str,
    now: &str,
) -> Result<bool> {
    let Some(prev) = get_hypothesis(db, id)? else {
        return Ok(false);
    };
    db.execute(&format!(
        "MATCH (h:{lbl} {{id: '{id}'}}) SET h.{st} = '{status}', h.{upd} = '{now}'",
        lbl = label::HYPOTHESIS,
        id = esc(id),
        st = prop::STATUS,
        upd = prop::UPDATED_AT,
        status = esc(status),
        now = esc(now),
    ))?;
    super::note::record_transition(db, "hypothesis", id, &prev.status, status, author, now)?;
    Ok(true)
}

fn row_to_hypothesis(row: &[Value], cols: &HashMap<&str, usize>) -> Hypothesis {
    Hypothesis {
        id:                str_val(get(row, cols, "h.id")),
        name:              str_val(get(row, cols, "h.name")),
        claim:             str_val(get(row, cols, "h.claim")),
        proposal:          str_val(get(row, cols, "h.proposal")),
        predicted_outcome: str_val(get(row, cols, "h.predicted_outcome")),
        status:            str_val(get(row, cols, "h.status")),
        author:            str_val(get(row, cols, "h.author")),
        evidence:          str_val(get(row, cols, "h.evidence")),
        inspected_by:      str_val(get(row, cols, "h.inspected_by")),
        last_inspected:    str_val(get(row, cols, "h.last_inspected")),
        created_at:        str_val(get(row, cols, "h.created_at")),
        updated_at:        str_val(get(row, cols, "h.updated_at")),
    }
}
