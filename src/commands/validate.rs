use anyhow::Result;
use std::process::Command as StdCommand;
use std::thread;
use std::time::{Duration, Instant};

use crate::db::{ensure_initialized, sqlite_db_path};
use crate::output::Printer;
use crate::types::ValidationResult;

pub fn run(intent_id: &str, timeout_secs: u64, printer: &Printer) -> Result<()> {
    // Running validations writes last_run/last_result and the VALIDATES
    // verdict — validator lane.
    let marker = crate::gate::acting_in_lane(&crate::gate::lane::RUN_VALIDATIONS, None)?;
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&sqlite_db_path(&cwd))?;
    let snapshot = store.query_snapshot()?;
    let intent_id = crate::db::queries::resolve_intent_from_snapshot(&snapshot, intent_id)?;

    // Ensure intent exists
    store
        .get_intent(&intent_id)?
        .ok_or_else(|| anyhow::anyhow!(crate::output::intent_not_found_list(&intent_id)))?;

    let to_run = store.validations_for_intent(&intent_id)?;
    if to_run.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "intent_id": intent_id,
                "results":   [],
                "message":   "No validations linked to this intent.",
                "next_step": format!(
                    "Add one: `loom validation add --name \"...\" --type test --command \"cargo test ...\"`, \
                     then link it: `loom edge validates <validation-id> {}`",
                    intent_id
                ),
            }));
        } else {
            println!("No validation nodes linked to intent '{}'.", intent_id);
            println!("  → Add one:  loom validation add --name \"...\" --type test --command \"cargo test ...\"");
            println!(
                "  → Link it:  loom edge validates <validation-id> {}",
                intent_id
            );
        }
        return Ok(());
    }

    drop(store);

    execute_and_record(
        &cwd,
        &to_run,
        timeout_secs,
        printer,
        ("intent_id", serde_json::json!(intent_id)),
        &marker,
    )
}

/// `loom validate --all`: run every PENDING proof — last_result == not_run,
/// i.e. never run or invalidated by a sync flood. One verb instead of
/// enumerating intents by hand after `loom sync` resets N proofs at once.
/// Passed/failed results are settled verdicts (re-run them per intent when you
/// mean to); blocked proofs carry a recorded reason and stay out everywhere.
pub fn run_all(timeout_secs: u64, printer: &Printer) -> Result<()> {
    let marker = crate::gate::acting_in_lane(&crate::gate::lane::RUN_VALIDATIONS, None)?;
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&sqlite_db_path(&cwd))?;

    let to_run: Vec<crate::types::Validation> = store
        .list_validations()?
        .into_iter()
        .filter(|v| v.last_result == "not_run")
        .collect();

    if to_run.is_empty() {
        if printer.json {
            printer.print_json(&serde_json::json!({
                "scope":   "all",
                "results": [],
                "message": "Nothing pending — every proof has a recorded result (passed/failed/blocked).",
            }));
        } else {
            println!(
                "✓ Nothing pending — every proof has a recorded result (passed/failed/blocked)."
            );
        }
        return Ok(());
    }
    if !printer.json {
        println!("Running {} pending validation(s)…", to_run.len());
    }
    drop(store);

    execute_and_record(
        &cwd,
        &to_run,
        timeout_secs,
        printer,
        ("scope", serde_json::json!("all")),
        &marker,
    )
}

