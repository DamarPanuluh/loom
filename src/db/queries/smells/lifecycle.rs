use std::collections::{HashMap, HashSet};

use super::{
    recurrent_teaching, teaching_for, AdjudicatedSmell, Smell, SmellCtx, HYPOTHESIS_BACKLOG_LIMIT,
    HYPOTHESIS_STALE_DAYS,
};

/// Lifecycle plane — recurrent regressions and pre-decision-plane backlog.
pub(super) fn detect_lifecycle_plane(
    ctx: &SmellCtx,
    smells: &mut Vec<Smell>,
    adj: &mut Vec<AdjudicatedSmell>,
) {
    detect_recurrent_trouble(
        ctx.notes,
        ctx.relates,
        ctx.governs,
        &ctx.name_of,
        &ctx.last_decision,
        smells,
        adj,
    );
    detect_hypothesis_accumulation(ctx.proposed_hypotheses, ctx.targets, smells);
}
/// newer than the last regression refutes the finding; a later regression
/// re-flags.
fn detect_recurrent_trouble(
    notes: &[crate::types::Note],
    relates: &[crate::types::RelatesTo],
    governs: &[crate::types::Governs],
    name_of: &HashMap<&str, &str>,
    last_decision: &HashMap<&str, &crate::types::Note>,
    smells: &mut Vec<Smell>,
    adjudicated_out: &mut Vec<AdjudicatedSmell>,
) {
    let mut trouble: HashMap<(String, String), usize> = HashMap::new();
    let mut last_trouble: HashMap<(String, String), String> = HashMap::new();
    let mut trouble_notes: HashMap<(String, String), Vec<&crate::types::Note>> = HashMap::new();
    for n in notes {
        if n.kind == "transition"
            && (n.text.ends_with("→ failing") || n.text.ends_with("→ needs_change"))
        {
            let key = (n.target_kind.clone(), n.target_id.clone());
            *trouble.entry(key.clone()).or_insert(0) += 1;
            trouble_notes.entry(key.clone()).or_default().push(n);
            let e = last_trouble.entry(key).or_default();
            if n.created_at > *e {
                *e = n.created_at.clone();
            }
        }
    }
    let edge_label: HashMap<&str, String> = {
        let mut m: HashMap<&str, String> = HashMap::new();
        for e in relates {
            m.insert(e.id.as_str(), format!("{} × {}", e.from_name, e.to_name));
        }
        for g in governs {
            m.insert(
                g.id.as_str(),
                format!("{} → {}", g.rule_name, g.intent_name),
            );
        }
        m
    };
    for ((kind, id), count) in trouble {
        if count < 2 {
            continue;
        }
        let label = if kind == "intent" {
            name_of.get(id.as_str()).copied().unwrap_or(&id).to_string()
        } else {
            edge_label
                .get(id.as_str())
                .cloned()
                .unwrap_or_else(|| id.clone())
        };
        let last = last_trouble
            .get(&(kind.clone(), id.clone()))
            .map(String::as_str)
            .unwrap_or("");
        let mut recent = trouble_notes
            .get(&(kind.clone(), id.clone()))
            .cloned()
            .unwrap_or_default();
        recent.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.text.cmp(&a.text))
        });
        let recent_detail = recent
            .iter()
            .take(3)
            .map(|n| format!("{} {} by {}", n.created_at, n.text, n.author))
            .collect::<Vec<_>>()
            .join(" · ");
        let history_cmd = if kind == "intent" {
            format!("loom note list --intent {id} --kind transition --limit 0")
        } else {
            format!("loom note list --edge {id} --kind transition --limit 0")
        };
        if let Some(d) = last_decision.get(id.as_str()) {
            if d.created_at.as_str() > last {
                adjudicated_out.push(AdjudicatedSmell {
                    kind: "recurrent_trouble".into(),
                    summary: format!("'{}' has regressed {} times", label, count),
                    ruling: d.text.clone(),
                    ruled_by: d.author.clone(),
                    ruled_at: d.created_at.clone(),
                    reopens_when: "another failing/needs_change transition lands after the ruling"
                        .into(),
                    teaching: recurrent_teaching(&kind, &id),
                });
                continue;
            }
        }
        smells.push(Smell {
            kind: "recurrent_trouble".into(),
            score: 2.0 * count as f64,
            summary: format!(
                "'{}' has regressed {} times (transitions to failing/needs_change)",
                label, count
            ),
            evidence: format!(
                "{count} transition(s) to failing/needs_change, the last at {last}; recent regressions: {recent_detail}; full history: `{history_cmd}`"
            ),
            remedy: format!(
                "recurring breakage means the criterion or the design is wrong — propose the redesign instead of patching again: `loom hypothesis add --name \"…\" --claim \"<what keeps regressing and the structural why>\" --proposal \"<the redesign>\" --predicted-outcome \"<no failing/needs_change transition after the next N syncs>\"{target}` (proven → adopted → planned intents); once addressed, `loom note add{nt} --kind decision --text \"<what was redesigned and why it won't recur>\"` resolves this finding (a decision newer than the last regression; history stays intact)",
                target = if kind == "intent" { format!(" --target {id}") } else { String::new() },
                nt = if kind == "intent" { format!(" --intent {id}") } else { format!(" --edge {id}") },
            ),
            teaching: recurrent_teaching(&kind, &id),
        });
    }
}

