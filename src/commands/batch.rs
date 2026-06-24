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
//! Input: one JSON object per line. Stdin ("-", the default) is the
//! frictionless path — paste the lines into a heredoc and run, no scratch
//! file to place, no repo pollution, nothing to clean up:
//!   loom batch - <<'EOF'
//!   {"op":"ground","a":"…","b":"…","confidence":0.9}
//!   EOF
//! A file path argument works for very large batches. Line shapes:
//!   {"op":"ground","a":"<intent>","b":"<intent>","criterion":"…","confidence":0.9}
//!   {"op":"issue","a":"…","b":"…","criterion":"…","evidence":"…","confidence":0.9}
//!   {"op":"independent","a":"…","b":"…","notes":"…"}
//!   {"op":"rule_verdict","rule":"<rule>","intent":"<intent>","status":"passing|failing|independent",
//!    "criterion":"…","evidence":"…","confidence":0.9}
//! ground also takes an optional "evidence"; ground/issue/rule_verdict take an
//! optional "evidence_locator" (string or array of `path:lines` anchors,
//! folded into the stored evidence as `@<locator>`).
//! Intents/rules resolve by id, exact name, or unique fragment — same
//! addressability as everywhere else.
//!
//! `criterion` may be OMITTED on ground/issue/rule_verdict when the edge
//! already carries one — re-verification re-affirms the recorded criterion
//! (that text passed the substantive gate when first written). A FIRST
//! verdict still requires it explicitly; omitting on a bare edge is an error.
//!
//! Failure semantics: the batch CONTINUES past a failed line (each line is an
//! independent verdict), reports per-line results, and exits non-zero if any
//! line failed — so CI and drivers can't mistake a partial batch for a clean one.

use anyhow::Result;
use std::io::Read;

use crate::db::ensure_initialized;
use crate::gate;
use crate::output::Printer;

pub fn run(file: &str, dry_run: bool, printer: &Printer) -> Result<()> {
    // Read ALL input BEFORE opening the database: loom's cross-process write
    // lock is exclusive (acquired on the first write transaction), and
    // `producer | loom batch -` starts both ends of the pipe concurrently —
    // taking the lock first would deadlock any producer that itself calls loom.
    // (A loom-calling producer must still write to a file and pass the path;
    // reading first fixes the non-loom-producer case.)
    let input = if file == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!(
                "Cannot read batch file '{file}': {e} — no scratch file is needed: pipe the lines \
                 via heredoc instead: loom batch - <<'EOF' … EOF (a file path is for very large batches)."
            ))?
    };

    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    run_with_sqlite(&mut store, &cwd, &input, dry_run, printer)
}

