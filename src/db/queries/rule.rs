//! QualityRule node queries.

use anyhow::Result;

use crate::db::schema::esc;
use crate::db::LoomDb;
use crate::types::QualityRule;

use super::row::{col_map, get, str_val};

pub fn insert_rule(db: &dyn LoomDb, rule: &QualityRule) -> Result<()> {
    let q = format!(
        "INSERT (:QualityRule {{id: '{id}', name: '{name}', description: '{desc}', \
         detection_logic: '{logic}', severity: '{sev}'}})",
        id    = esc(&rule.id),
        name  = esc(&rule.name),
        desc  = esc(&rule.description),
        logic = esc(&rule.detection_logic),
        sev   = esc(&rule.severity),
    );
    db.execute(&q)?;
    Ok(())
}

pub fn list_rules(db: &dyn LoomDb) -> Result<Vec<QualityRule>> {
    let q = "MATCH (r:QualityRule) \
             RETURN r.id, r.name, r.description, r.detection_logic, r.severity \
             ORDER BY r.name";
    let result = db.execute(q)?;
    let cols = col_map(&result);
    Ok(result.rows().iter().map(|row| QualityRule {
        id:              str_val(get(row, &cols, "r.id")),
        name:            str_val(get(row, &cols, "r.name")),
        description:     str_val(get(row, &cols, "r.description")),
        detection_logic: str_val(get(row, &cols, "r.detection_logic")),
        severity:        str_val(get(row, &cols, "r.severity")),
    }).collect())
}
