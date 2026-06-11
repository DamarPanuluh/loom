//! `loom batch` — apply many verdicts in ONE call, from JSON Lines.
//!
//! The post-sync re-verification problem: touching two central files can
//! stale dozens of claims, and re-verifying them one CLI invocation each is
//! pure ceremony (agents end up scripting shell loops around loom). This is
//! the sanctioned bulk path: one process, one session, every line still
//! passing the SAME gates as the single-shot commands — lane enforcement,
//! substantive criterion/evidence/notes, confidence in [0,1], transition
//! notes recorded per edge. Bulk changes the ceremony, never the honesty.
//!
//! Input: one JSON object per line (file path arg, or "-" = stdin):
//!   {"op":"ground","a":"<intent>","b":"<intent>","criterion":"…","confidence":0.9}
//!   {"op":"issue","a":"…","b":"…","criterion":"…","evidence":"…","confidence":0.9}
//!   {"op":"independent","a":"…","b":"…","notes":"…"}
//!   {"op":"rule_verdict","rule":"<rule>","intent":"<intent>","status":"passing|failing|independent",
//!    "criterion":"…","evidence":"…","confidence":0.9}
//! Intents/rules resolve by id, exact name, or unique fragment — same
//! addressability as everywhere else.
//!
//! Failure semantics: the batch CONTINUES past a failed line (each line is an
//! independent verdict), reports per-line results, and exits non-zero if any
//! line failed — so CI and drivers can't mistake a partial batch for a clean one.

use anyhow::Result;
use std::io::Read;

use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::db::queries::{
    get_or_create_relates_to, insert_governs, resolve_intent, resolve_rule,
    update_governs_verdict, update_relates_to_ground, update_relates_to_independent,
    update_relates_to_issue,
};
use crate::gate;
use crate::output::Printer;

pub fn run(file: &str, printer: &Printer) -> Result<()> {
    // Read ALL input BEFORE opening the database: the session lock is
    // exclusive, and `producer | loom batch -` starts both ends of the pipe
    // concurrently — taking the lock first would deadlock any producer that
    // itself calls loom. (A loom-calling producer must still write to a file
    // and pass the path; reading first fixes the non-loom-producer case.)
    let input = if file == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("Cannot read batch file '{file}': {e} — pass a readable JSONL file or '-' for stdin."))?
    };

    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut ok = 0usize;
    let mut failed = 0usize;

    for (lineno, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let n = lineno + 1;
        // Per-LINE transaction (not per batch): the contract is "continue past
        // failed lines", so the batch is never all-or-nothing — but one line's
        // verdict and its transition note must land together or not at all.
        match crate::db::with_transaction(&db, || apply_line(&db, line)) {
            Ok(desc) => {
                ok += 1;
                results.push(serde_json::json!({"line": n, "status": "ok", "applied": desc}));
            }
            Err(e) => {
                failed += 1;
                results.push(serde_json::json!({"line": n, "status": "error", "error": e.to_string()}));
            }
        }
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": if failed == 0 { "ok" } else { "partial" },
            "ok": ok, "failed": failed, "results": results,
        }));
    } else {
        for r in &results {
            if r["status"] == "ok" {
                println!("  ✓ line {}: {}", r["line"], r["applied"].as_str().unwrap_or(""));
            } else {
                println!("  ✗ line {}: {}", r["line"], r["error"].as_str().unwrap_or(""));
            }
        }
        println!();
        println!("  {ok} applied, {failed} failed.");
    }
    if failed > 0 {
        anyhow::bail!("{failed} of {} batch op(s) failed — see per-line results above.", ok + failed);
    }
    Ok(())
}