/// Phases 2+3 shared by `run` and `run_all`: execute commands with the DB
/// CLOSED (the graph lock must be released — a validation may itself invoke
/// loom; found by loom validating itself), then reopen and persist results +
/// VALIDATES verdicts in one transaction. `scope` is the JSON envelope key
/// identifying what was run (intent_id vs all).
fn execute_and_record(
    cwd: &std::path::Path,
    to_run: &[crate::types::Validation],
    timeout_secs: u64,
    printer: &Printer,
    scope: (&str, serde_json::Value),
    marker: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut results: Vec<serde_json::Value> = Vec::new();
    // (validation_id, result, edge note, discrimination_status)
    let mut outcomes: Vec<(String, String, String, String)> = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;

    for validation in to_run {
        if validation.last_result == "blocked" {
            // A recorded "can't run yet" — don't run it, don't overwrite it.
            // Unblock by re-marking: `loom validation mark <id> --result passed|failed`.
            results.push(serde_json::json!({
                "validation_id": validation.id,
                "name":          validation.name,
                "result":        "blocked",
                "reason":        "marked blocked — see its VALIDATES edge notes for why",
            }));
            if !printer.json {
                println!(
                    "  ⊘ {} [blocked — re-mark with `loom validation mark` when unblocked]",
                    validation.name
                );
            }
            continue;
        }
        if validation.command.is_empty() {
            results.push(serde_json::json!({
                "validation_id": validation.id,
                "name":          validation.name,
                "result":        "skipped",
                "reason":        "no command defined — record by hand: `loom validation mark`",
            }));
            if !printer.json {
                println!(
                    "  - {} [skipped — no command; record by hand: `loom validation mark {}`]",
                    validation.name, validation.id
                );
            }
            continue;
        }

        // A saga consumes a LIVE target via `{{ env.X }}` values passed at
        // invocation. If they're missing here, the proof CANNOT run — that is
        // `blocked` (environment not ready), not `failed` (code wrong). Running
        // the command anyway would record a dishonest failure and send the
        // driver chasing a phantom code bug.
        if validation.validation_type == "saga" {
            if let Some(missing) = saga_missing_env(cwd, validation) {
                let invocation: String = missing
                    .iter()
                    .map(|v| format!("{v}=<value> "))
                    .chain([format!("loom saga run {}", validation.name)])
                    .collect();
                let diagnose_invocation: String = missing
                    .iter()
                    .map(|v| format!("{v}=<value> "))
                    .chain([format!("loom saga diagnose {}", validation.name)])
                    .collect();
                let reason = format!(
                    "missing env value(s): {} — bring up the live target the way this repo does (docker-compose / Makefile / scripts / README), diagnose with `{}`, then stamp proof with `{}`; mark blocked only if it genuinely can't run yet",
                    missing.join(", "),
                    diagnose_invocation,
                    invocation
                );
                outcomes.push((
                    validation.id.clone(),
                    "blocked".to_string(),
                    format!("blocked: {reason}"),
                    String::new(), // not run → no discrimination
                ));
                results.push(serde_json::json!({
                    "validation_id": validation.id,
                    "name":          validation.name,
                    "result":        "blocked",
                    "reason":        reason,
                }));
                if !printer.json {
                    println!("  ⊘ {} [blocked — {}]", validation.name, reason);
                }
                continue;
            }
        }

        // Run the command via sh -c so shell features work (e.g. cargo test --test foo)
        let (exit_status, output) =
            match run_validation_command(&validation.command, cwd, timeout_secs) {
                Ok(pair) => (Ok(pair.0), pair.1),
                Err(e) => (Err(e), String::new()),
            };

        let (result, detail) = match exit_status {
            Ok(CommandOutcome::Exited(s)) if s.success() => {
                passed += 1;
                (ValidationResult::Passed, None)
            }
            Ok(CommandOutcome::Exited(s)) => {
                failed += 1;
                (ValidationResult::Failed, Some(format!("exited with {s}")))
            }
            Ok(CommandOutcome::TimedOut) => {
                failed += 1;
                (
                    ValidationResult::Failed,
                    Some(format!("timed out after {timeout_secs}s")),
                )
            }
            Err(e) => {
                failed += 1;
                eprintln!(
                    "Warning: Could not run command for '{}': {}",
                    validation.name, e
                );
                (ValidationResult::Failed, Some(e.to_string()))
            }
        };
        let new_result = result.to_string();
        // G2: only a PASSED run whose captured output shows a runner ASSERTING
        // earns `discriminating` (→ EXECUTED tier). A passed-but-inert run
        // (exit 0, no assertion signal) or any non-pass run is `ran_inert`.
        let discrimination = if result == ValidationResult::Passed {
            proof_discrimination(&output)
        } else {
            "ran_inert"
        };
        outcomes.push((
            validation.id.clone(),
            new_result.clone(),
            String::new(),
            discrimination.to_string(),
        ));

        let mut entry = serde_json::json!({
            "validation_id": validation.id,
            "name":          validation.name,
            "type":          validation.validation_type,
            "command":       validation.command,
            "result":        &new_result,
            "discrimination": discrimination,
            "run_at":        &now,
        });
        if let Some(detail) = &detail {
            entry["detail"] = serde_json::Value::String(detail.clone());
        }
        results.push(entry);

        if !printer.json {
            let mark = if new_result == "passed" { "✓" } else { "✗" };
            println!("  {} {} [{}]", mark, validation.name, new_result);
            println!("    cmd: {}", validation.command);
            if let Some(detail) = &detail {
                println!("    detail: {detail}");
            }
            // Surface the discrimination gate AT pass time, not three reads later.
            // A command that exits 0 but asserts nothing loom recognizes is
            // ASSERTED-only, not EXECUTED — it never advances the Realized rung,
            // and a driver who isn't told here only discovers it via `loom status`.
            if new_result == "passed"
                && discrimination == "ran_inert"
                && !validation.command.trim().is_empty()
            {
                println!(
                    "    ⚠ passed but NON-DISCRIMINATING: exit 0 with no recognized assertion signal \
                     (e.g. `test result: ok. N passed`, `N passing`, `--- PASS:`) — counts as \
                     ASSERTED-only, NOT executed-proven, so it will NOT advance the Realized rung. \
                     Make the test ASSERT ≥1 thing."
                );
            }
        }
    }

    let mut store = crate::db::sqlite::SqliteGraphStore::open(&sqlite_db_path(cwd))?;
    for (vid, new_result, edge_note, discrimination) in &outcomes {
        store.mark_validation_result(
            vid,
            new_result,
            validation_result_edge_status(new_result),
            edge_note,
            marker,
            &now,
            // The executor RAN the command — stamp last_executed_run so the
            // proven axis counts this as EXECUTED, not merely ASSERTED…
            Some(&now),
            // …but only `discriminating` (the runner actually asserted) feeds
            // the EXECUTED tier; a `ran_inert` exit-0 falls back to ASSERTED.
            Some(discrimination),
        )?;
    }

    // End-of-run summary moves the phase: full anchor, result-sensitive.
    let next_step = if failed > 0 {
        "`loom next --mode fix`"
    } else {
        crate::output::STATUS_RECHECK_NEXT_STEP
    };
    if printer.json {
        let (scope_key, scope_val) = scope;
        printer.print_json(&crate::output::with_read_anchor(
            serde_json::json!({
                scope_key:   scope_val,
                "passed":    passed,
                "failed":    failed,
                "results":   results,
            }),
            &store,
            next_step,
        )?);
    } else {
        println!();
        println!("  Summary: {}/{} passed", passed, passed + failed);
        let snapshot = store.query_snapshot()?;
        let graph_state = store.graph_state(&snapshot)?;
        println!("  → Next: {next_step}");
        println!("  {}", crate::output::fmt_pulse(&graph_state));
    }

    Ok(())
}