fn run_with_sqlite(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    root: &std::path::Path,
    input: &str,
    dry_run: bool,
    printer: &Printer,
) -> Result<()> {
    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut evidences: Vec<String> = Vec::new();

    for (lineno, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let n = lineno + 1;
        match apply_line_sqlite(store, root, dry_run, line) {
            Ok((desc, evidence)) => {
                ok += 1;
                if let Some(e) = evidence {
                    evidences.push(e);
                }
                results.push(serde_json::json!({"line": n, "status": "ok", "applied": desc}));
            }
            Err(e) => {
                failed += 1;
                results.push(
                    serde_json::json!({"line": n, "status": "error", "error": e.to_string()}),
                );
            }
        }
    }

    // Statistical honesty (FLAG, never reject — the policy is reject the
    // unambiguous, flag the rest). The corrupt-batch signature is ONE evidence
    // body pasted across many distinct edges. Re-affirming a stored claim
    // supplies no evidence (it reuses the recorded one), so this only fires on
    // SUPPLIED, byte-identical evidence reused across ≥ REUSE_FLAG edges —
    // copied prose laundering a guess into green. Surfaced so the orchestrator
    // can route the cluster to review; `loom doctor` carries the durable,
    // graph-wide version.
    const REUSE_FLAG: usize = 3;
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for e in &evidences {
        *counts.entry(e.as_str()).or_default() += 1;
    }
    let mut reused: Vec<(&str, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= REUSE_FLAG)
        .collect();
    reused.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let mut warnings: Vec<String> = reused
        .iter()
        .map(|(evidence, c)| {
            let preview: String = evidence.chars().take(60).collect();
            format!(
                "copied evidence: one evidence body recorded on {c} distinct edges in this batch — \
                 confirm it is edge-specific, not pasted, then send the cluster to `loom next --mode review` (\"{preview}…\")"
            )
        })
        .collect();

    // Lane-bypass honesty (audit card 76223551): a bare `llm`/`human` agent
    // (no LOOM_AGENT role) solo-passes every lane, so a multi-agent batch run
    // with a forgotten LOOM_AGENT silently records every verdict as unguarded
    // solo — separation of duties collapses with NO signal at record time. Solo
    // batch by one driver is legitimate, so FLAG (advisory, like the
    // copied-evidence warning), never reject. `loom doctor` carries the
    // durable graph-wide version (it already hints when ALL verdicts are solo);
    // this is the at-record-time surfacing the doctor hint can't give, fired
    // once per run only when a verdict was actually recorded in solo mode.
    if ok > 0 && crate::agent::session_role().is_none() {
        warnings.push(format!(
            "solo mode: {ok} verdict(s) recorded with no LOOM_AGENT role declared — lane gates are \
             OFF (a bare llm passes every lane). Legitimate for one driver; for real separation of \
             duties in a multi-agent run set LOOM_AGENT=llm:<role> per agent \
             (analyzer|fixer|quality|reviewer|validator). See `loom guide`."
        ));
    }

    let snapshot = store.query_snapshot()?;
    let gs = store.graph_state(&snapshot)?;
    let next_step = if failed > 0 {
        format!("fix the {failed} rejected line(s) above and re-run `loom batch`")
    } else if dry_run {
        format!("dry run — nothing written; re-run without --dry-run to apply the {ok} verdict(s)")
    } else {
        gs.next_action.clone()
    };
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": if failed == 0 { "ok" } else { "partial" },
            "dry_run": dry_run,
            "ok": ok, "failed": failed, "results": results,
            "warnings": warnings, "warnings_total": warnings.len(),
            "next_step": next_step,
            "graph_state": crate::output::pulse_json(&gs),
        }));
    } else {
        for r in &results {
            if r["status"] == "ok" {
                println!(
                    "  ✓ line {}: {}",
                    r["line"],
                    r["applied"].as_str().unwrap_or("")
                );
            } else {
                println!(
                    "  ✗ line {}: {}",
                    r["line"],
                    r["error"].as_str().unwrap_or("")
                );
            }
        }
        println!();
        if dry_run {
            println!("  [dry run] {ok} would apply, {failed} would fail (nothing written).");
        } else {
            println!("  {ok} applied, {failed} failed.");
        }
        if !warnings.is_empty() {
            println!(
                "  ⚠ {} advisory warning(s) (flagged — not rejected):",
                warnings.len()
            );
            for w in &warnings {
                println!("    · {w}");
            }
        }
        println!("  → Next: {next_step}");
        println!("  {}", crate::output::fmt_pulse(&gs));
    }
    if failed > 0 {
        anyhow::bail!(
            "{failed} of {} batch op(s) failed — see per-line results above.",
            ok + failed
        );
    }
    Ok(())
}

/// `Some(evidence)` when the op recorded a non-empty evidence body (ground may
/// omit it on re-affirm; independent carries none) — collected across the batch
/// so `run_with_sqlite` can flag copied evidence reused across distinct edges.
fn evidence_opt(evidence: &str) -> Option<String> {
    let e = evidence.trim();
    if e.is_empty() {
        None
    } else {
        Some(e.to_string())
    }
}

trait BatchNoteLookup {
    fn list_notes_by_kind_and_target_kind(
        &self,
        kind: &str,
        target_kind: &str,
    ) -> Result<Vec<crate::types::Note>>;
}

impl BatchNoteLookup for crate::db::sqlite::SqliteGraphStore {
    fn list_notes_by_kind_and_target_kind(
        &self,
        kind: &str,
        target_kind: &str,
    ) -> Result<Vec<crate::types::Note>> {
        Ok(self
            .list_notes(None, Some(kind))?
            .into_iter()
            .filter(|note| note.target_kind == target_kind)
            .collect())
    }
}

