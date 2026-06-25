use anyhow::Result;
use std::collections::{HashMap, HashSet};

use super::{teaching_for, Smell, SmellCtx};

/// Normative plane — quality-rule coverage and vocab hygiene.
pub(super) fn detect_normative_plane(ctx: &SmellCtx, smells: &mut Vec<Smell>) -> Result<()> {
    detect_unmeasured_intents(
        ctx.intents,
        ctx.rules,
        ctx.governs,
        ctx.hierarchy,
        &ctx.files_of,
        smells,
    );
    detect_unused_rule(ctx.rules, ctx.governs, smells);
    detect_vocab_drift(ctx.intents, ctx.vocab_terms, smells)?;
    Ok(())
}
/// 5. The measuring stick, unused — the normative plane only measures where
/// someone applied a rule. Surfaces every rule × coded-intent pairing never
/// considered (no GOVERNS edge directly or via an ancestor's verdict).
fn detect_unmeasured_intents(
    intents: &[crate::types::Intent],
    rules: &[crate::types::QualityRule],
    governs: &[crate::types::Governs],
    hierarchy: &[(String, String)],
    files_of: &HashMap<&str, HashSet<&str>>,
    smells: &mut Vec<Smell>,
) {
    let considered: HashSet<(&str, &str)> = governs
        .iter()
        .filter(|g| {
            matches!(
                g.inspection_status.as_str(),
                "passing" | "failing" | "independent" | "partial"
            )
        })
        .map(|g| (g.rule_id.as_str(), g.intent_id.as_str()))
        .collect();
    let covers_set = super::super::scoring::covers_descendants_set(governs);
    let parent_of: HashMap<&str, &str> = hierarchy
        .iter()
        .map(|(p, c)| (c.as_str(), p.as_str()))
        .collect();
    let considered_up = |rule_id: &str, intent_id: &str| -> bool {
        super::super::scoring::governs_covers_intent(
            rule_id,
            intent_id,
            &considered,
            &covers_set,
            &parent_of,
        )
    };
    for r in rules {
        let unmeasured: Vec<&crate::types::Intent> = intents
            .iter()
            .filter(|i| {
                i.status != "deprecated"
                    && files_of.contains_key(i.id.as_str())
                    && !considered_up(&r.id, &i.id)
            })
            .collect();
        if unmeasured.is_empty() {
            continue;
        }
        let sample: Vec<String> = unmeasured
            .iter()
            .take(3)
            .map(|i| format!("{} ({})", i.name, i.id))
            .collect();
        smells.push(Smell {
            kind: "unmeasured_intents".into(),
            score: unmeasured.len() as f64,
            summary: format!(
                "rule '{}' has never been held against {} intent(s) that have code (neither directly nor via an ancestor's verdict)",
                r.name,
                unmeasured.len()
            ),
            evidence: format!("e.g. {}", sample.join(" · ")),
            remedy: format!(
                "measure at the highest HONEST altitude: loom rule verdict {} <component> --status passing|failing|independent --covers-descendants covers the component's descendants too (without --covers-descendants it covers only the component; independent = measured, rule doesn't apply); otherwise drop to a leaf where the rule has specific bite",
                r.id
            ),
            teaching: teaching_for("unmeasured_intents"),
        });
    }
}
/// 10. Unused rule — a measuring stick connected to nothing at all.
fn detect_unused_rule(
    rules: &[crate::types::QualityRule],
    governs: &[crate::types::Governs],
    smells: &mut Vec<Smell>,
) {
    let used: HashSet<&str> = governs.iter().map(|g| g.rule_id.as_str()).collect();
    for r in rules {
        if !used.contains(r.id.as_str()) {
            smells.push(Smell {
                kind: "unused_rule".into(),
                score: 5.0,
                summary: format!("rule '{}' governs nothing", r.name),
                evidence: "a quality rule with zero GOVERNS edges measures nothing".into(),
                remedy: format!(
                    "loom rule verdict {} <intent-id> --status passing|failing|independent --criterion … --evidence … (the verdict creates the edge and measures it in one step; independent = the rule does not apply) — or delete it if it was a mistake",
                    r.id
                ),
                teaching: teaching_for("unused_rule"),
            });
        }
    }
}

/// 11. Vocab drift — the registry policing itself: two registered terms that
/// read like the same word. Synonym terms split the keyspace and silently halve
/// the duplicate-detection signal.
fn detect_vocab_drift(
    intents: &[crate::types::Intent],
    vocab_terms: &[crate::types::VocabTerm],
    smells: &mut Vec<Smell>,
) -> Result<()> {
    let terms = vocab_terms;
    let counts = crate::db::queries::vocab::tag_counts(intents)?;
    for i in 0..terms.len() {
        for j in (i + 1)..terms.len() {
            let (a, b) = (&terms[i], &terms[j]);
            if !crate::db::queries::vocab::terms_look_alike(&a.name, &b.name) {
                continue;
            }
            let (ua, ub) = (
                counts.get(&a.name).copied().unwrap_or(0),
                counts.get(&b.name).copied().unwrap_or(0),
            );
            // Keep the better-established term; merge the other into it.
            let (keep, drop) = if ua >= ub { (a, b) } else { (b, a) };
            smells.push(Smell {
                kind: "vocab_drift".into(),
                score: 3.0 + (ua + ub) as f64,
                summary: format!(
                    "vocab terms '{}' and '{}' read like the same word — split keyspace halves collision signal",
                    a.name, b.name
                ),
                evidence: format!(
                    "'{}' tags {} intent(s), '{}' tags {} intent(s); intents split across synonym terms never collide in duplicate detection",
                    a.name, ua, b.name, ub
                ),
                remedy: format!(
                    "loom vocab merge {} {}  → retags every intent and deletes '{}' (one sweep, nothing to re-inspect); if they are genuinely distinct concepts the NAMES must stop reading alike — register a sharper term (`loom vocab add`), retag its intents (`loom intent tag`), then merge the look-alike away",
                    drop.name, keep.name, drop.name
                ),
                teaching: teaching_for("vocab_drift"),
            });
        }
    }
    Ok(())
}