/// 8. Hypothesis accumulation — the pre-decision plane turning into a note dump.
/// A stale or swollen proposed-hypothesis queue means agents add redesign ideas
/// faster than they prove/reject/adopt them.
fn detect_hypothesis_accumulation(
    proposed: &[crate::types::Hypothesis],
    targets: &[crate::types::TargetsEdge],
    smells: &mut Vec<Smell>,
) {
    if proposed.is_empty() {
        return;
    }
    let now = chrono::Utc::now();
    let stale: Vec<&crate::types::Hypothesis> = proposed
        .iter()
        .filter(|h| {
            chrono::DateTime::parse_from_rfc3339(&h.created_at)
                .map(|created| {
                    now.signed_duration_since(created.with_timezone(&chrono::Utc))
                        .num_days()
                        >= HYPOTHESIS_STALE_DAYS
                })
                .unwrap_or(false)
        })
        .collect();
    if proposed.len() >= HYPOTHESIS_BACKLOG_LIMIT || !stale.is_empty() {
        let targeted: HashSet<&str> = targets.iter().map(|t| t.hypothesis_id.as_str()).collect();
        let untargeted = proposed
            .iter()
            .filter(|h| !targeted.contains(h.id.as_str()))
            .count();
        if let Some(oldest) = proposed.iter().min_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.name.cmp(&b.name))
        }) {
            let sample: Vec<&str> = proposed.iter().take(5).map(|h| h.name.as_str()).collect();
            let stale_names: Vec<&str> = stale.iter().take(5).map(|h| h.name.as_str()).collect();
            let stale_detail = if stale_names.is_empty() {
                "none".to_string()
            } else {
                stale_names.join(" · ")
            };
            smells.push(Smell {
                kind: "hypothesis_accumulation".into(),
                score: proposed.len() as f64 + 3.0 * stale.len() as f64,
                summary: format!(
                    "{} proposed hypothesis(es) are waiting for proof; {} stale, {} untargeted",
                    proposed.len(),
                    stale.len(),
                    untargeted
                ),
                evidence: format!(
                    "{} proposed hypothesis(es), {} older than {}d, {} without TARGETS; oldest is '{}' created at {}; examples: {}; stale examples: {}",
                    proposed.len(),
                    stale.len(),
                    HYPOTHESIS_STALE_DAYS,
                    untargeted,
                    oldest.name,
                    oldest.created_at,
                    sample.join(" · "),
                    stale_detail
                ),
                remedy: format!(
                    "drain the pre-decision plane: `loom next --mode prove` then `loom hypothesis prove <id> --verdict supported|refuted --evidence \"…\"`; for supported claims, adopt or reject them (`loom hypothesis adopt|reject`); for untargeted claims, add TARGETS first (`loom hypothesis target <id> <intent>`). Green requires fewer than {limit} proposed hypotheses and none older than {days}d.",
                    limit = HYPOTHESIS_BACKLOG_LIMIT,
                    days = HYPOTHESIS_STALE_DAYS,
                ),
                teaching: teaching_for("hypothesis_accumulation"),
            });
        }
    }
}
