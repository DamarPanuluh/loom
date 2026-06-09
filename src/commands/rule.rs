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

/// The ISO 5055 measuring sticks: (name, severity, description, detection_logic).
/// Two-to-three CWE-grounded rules per quality characteristic, written so an
/// LLM holding one against an intent's code knows exactly what to look for.
/// They are sticks, not detectors — verdicts still come from inspection.
const ISO5055_PACK: &[(&str, &str, &str, &str)] = &[
    // Reliability
    ("iso5055-rel-no-unchecked-failure", "error",
     "ISO 5055 Reliability (CWE-252/248/391): every fallible operation's failure path is handled or explicitly propagated — no silently ignored return value, no exception/panic escaping a boundary uncaught.",
     "Inspect the intent's error paths: ignored Results/return codes, unwrap/expect on external input, bare catch-alls, missing error branches at I/O, parse, lock, and network boundaries."),
    ("iso5055-rel-resource-release", "error",
     "ISO 5055 Reliability (CWE-772/404): every acquired resource (file, lock, connection, handle) is released on ALL paths, including error paths.",
     "Look for acquisitions without RAII/defer/finally protection, locks held across I/O or awaits, and early returns that skip cleanup."),
    ("iso5055-rel-boundary-validation", "error",
     "ISO 5055 Reliability (CWE-20): external input (CLI args, file content, env vars, network data) is validated before use; invalid input yields a typed error, never corruption or a crash.",
     "Trace each external input to its first use: is there a validation/parse step with an error path before the value reaches logic or storage?"),
    // Security
    ("iso5055-sec-no-injection", "error",
     "ISO 5055 Security (CWE-89/78/79): untrusted data is never concatenated into SQL/shell/HTML/query strings — parameterize, escape at the boundary, or reject.",
     "Trace untrusted inputs to every interpreter sink (exec/system calls, query strings, format/eval, HTML output) and check the escaping/parameterization at each."),
    ("iso5055-sec-no-hardcoded-secrets", "error",
     "ISO 5055 Security (CWE-798): no credentials, tokens, or keys in source or config committed to the repo; secrets come from the environment or a secret store.",
     "Scan the intent's files for key-like literals, connection strings with passwords, and tokens; check how the code obtains credentials."),
    ("iso5055-sec-least-surface", "error",
     "ISO 5055 Security (CWE-284/732): expose the minimum — no debug/admin paths reachable in production flows, no overly-permissive file modes or defaults.",
     "Enumerate what the intent exposes (endpoints, files written, flags) and check each against who actually needs it."),
    // Performance efficiency
    ("iso5055-perf-bounded-work", "warning",
     "ISO 5055 Performance Efficiency (CWE-834/1050): no unbounded loops/recursion over external-sized data; iteration and queries are bounded, paginated, or capped.",
     "Look for loops over unbounded collections nested in loops (N+1 patterns), recursion without a depth guard, and full scans where a limit exists."),
    ("iso5055-perf-no-redundant-work", "warning",
     "ISO 5055 Performance Efficiency (CWE-1042/1046): no repeated identical I/O, queries, or allocation in hot paths — cache or hoist invariant work out of loops.",
     "Find work inside loops that is invariant across iterations (reads, compiles, allocations) and repeated identical calls that could be batched."),
    // Maintainability
    ("iso5055-main-single-responsibility", "warning",
     "ISO 5055 Maintainability (CWE-1080/1120): each unit (file, function, intent) owns one coherent responsibility; oversized or multi-concern units are split.",
     "Check unit sizes and concern count; cross-check `loom smells` (tangled_file / scattered_intent) for the same intent."),
    ("iso5055-main-no-dead-or-duplicate-code", "warning",
     "ISO 5055 Maintainability (CWE-561/1041): no unreachable or unused code; no copy-pasted logic where one definition should exist.",
     "Look for unused functions/exports, commented-out blocks kept 'just in case', and near-identical logic in sibling files."),
];

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

        RuleCmd::Seed { pack } => {
            gate::acting_in_lane("seed a rule pack", &[role::QUALITY], None)?;
            if pack != "iso5055" {
                anyhow::bail!("Unknown pack '{}'. Available: iso5055", pack);
            }
            let existing: std::collections::HashSet<String> =
                list_rules(&db)?.into_iter().map(|r| r.name).collect();
            let mut created: Vec<QualityRule> = Vec::new();
            let mut skipped = 0usize;
            for (name, severity, description, detection) in ISO5055_PACK {
                if existing.contains(*name) {
                    skipped += 1;
                    continue;
                }
                let rule = QualityRule {
                    id:              Uuid::new_v4().to_string(),
                    name:            (*name).to_string(),
                    description:     (*description).to_string(),
                    detection_logic: (*detection).to_string(),
                    severity:        (*severity).to_string(),
                };
                insert_rule(&db, &rule)?;
                created.push(rule);
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "pack": pack,
                    "created": created, "skipped_existing": skipped,
                    "next": "loom smells will flag every coded intent these rules were never held against; \
                             resolve each with loom rule apply + loom rule verdict (passing|failing|independent).",
                }));
            } else {
                println!("✓ Seeded pack '{}': {} rule(s) created, {} already present.", pack, created.len(), skipped);
                for r in &created {
                    println!("  + [{}] {}", r.severity, r.name);
                }
                println!("  → `loom smells` now flags every coded intent these were never held against;");
                println!("    measure with `loom rule apply` + `loom rule verdict` (independent = doesn't apply).");
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
            let intent_id = crate::db::queries::resolve_intent(&db, &intent_id)?;
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
            let rule_id = crate::db::queries::resolve_rule(&db, &rule_id)?;
            let intent_id = crate::db::queries::resolve_intent(&db, &intent_id)?;
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
            let rule_id = crate::db::queries::resolve_rule(&db, &rule_id)?;
            let intent_id = crate::db::queries::resolve_intent(&db, &intent_id)?;
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