fn apply_line_sqlite(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    root: &std::path::Path,
    dry_run: bool,
    line: &str,
) -> Result<(String, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| {
        anyhow::anyhow!(
            "not valid JSON: {e} — each line must be ONE JSON object: {{\"op\": \"<name>\", …}}"
        )
    })?;
    let op = v.get("op").and_then(|x| x.as_str()).ok_or_else(|| {
        anyhow::anyhow!(
            "missing or non-string field 'op' — each line must be ONE JSON object: \
             {{\"op\": \"<name>\", …}} (ground | issue | independent | rule_verdict | smell_decision)"
        )
    })?;
    let now = chrono::Utc::now().to_rfc3339();

    match op {
        "ground" => {
            let by = gate::acting_in_lane(&gate::lane::GROUND_RELATES_TO, None)?;
            let snapshot = store.query_snapshot()?;
            let a = crate::db::queries::resolve_intent_from_snapshot(
                &snapshot,
                str_field(&v, op, "a")?,
            )?;
            let b = crate::db::queries::resolve_intent_from_snapshot(
                &snapshot,
                str_field(&v, op, "b")?,
            )?;
            let confidence = f64_field(&v, op, "confidence")?;
            gate::require_confidence(confidence)?;
            let stored_criterion = store
                .get_relates_to_between(&a, &b)?
                .map(|edge| edge.criterion)
                .unwrap_or_default();
            let criterion = criterion_or_stored(&v, op, &stored_criterion)?;
            gate::require_substantive(
                "criterion",
                criterion,
                gate::RELATES_TO_CRITERION_PURPOSE,
            )?;
            let evidence = v.get("evidence").and_then(|x| x.as_str()).unwrap_or("");
            if !evidence.trim().is_empty() {
                gate::require_substantive(
                    "evidence",
                    evidence,
                    gate::RELATES_TO_EVIDENCE_PURPOSE,
                )?;
            }
            gate::require_locators_resolve(root, &locators_field(&v))?;
            let evidence = gate::compose_evidence(&locators_field(&v), evidence)?;
            if dry_run {
                return Ok((
                    format!("[dry-run] would ground {a} × {b}"),
                    evidence_opt(&evidence),
                ));
            }
            let edge = store
                .upsert_relates_to_ground(&a, &b, criterion, &evidence, confidence, &by, &now)?;
            let kinds = kinds_field(&v);
            if !kinds.is_empty() {
                crate::commands::edge::apply_judgment_kinds(
                    store,
                    &edge.kinds,
                    &edge.from_id,
                    &edge.to_id,
                    &kinds,
                )?;
            }
            Ok((
                format!("ground {} × {}", edge.from_name, edge.to_name),
                evidence_opt(&evidence),
            ))
        }
        "issue" => {
            let by = gate::acting_in_lane(&gate::lane::ISSUE_RELATES_TO, None)?;
            let snapshot = store.query_snapshot()?;
            let a = crate::db::queries::resolve_intent_from_snapshot(
                &snapshot,
                str_field(&v, op, "a")?,
            )?;
            let b = crate::db::queries::resolve_intent_from_snapshot(
                &snapshot,
                str_field(&v, op, "b")?,
            )?;
            let evidence = str_field(&v, op, "evidence")?;
            let confidence = f64_field(&v, op, "confidence")?;
            gate::require_substantive("evidence", evidence, "what was actually found to be wrong")?;
            gate::require_confidence(confidence)?;
            gate::require_locators_resolve(root, &locators_field(&v))?;
            let evidence = gate::compose_evidence(&locators_field(&v), evidence)?;
            let stored_criterion = store
                .get_relates_to_between(&a, &b)?
                .map(|edge| edge.criterion)
                .unwrap_or_default();
            let criterion = criterion_or_stored(&v, op, &stored_criterion)?;
            gate::require_substantive(
                "criterion",
                criterion,
                "the criterion the code was checked against",
            )?;
            if dry_run {
                return Ok((
                    format!("[dry-run] would issue {a} × {b}"),
                    evidence_opt(&evidence),
                ));
            }
            let edge = store
                .upsert_relates_to_issue(&a, &b, criterion, &evidence, confidence, &by, &now)?;
            let kinds = kinds_field(&v);
            if !kinds.is_empty() {
                crate::commands::edge::apply_judgment_kinds(
                    store,
                    &edge.kinds,
                    &edge.from_id,
                    &edge.to_id,
                    &kinds,
                )?;
            }
            Ok((
                format!("issue {} × {}", edge.from_name, edge.to_name),
                evidence_opt(&evidence),
            ))
        }
        "independent" => {
            let by = gate::acting_in_lane(&gate::lane::INDEPENDENT_RELATES_TO, None)?;
            let snapshot = store.query_snapshot()?;
            let a = crate::db::queries::resolve_intent_from_snapshot(
                &snapshot,
                str_field(&v, op, "a")?,
            )?;
            let b = crate::db::queries::resolve_intent_from_snapshot(
                &snapshot,
                str_field(&v, op, "b")?,
            )?;
            let notes = str_field(&v, op, "notes")?;
            gate::require_substantive("notes", notes, gate::INDEPENDENT_NOTES_PURPOSE)?;
            if dry_run {
                return Ok((format!("[dry-run] would mark independent {a} × {b}"), None));
            }
            let edge = store.upsert_relates_to_independent(&a, &b, notes, &by, &now)?;
            Ok((
                format!("independent {} × {}", edge.from_name, edge.to_name),
                None,
            ))
        }
        "rule_verdict" => {
            let by = gate::acting_in_lane(&gate::lane::GOVERNS_VERDICT, None)?;
            let rule = store.resolve_rule(str_field(&v, op, "rule")?)?;
            let snapshot = store.query_snapshot()?;
            let intent = crate::db::queries::resolve_intent_from_snapshot(
                &snapshot,
                str_field(&v, op, "intent")?,
            )?;
            let status = str_field(&v, op, "status")?;
            if status != "passing" && status != "failing" && status != "independent" && status != "partial" {
                anyhow::bail!("invalid status '{status}' (passing | failing | independent | partial)");
            }
            let stored_criterion = store
                .list_governs_for_intent(&intent)?
                .into_iter()
                .find(|edge| edge.rule_id == rule)
                .map(|edge| edge.criterion)
                .unwrap_or_default();
            let criterion = criterion_or_stored(&v, op, &stored_criterion)?;
            let evidence = str_field(&v, op, "evidence")?;
            let confidence = f64_field(&v, op, "confidence")?;
            gate::require_substantive("criterion", criterion, gate::GOVERNS_CRITERION_PURPOSE)?;
            gate::require_substantive(
                "evidence",
                evidence,
                if status == "independent" {
                    gate::VERDICT_EVIDENCE_INDEPENDENT_PURPOSE
                } else {
                    gate::VERDICT_EVIDENCE_FAILING_PURPOSE
                },
            )?;
            gate::require_confidence(confidence)?;
            gate::require_passing_locator(status, &locators_field(&v))?;
            gate::require_locators_resolve(root, &locators_field(&v))?;
            let covers_descendants = v.get("covers_descendants")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            if covers_descendants && evidence.trim().is_empty() {
                anyhow::bail!("covers_descendants=true requires evidence justifying why the same criterion applies to every child");
            }
            let evidence = gate::compose_evidence(&locators_field(&v), evidence)?;
            if dry_run {
                return Ok((
                    format!("[dry-run] would rule_verdict {status}: {rule} → {intent}"),
                    evidence_opt(&evidence),
                ));
            }
            store.upsert_governs_verdict(
                &rule, &intent, status, criterion, &evidence, confidence, &by, &now,
                covers_descendants,
            )?;
            Ok((
                format!("rule_verdict {status}: {rule} → {intent}"),
                evidence_opt(&evidence),
            ))
        }
        "smell_decision" => {
            let smell_id = str_field(&v, op, "smell")?;
            let text = str_field(&v, op, "text")?;
            gate::require_substantive(
                "text",
                text,
                "why this smell finding is accepted for this exact code shape",
            )?;
            let prior_notes = store.list_notes_by_kind_and_target_kind("decision", "smell")?;
            let prior_rulings: Vec<(&str, &str)> = prior_notes
                .iter()
                .filter(|n| n.target_id != smell_id)
                .map(|n| (n.target_id.as_str(), n.text.as_str()))
                .collect();
            gate::require_distinct_smell_ruling(text, &prior_rulings)?;
            if dry_run {
                return Ok((format!("[dry-run] would smell_decision {smell_id}"), None));
            }
            store.insert_note(&crate::types::Note {
                id: uuid::Uuid::new_v4().to_string(),
                kind: "decision".to_string(),
                text: text.to_string(),
                author: crate::agent::acting(None),
                target_kind: "smell".to_string(),
                target_id: smell_id.to_string(),
                resolution: String::new(),
                audience: String::new(),
                created_at: now.clone(),
            })?;
            Ok((format!("smell_decision {smell_id}"), None))
        }
        other => {
            anyhow::bail!("unknown op '{other}' (ground | issue | independent | rule_verdict | smell_decision)")
        }
    }
}