/// Apply one JSONL op through the SAME query functions and gates the
/// single-shot commands use. Returns a one-line description of what happened.
fn apply_line(db: &GrafeoDb, line: &str) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(line)
        .map_err(|e| anyhow::anyhow!("not valid JSON: {e} — each line must be ONE JSON object: {{\"op\": \"<name>\", …}}"))?;
    let op = v
        .get("op")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow::anyhow!(
            "missing or non-string field 'op' — each line must be ONE JSON object: \
             {{\"op\": \"<name>\", …}} (ground | issue | independent | rule_verdict)"
        ))?;
    let now = chrono::Utc::now().to_rfc3339();

    match op {
        "ground" => {
            let by = gate::acting_in_lane(
                "ground a RELATES_TO edge", &[role::ANALYZER, role::FIXER], None,
            )?;
            let a = resolve_intent(db, str_field(&v, op, "a")?)?;
            let b = resolve_intent(db, str_field(&v, op, "b")?)?;
            let criterion = str_field(&v, op, "criterion")?;
            let confidence = f64_field(&v, op, "confidence")?;
            gate::require_substantive(
                "criterion", criterion,
                "the falsifiable coexistence criterion this edge was checked against",
            )?;
            gate::require_confidence(confidence)?;
            let edge = get_or_create_relates_to(db, &a, &b, &now)?;
            update_relates_to_ground(db, &edge.from_id, &edge.to_id, criterion, confidence, &by, &now)?;
            Ok(format!("ground {} × {}", edge.from_name, edge.to_name))
        }
        "issue" => {
            let by = gate::acting_in_lane(
                "record an issue on a RELATES_TO edge", &[role::ANALYZER, role::FIXER], None,
            )?;
            let a = resolve_intent(db, str_field(&v, op, "a")?)?;
            let b = resolve_intent(db, str_field(&v, op, "b")?)?;
            let criterion = str_field(&v, op, "criterion")?;
            let evidence = str_field(&v, op, "evidence")?;
            let confidence = f64_field(&v, op, "confidence")?;
            gate::require_substantive("criterion", criterion, "the criterion the code was checked against")?;
            gate::require_substantive("evidence", evidence, "what was actually found to be wrong")?;
            gate::require_confidence(confidence)?;
            let edge = get_or_create_relates_to(db, &a, &b, &now)?;
            update_relates_to_issue(db, &edge.from_id, &edge.to_id, criterion, evidence, confidence, &by, &now)?;
            Ok(format!("issue {} × {}", edge.from_name, edge.to_name))
        }
        "independent" => {
            let by = gate::acting_in_lane(
                "confirm two intents independent", &[role::ANALYZER], None,
            )?;
            let a = resolve_intent(db, str_field(&v, op, "a")?)?;
            let b = resolve_intent(db, str_field(&v, op, "b")?)?;
            let notes = str_field(&v, op, "notes")?;
            gate::require_substantive(
                "notes", notes, "why these two intents have no meaningful relationship",
            )?;
            let edge = get_or_create_relates_to(db, &a, &b, &now)?;
            update_relates_to_independent(db, &edge.from_id, &edge.to_id, notes, &by, &now)?;
            Ok(format!("independent {} × {}", edge.from_name, edge.to_name))
        }
        "rule_verdict" => {
            let by = gate::acting_in_lane("record a GOVERNS verdict", &[role::QUALITY], None)?;
            let rule = resolve_rule(db, str_field(&v, op, "rule")?)?;
            let intent = resolve_intent(db, str_field(&v, op, "intent")?)?;
            let status = str_field(&v, op, "status")?;
            if status != "passing" && status != "failing" && status != "independent" {
                anyhow::bail!("invalid status '{status}' (passing | failing | independent)");
            }
            let criterion = str_field(&v, op, "criterion")?;
            let evidence = str_field(&v, op, "evidence")?;
            let confidence = f64_field(&v, op, "confidence")?;
            gate::require_substantive(
                "criterion", criterion, "what compliance looks like for this rule on this intent",
            )?;
            gate::require_substantive(
                "evidence", evidence,
                if status == "independent" { "why this rule does not apply to this intent" }
                else { "what was actually found in the code during inspection" },
            )?;
            gate::require_confidence(confidence)?;
            // The verdict IS the measurement — create the edge if absent,
            // exactly like the single-shot `loom rule verdict`.
            let found = update_governs_verdict(
                db, &rule, &intent, status, criterion, evidence, confidence, &by, &now,
            )?;
            if !found {
                insert_governs(db, &rule, &intent, criterion, &now)?;
                update_governs_verdict(
                    db, &rule, &intent, status, criterion, evidence, confidence, &by, &now,
                )?;
            }
            Ok(format!("rule_verdict {status}: {rule} → {intent}"))
        }
        other => anyhow::bail!(
            "unknown op '{other}' (ground | issue | independent | rule_verdict)"
        ),
    }
}

/// The full field list an op requires — quoted back on every missing-field
/// error so a driver can repair the line without consulting the docs.
fn required_fields(op: &str) -> &'static str {
    match op {
        "ground" => "a, b, criterion, confidence",
        "issue" => "a, b, criterion, evidence, confidence",
        "independent" => "a, b, notes",
        "rule_verdict" => "rule, intent, status, criterion, evidence, confidence",
        _ => "op",
    }
}

fn str_field<'a>(v: &'a serde_json::Value, op: &str, key: &str) -> Result<&'a str> {
    v.get(key).and_then(|x| x.as_str()).ok_or_else(|| {
        anyhow::anyhow!(
            "op '{op}': missing or non-string field '{key}' (requires: {})",
            required_fields(op)
        )
    })
}

fn f64_field(v: &serde_json::Value, op: &str, key: &str) -> Result<f64> {
    v.get(key).and_then(|x| x.as_f64()).ok_or_else(|| {
        anyhow::anyhow!(
            "op '{op}': missing or non-numeric field '{key}' (requires: {})",
            required_fields(op)
        )
    })
}