enum CommandOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
}

/// Run a proof command and return BOTH its outcome and its captured output.
/// Output is captured to a temp file (not inherited) — so the runner's stdout no
/// longer leaks into `loom validate --json`, and G2 can inspect what the runner
/// actually printed to decide whether it asserted anything.
fn run_validation_command(
    command: &str,
    cwd: &std::path::Path,
    timeout_secs: u64,
) -> Result<(CommandOutcome, String)> {
    use std::fs::File;
    // Unique temp path without Date/random (unavailable): pid + a process-local
    // sequence. Captures stdout AND stderr (runners split across both).
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let log_path =
        std::env::temp_dir().join(format!("loom-proof-{}-{seq}.log", std::process::id()));
    let log = File::create(&log_path)?;
    let log_err = log.try_clone()?;

    let mut cmd = StdCommand::new("sh");
    cmd.arg("-c").arg(command).current_dir(cwd);
    // Throwaway-graph proofs (temp-dir init/import) must not inherit a pinned
    // session — LOOM_GRAPH beats cwd and would mutate the driver's graph.
    cmd.env_remove("LOOM_GRAPH");
    cmd.stdout(log).stderr(log_err);
    // Run the command in its OWN process group so the timeout can kill the WHOLE
    // tree. `sh -c` forks the real runner (cargo, a test, a hostile `sleep`);
    // killing only `sh` let the child outlive the deadline — the timeout read as
    // enforced ("timed out after 1s") while the work ran to completion (a DoS
    // guard that didn't guard).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);

    let outcome = loop {
        if let Some(status) = child.try_wait()? {
            break CommandOutcome::Exited(status);
        }
        if Instant::now() >= deadline {
            terminate_command_tree(&mut child);
            let _ = child.wait();
            break CommandOutcome::TimedOut;
        }
        thread::sleep(Duration::from_millis(100));
    };
    let captured = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);
    Ok((outcome, captured))
}

