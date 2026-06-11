//! QualityRule node queries.

use anyhow::Result;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::QualityRule;

use super::row::{col_map, get, str_val};

pub fn insert_rule(db: &dyn LoomDb, rule: &QualityRule) -> Result<()> {
    let q = format!(
        "INSERT (:QualityRule {{id: '{id}', name: '{name}', description: '{desc}', \
         detection_logic: '{logic}', severity: '{sev}', inspection_effort: '{eff}'}})",
        id    = esc(&rule.id),
        name  = esc(&rule.name),
        desc  = esc(&rule.description),
        logic = esc(&rule.detection_logic),
        sev   = esc(&rule.severity),
        eff   = esc(&rule.inspection_effort),
    );
    db.execute(&q)?;
    Ok(())
}

/// Resolve a rule key — exact id, exact name, or unique name fragment — to
/// the rule's id (same contract as `resolve_intent`; rule names are the
/// natural key, e.g. "iso5055-sec-no-injection").
pub fn resolve_rule(db: &dyn LoomDb, key: &str) -> Result<String> {
    let rules = list_rules(db)?;
    if rules.iter().any(|r| r.id == key) {
        return Ok(key.to_string());
    }
    let kl = key.to_lowercase();
    let exact: Vec<_> = rules.iter().filter(|r| r.name.to_lowercase() == kl).collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    let subs: Vec<_> = rules.iter().filter(|r| r.name.to_lowercase().contains(&kl)).collect();
    match subs.len() {
        1 => Ok(subs[0].id.clone()),
        0 => anyhow::bail!(
            "No rule matches '{}' (by id, exact name, or name fragment). Run `loom rule list`.",
            key
        ),
        _ => {
            // Bounded: an ambiguity over a big rule set must not flood the
            // driver's context — show a sample, point at the inventory.
            let cap = crate::output::SECTION_CAP;
            let mut shown = subs
                .iter()
                .take(cap)
                .map(|r| format!("'{}'", r.name))
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(m) = crate::output::more_marker(subs.len(), subs.len().min(cap), "`loom rule list`") {
                shown.push_str(", ");
                shown.push_str(&m);
            }
            anyhow::bail!(
                "'{}' is ambiguous — it matches: {}. Narrow the fragment or use an id.",
                key, shown
            )
        }
    }
}

pub fn list_rules(db: &dyn LoomDb) -> Result<Vec<QualityRule>> {
    let q = "MATCH (r:QualityRule) \
             RETURN r.id, r.name, r.description, r.detection_logic, r.severity, \
                    r.inspection_effort \
             ORDER BY r.name";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| QualityRule {
        id:              str_val(get(row, &cols, "r.id")),
        name:            str_val(get(row, &cols, "r.name")),
        description:     str_val(get(row, &cols, "r.description")),
        detection_logic: str_val(get(row, &cols, "r.detection_logic")),
        severity:        str_val(get(row, &cols, "r.severity")),
        // Optional field — absent on rules created before the effort axis;
        // "" reads as mid everywhere.
        inspection_effort: str_val(get(row, &cols, "r.inspection_effort")),
    }).collect())
}
