use anyhow::Result;
use std::env;
use uuid::Uuid;

use crate::cli::RuleCmd;
use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    insert_governs, insert_rule, list_governs_for_intent, list_rules, update_governs_verdict,
};
use crate::gate;
use crate::output::{fmt_rule_row, Printer};
use crate::types::QualityRule;

pub fn run(cmd: RuleCmd, printer: &Printer) -> Result<()> {
    let cwd = env::current_dir()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    match cmd {
        RuleCmd::Add { name, description, severity } => {
            gate::acting_in_lane("add a quality rule", &[role::QUALITY], None)?;
            // Validate severity
            severity.parse::<crate::types::Severity>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            let id   = Uuid::new_v4().to_string();
            let rule = QualityRule {
                id:              id.clone(),
                name:            name.clone(),
                description,
                detection_logic: String::new(),
                severity,
            };
            insert_rule(&db, &rule)?;

            if printer.json {
                printer.print_json(&rule);
            } else {
                println!("✓ Rule '{}' created  (id: {})", name, id);
            }
        }

        RuleCmd::List => {
            let rules = list_rules(&db)?;
            if printer.json {
                printer.print_json(&rules);
            } else if rules.is_empty() {
                println!("(no rules defined)");
            } else {
                for r in &rules {
                    println!("{}", fmt_rule_row(r));
                }
            }
        }

        RuleCmd::Check { intent_id } => {
            // Show all GOVERNS edges for this intent (grouped by inspection_status)
            let governs = list_governs_for_intent(&db, &intent_id)?;
            if printer.json {
                printer.print_json(&governs);
            } else if governs.is_empty() {
                println!("No GOVERNS edges for intent '{}' — no rules applied.", intent_id);
                println!("  → Apply a rule: loom edge govern <rule-id> {}", intent_id);
            } else {
                let failing: Vec<_> = governs
                    .iter()
                    .filter(|g| g.inspection_status == "failing")
                    .collect();
                let passing: Vec<_> = governs
                    .iter()
                    .filter(|g| g.inspection_status == "passing")
                    .collect();
                let uninspected: Vec<_> = governs
                    .iter()
                    .filter(|g| g.inspection_status == "uninspected")
                    .collect();

                println!(
                    "GOVERNS edges for intent '{}':  {} failing, {} passing, {} uninspected",
                    intent_id, failing.len(), passing.len(), uninspected.len()
                );
                println!();
                for g in &failing {
                    println!(
                        "  [FAILING]  rule={rname}  criterion={crit}",
                        rname = g.rule_name,
                        crit  = g.criterion,
                    );
                    if !g.evidence.is_empty() {
                        println!("    evidence: {}", g.evidence);
                    }
                }
                for g in &uninspected {
                    println!(
                        "  [uninspected]  rule={}  (edge id: {})",
                        g.rule_name, g.id
                    );
                }
                for g in &passing {
                    println!("  [passing]  rule={}", g.rule_name);
                }
            }
        }

        RuleCmd::Apply { rule_id, intent_id, criterion } => {
            gate::acting_in_lane("apply a quality rule", &[role::QUALITY], None)?;
            let now = chrono::Utc::now().to_rfc3339();
            let edge_id = Uuid::new_v4().to_string();
            let crit = criterion.as_deref().unwrap_or("");
            if !crit.is_empty() {
                // Criterion is optional at apply time (the edge starts
                // uninspected) — but if given, it must be substantive.
                gate::require_substantive(
                    "criterion", crit,
                    "what compliance looks like for this rule on this intent",
                )?;
            }
            insert_governs(&db, &edge_id, &rule_id, &intent_id, crit, &now)?;
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":    "ok",
                    "edge_id":   edge_id,
                    "rule_id":   rule_id,
                    "intent_id": intent_id,
                    "message":   "GOVERNS edge created with inspection_status=uninspected. \
                                  Inspect and update via `loom rule check`.",
                }));
            } else {
                println!("✓ GOVERNS edge created  (id: {})", edge_id);
                println!("  rule   → {}", rule_id);
                println!("  intent → {}", intent_id);
                println!("  Run `loom rule check {}` to inspect.", intent_id);
            }
        }

        RuleCmd::Verdict {
            rule_id, intent_id, status, criterion, evidence, confidence, inspected_by,
        } => {
            let by = gate::acting_in_lane(
                "record a GOVERNS verdict", &[role::QUALITY], inspected_by.as_deref(),
            )?;
            if status != "passing" && status != "failing" && status != "independent" {
                anyhow::bail!(
                    "Invalid --status '{}'. A verdict is passing (complies), failing (violates), \
                     or independent (measured — the rule does not apply to this intent).",
                    status
                );
            }
            gate::require_substantive(
                "criterion", &criterion,
                "what compliance looks like for this rule on this intent (falsifiable)",
            )?;
            gate::require_substantive(
                "evidence", &evidence,
                if status == "independent" {
                    "why this rule does not apply to this intent"
                } else {
                    "what was actually found in the code during inspection"
                },
            )?;
            gate::require_confidence(confidence)?;

            let now = chrono::Utc::now().to_rfc3339();
            let found = update_governs_verdict(
                &db, &rule_id, &intent_id, &status, &criterion, &evidence,
                confidence, &by, &now,
            )?;
            if !found {
                anyhow::bail!(
                    "No GOVERNS edge between rule '{}' and intent '{}'. \
                     Apply the rule first: loom rule apply {} {}",
                    rule_id, intent_id, rule_id, intent_id
                );
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":            "ok",
                    "rule_id":           rule_id,
                    "intent_id":         intent_id,
                    "inspection_status": status,
                    "criterion":         criterion,
                    "evidence":          evidence,
                    "confidence":        confidence,
                    "inspected_by":      by,
                    "last_inspected":    now,
                }));
            } else {
                let mark = match status.as_str() {
                    "passing" => "✓",
                    "independent" => "◦",
                    _ => "✗",
                };
                println!("{} GOVERNS verdict recorded: {}", mark, status);
                println!("  rule   → {}", rule_id);
                println!("  intent → {}", intent_id);
                if status == "failing" {
                    println!("  → Next: flag the intent (`loom intent mark {} --lifecycle needs_change --reason \"…\"`) or fix and re-verdict.", intent_id);
                }
            }
        }
    }
    Ok(())
}