/// G2 falsification-witness: did the captured output show a recognized test
/// runner ACTUALLY ASSERT at least one thing? Conservative — an unrecognized or
/// zero-assertion run is `ran_inert` (demoted to the ASSERTED tier), so exit-0
/// alone can never mint EXECUTED. cargo is mandatory (dogfood); pytest / jest /
/// vitest / mocha (`N passed`) and `go test -v` (`--- PASS:`) are recognized.
pub(crate) fn proof_discrimination(output: &str) -> &'static str {
    for raw in output.lines() {
        let line = raw.trim();
        // cargo: "test result: ok. 12 passed; 0 failed; …"
        if let Some(rest) = line.strip_prefix("test result: ok.") {
            if leading_count(rest) >= 1 {
                return "discriminating";
            }
        }
        // go test -v: at least one passing test function.
        if line.starts_with("--- PASS:") {
            return "discriminating";
        }
        // pytest / jest / vitest / mocha: "<n> passed" with n >= 1.
        if let Some(n) = passed_count(line) {
            if n >= 1 {
                return "discriminating";
            }
        }
    }
    "ran_inert"
}

/// First run of ASCII digits in `s`, parsed (0 if none).
fn leading_count(s: &str) -> u64 {
    s.chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// The integer immediately preceding the word `passed` in a line, if any
/// (`"Tests: 12 passed, 3 total"` → 12).
fn passed_count(line: &str) -> Option<u64> {
    let idx = line.find("passed")?;
    let digits: String = line[..idx]
        .trim_end()
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse().ok()
}

/// Kill a timed-out validation command AND its descendants. With `process_group(0)`
/// the child is its own group leader (pgid == child pid), so `kill -KILL -<pgid>`
/// reaches every forked process — not just `sh`. Pure-std (shells out to `kill`);
/// `child.kill()` is the belt-and-suspenders fallback for the leader itself.
fn terminate_command_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pgid = child.id();
        let _ = StdCommand::new("kill")
            .arg("-KILL")
            .arg(format!("-{pgid}"))
            .status();
    }
    let _ = child.kill();
}

// ---------------------------------------------------------------------------
// Private: saga env pre-flight
// ---------------------------------------------------------------------------

/// For a saga validation: the env vars its spec needs that this process
/// doesn't have, or None when there's nothing missing / the spec can't be
/// read (then the command runs and fails loudly on its own).
fn saga_missing_env(root: &std::path::Path, v: &crate::types::Validation) -> Option<Vec<String>> {
    let rel = crate::commands::saga::spec_path_of(v)?;
    // Confine the spec path to the repo root before reading it — a tampered/
    // imported graph's `spec:` line could be `../../../etc/...` and this preflight
    // would otherwise open and parse a file outside the repo (`saga run` already
    // confines; this read had drifted from that guard).
    let confined = crate::repo::confine(root, std::path::Path::new(&rel))?;
    let spec = crate::saga::spec::load_spec_file(&root.join(confined)).ok()?;
    let missing = crate::saga::spec::missing_env(&spec);
    if missing.is_empty() {
        None
    } else {
        Some(missing)
    }
}

// ---------------------------------------------------------------------------
// Private: map validation result → VALIDATES edge inspection_status
// ---------------------------------------------------------------------------

fn validation_result_edge_status(validation_result: &str) -> &'static str {
    match validation_result {
        "passed" => "passing",
        "failed" => "failing",
        _ => "uninspected",
    }
}

#[cfg(test)]
mod tests {
    use super::proof_discrimination;

    #[test]
    fn discrimination_recognizes_real_runners_and_demotes_inert() {
        // Recognized passing runners → discriminating.
        assert_eq!(
            proof_discrimination("running 3 tests\ntest result: ok. 3 passed; 0 failed;"),
            "discriminating"
        );
        assert_eq!(
            proof_discrimination("=== 12 passed in 0.31s ==="),
            "discriminating"
        );
        assert_eq!(
            proof_discrimination("Tests:       5 passed, 5 total"),
            "discriminating"
        );
        assert_eq!(
            proof_discrimination("=== RUN   TestX\n--- PASS: TestX (0.00s)\nPASS\nok  pkg"),
            "discriminating"
        );
        // Inert: exit-0 with no assertion signal.
        assert_eq!(proof_discrimination(""), "ran_inert");
        assert_eq!(proof_discrimination("hello world"), "ran_inert");
        // Zero assertions is NOT discriminating.
        assert_eq!(
            proof_discrimination("test result: ok. 0 passed; 0 failed;"),
            "ran_inert"
        );
        assert_eq!(proof_discrimination("0 passed"), "ran_inert");
    }
}