/// The full field list an op requires — quoted back on every missing-field
/// error so a driver can repair the line without consulting the docs.
fn required_fields(op: &str) -> &'static str {
    match op {
        "ground" => "a, b, confidence (+ criterion unless the edge already has one; optional: evidence, evidence_locator)",
        "issue" => "a, b, evidence, confidence (+ criterion unless the edge already has one; optional: evidence_locator)",
        "independent" => "a, b, notes",
        "rule_verdict" => "rule, intent, status, evidence, confidence (+ criterion unless the pair was measured before; optional: evidence_locator)",
        "smell_decision" => "smell, text",
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

/// Optional `evidence_locator`: a single string or an array of strings —
/// file/line anchors folded into the stored evidence (see
/// `gate::compose_evidence`). Absent → no anchors.
fn locators_field(v: &serde_json::Value) -> Vec<String> {
    match v.get("evidence_locator") {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Optional judgment relationship `kind` (string) or `kinds` (array) on a
/// ground/issue line — applied via the same validate-and-merge path as
/// `loom edge explore … --kind`.
fn kinds_field(v: &serde_json::Value) -> Vec<String> {
    match v.get("kinds").or_else(|| v.get("kind")) {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// The explicit `criterion` when given, the stored one when omitted on an
/// edge that already carries it (re-verification re-affirms the recorded
/// claim), and an error otherwise — a first verdict must spell it out.
fn criterion_or_stored<'a>(v: &'a serde_json::Value, op: &str, stored: &'a str) -> Result<&'a str> {
    match v.get("criterion").and_then(|x| x.as_str()) {
        Some(c) if !c.trim().is_empty() => Ok(c),
        _ if !stored.is_empty() => Ok(stored),
        _ => Err(anyhow::anyhow!(
            "op '{op}': no 'criterion' given and none on record — omitting criterion only works \
             when re-verdicting an edge that already carries one (requires: {})",
            required_fields(op)
        )),
    }
}

fn f64_field(v: &serde_json::Value, op: &str, key: &str) -> Result<f64> {
    v.get(key).and_then(|x| x.as_f64()).ok_or_else(|| {
        anyhow::anyhow!(
            "op '{op}': missing or non-numeric field '{key}' (requires: {})",
            required_fields(op)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{criterion_or_stored, locators_field};

    #[test]
    fn criterion_explicit_beats_stored_beats_error() {
        let explicit = serde_json::json!({"criterion": "explicit text"});
        assert_eq!(
            criterion_or_stored(&explicit, "ground", "stored").unwrap(),
            "explicit text"
        );

        let omitted = serde_json::json!({"op": "ground"});
        assert_eq!(
            criterion_or_stored(&omitted, "ground", "stored").unwrap(),
            "stored",
            "omission re-affirms the recorded criterion"
        );

        let blank = serde_json::json!({"criterion": "   "});
        assert_eq!(
            criterion_or_stored(&blank, "ground", "stored").unwrap(),
            "stored",
            "whitespace-only reads as omitted"
        );

        let err = criterion_or_stored(&omitted, "ground", "").unwrap_err();
        assert!(
            err.to_string().contains("none on record"),
            "a first verdict must spell the criterion out: {err}"
        );
    }

    #[test]
    fn evidence_locator_accepts_string_or_array() {
        assert_eq!(
            locators_field(&serde_json::json!({"evidence_locator": "src/a.rs:1-9"})),
            vec!["src/a.rs:1-9".to_string()]
        );
        assert_eq!(
            locators_field(
                &serde_json::json!({"evidence_locator": ["src/a.rs:1-9", "src/b.rs:3"]})
            ),
            vec!["src/a.rs:1-9".to_string(), "src/b.rs:3".to_string()]
        );
        assert!(locators_field(&serde_json::json!({"op": "ground"})).is_empty());
    }
}
