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

    let grounding = grounding_by_validation(&snapshot);
    drop(store);

    execute_and_record(
        &cwd,
        &to_run,
        timeout_secs,
        printer,
        ("intent_id", serde_json::json!(intent_id)),
        &marker,
        &grounding,
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
    let grounding = grounding_by_validation(&store.query_snapshot()?);
    drop(store);

    execute_and_record(
        &cwd,
        &to_run,
        timeout_secs,
        printer,
        ("scope", serde_json::json!("all")),
        &marker,
        &grounding,
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
    grounding: &std::collections::HashMap<String, Grounding>,
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
        // A hand-marked verdict is a HUMAN decision (`loom validation mark`) —
        // `loom validate` re-runs an intent's proofs, but it must never silently
        // overwrite a manual mark by running the command underneath it (the
        // re-run/re-block loop that drops a marked-passed saga back to blocked).
        // It stays put until the code changes (`loom sync` resets it to not_run)
        // or the operator re-marks it (`loom validation mark <id> --result …`,
        // including `--result not_run` to clear and force a fresh run).
        if manual_verdict_is_sticky(validation) {
            results.push(serde_json::json!({
                "validation_id": validation.id,
                "name":          validation.name,
                "result":        validation.last_result,
                "reason":        "kept — hand-marked result; `loom validate` won't overwrite a manual mark. Clear with `loom validation mark <id> --result not_run`, or it resets on `loom sync` when the code changes.",
            }));
            if !printer.json {
                println!(
                    "  = {} [{} — kept manual mark; not re-run]",
                    validation.name, validation.last_result
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

        // The built-in saga engine (`loom saga run`) consumes a LIVE target via
        // `{{ env.X }}` values passed at invocation. If they're missing here, the
        // proof CANNOT run — that is `blocked` (environment not ready), not
        // `failed` (code wrong). Running the command anyway would record a
        // dishonest failure and send the driver chasing a phantom code bug.
        // A saga whose command is anything ELSE (a self-contained script that
        // brings up its own target, waits for health, runs the chain, tears it
        // down) owns its environment — honor it like any other command and let
        // its exit code speak; the ambient-env precheck does not apply.
        if validation.validation_type == "saga"
            && saga_command_uses_builtin_engine(&validation.command)
        {
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
        let run_started_at = std::time::SystemTime::now();
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
        let raw_discrimination = if result != ValidationResult::Passed {
            "ran_inert"
        } else if validation.validation_type == "saga" {
            // A passing saga is a runtime proof against a live target: it
            // asserted each step's response, exactly as the `loom saga run`
            // engine path does (which stamps `discriminating`). A saga script's
            // output won't carry a unit-runner pass-string, so don't route it
            // through the runner-output heuristic — credit it directly. The
            // forgery guard below still demotes a print-only command posing as a saga.
            "discriminating"
        } else {
            proof_discrimination(&output)
        };
        // FORGERY GUARD: a runner pass-signal in stdout only earns EXECUTED-proven
        // if the command actually RAN a test. A command whose every segment is a
        // pure print/no-op (`echo 'N passed'`, `printf …`, `cat … ; true`) prints
        // the phrase a runner would, but executes nothing — it must NOT mint a
        // green executed-proven rung. Demote it to asserted-only (a real test
        // always invokes a runner/interpreter/script, which this detects).
        let forged_signal =
            raw_discrimination == "discriminating" && command_only_prints(&validation.command);
        // RELEVANCE GATE (test-type only): a passing test that does not REACH the
        // intent's grounded code (statically — by import graph, conftest, or naming
        // the locator symbol) cannot exercise its criterion. `pytest
        // test_irrelevant.py` passing `1+1==2` while grounded on mod.py is the
        // forge this closes. Whole-suite runners (`cargo test`, no named file) are
        // Unconfirmed → benefit of the doubt; only a NAMED test demonstrably
        // missing the grounding is demoted.
        let irrelevant_proof = raw_discrimination == "discriminating"
            && !forged_signal
            && validation.validation_type == "test"
            && grounding
                .get(&validation.id)
                .map(|g| proof_relevance(cwd, &validation.command, g, run_started_at))
                == Some(ProofRelevance::Irrelevant);
        let discrimination = if forged_signal || irrelevant_proof {
            "ran_inert"
        } else {
            raw_discrimination
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
            if new_result == "passed" && forged_signal {
                println!(
                    "    ⚠ passed but NON-EXECUTING: the command only PRINTS a test-runner \
                     pass-string — it runs NO test. A `test` proof must invoke a runner \
                     (pytest / cargo test / go test / node --test / …) against the grounded code; \
                     an echoed pass-line counts as ASSERTED-only, NOT executed-proven, and will NOT \
                     advance the Realized rung."
                );
            } else if new_result == "passed" && irrelevant_proof {
                println!(
                    "    ⚠ passed but IRRELEVANT: loom found no evidence the test exercises the \
                     grounded symbol. This is based on static import/symbol-usage analysis. For a \
                     DEFINITIVE answer, enable coverage: `LOOM_COVERAGE_FILE=<lcov-path> loom \
                     validate {}` — an LCOV report showing the grounded symbol's lines were \
                     executed will confirm it regardless of imports. If the test exercises the \
                     code indirectly (subprocess / e2e), record it as an `assertion` or `saga` \
                     validation instead of `test`.",
                    scope.1
                );
            } else if new_result == "passed"
                && discrimination == "ran_inert"
                && !validation.command.trim().is_empty()
            {
                println!(
                    "    ⚠ passed but NON-DISCRIMINATING: exit 0 but no recognized test-runner \
                     pass-signal in the output — counts as ASSERTED-only, NOT executed-proven, so it \
                     will NOT advance the Realized rung. Emit (or run a runner that emits) one of: \
                     `test result: ok. N passed` (cargo), `N passed` (pytest/jest/vitest), \
                     `N passing` (mocha), `# pass N` / `ℹ pass N` (node --test), `--- PASS:` (go), \
                     `Ran N tests` + `OK` (python unittest). Or make the test ASSERT ≥1 thing so a \
                     real runner reports it."
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
    // python stdlib unittest prints "Ran <n> test(s) ..." then a terminal "OK";
    // neither line alone is unambiguous, so require BOTH (n >= 1).
    let mut unittest_ran = false;
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
        // pytest / jest / vitest: "<n> passed" with n >= 1.
        if passed_count(line).is_some_and(|n| n >= 1) {
            return "discriminating";
        }
        // mocha: "<n> passing" with n >= 1 (NOT "passed" — distinct token).
        if count_before(line, "passing").is_some_and(|n| n >= 1) {
            return "discriminating";
        }
        // node:test / TAP summary: "# pass <n>" or "ℹ pass <n>" with n >= 1.
        if tap_pass_count(line).is_some_and(|n| n >= 1) {
            return "discriminating";
        }
        // python unittest: "Ran <n> test(s)" (n >= 1) ... then "OK" / "OK (…)".
        if line
            .strip_prefix("Ran ")
            .is_some_and(|rest| leading_count(rest) >= 1)
        {
            unittest_ran = true;
        }
        if unittest_ran && (line == "OK" || line.starts_with("OK ") || line.starts_with("OK(")) {
            return "discriminating";
        }
    }
    "ran_inert"
}

/// The integer immediately preceding `word` in `line`, if any
/// (`count_before("2 passing (4ms)", "passing")` → 2).
fn count_before(line: &str, word: &str) -> Option<u64> {
    let idx = line.find(word)?;
    line[..idx]
        .trim_end()
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .ok()
}

/// True when a command is PROVABLY non-executing — every segment (split on shell
/// sequencing) is a pure print / no-op builtin (`echo`/`printf`/`cat`/`true`/`:`).
/// Such a command can PRINT a runner's pass-string but ran no test, so it must not
/// earn EXECUTED-proven (the `echo 'N passed'` forgery). This is the line loom can
/// defend honestly: a bare print builtin CANNOT have run a test. A command that
/// invokes a process/interpreter/script (`sh -c …`, `pytest`, `./run.sh`) is given
/// the benefit of the doubt — loom cannot prove from stdout alone whether it ran a
/// real test, so the executed/asserted gate is a heuristic, not a security
/// boundary against a determined forger.
fn command_only_prints(command: &str) -> bool {
    const NOOP: &[&str] = &[
        "echo", "printf", "true", "false", ":", "cat", "test", "[", "sleep", "env", "head", "tail",
        "yes", "tee",
    ];
    let segments: Vec<&str> = command
        .split([';', '|', '&', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return false;
    }
    segments.iter().all(|seg| {
        // Skip leading `VAR=val` env-assignments, then take the command word.
        let word = seg
            .split_whitespace()
            .find(|t| !(t.contains('=') && t.split('=').next().is_some_and(is_env_var_name)));
        match word {
            None => true, // pure env-assignment segment runs nothing
            Some(w) => NOOP.contains(&w.rsplit('/').next().unwrap_or(w)),
        }
    })
}

fn is_env_var_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// How well a `test`-type proof can be confirmed to EXERCISE the intent's grounded
/// code, by static import graph. Defends the executed-proven rung against a
/// passing-but-IRRELEVANT test (`pytest test_irrelevant.py` that never touches the
/// grounded module) — which slips past the stdout pass-signal check and even the
/// `nonlocal_proof` smell (a co-located but irrelevant test is "local").
#[derive(PartialEq, Clone, Copy)]
enum ProofRelevance {
    /// A grounded file is reachable from the proof's named test files (transitively
    /// via imports), the grounded file IS the test target, or a grounded locator
    /// symbol is named in a test file → it plausibly exercises the code.
    Confirmed,
    /// We read >=1 of the proof's NAMED test files via the extractor and none
    /// reaches the grounding (nor names a locator) → it cannot exercise it.
    Irrelevant,
    /// The command names no resolvable source file (a whole-suite runner like
    /// `cargo test` / `go test ./...` / bare `pytest`), or nothing was readable →
    /// benefit of the doubt (the relevant test may be among those it discovers).
    Unconfirmed,
}

fn proof_relevance(
    root: &std::path::Path,
    command: &str,
    grounding: &Grounding,
    run_started_at: std::time::SystemTime,
) -> ProofRelevance {
    if grounding.files.is_empty() {
        return ProofRelevance::Unconfirmed;
    }
    let grounded: std::collections::HashSet<&str> =
        grounding.files.iter().map(String::as_str).collect();

    // ── TIER 1: Coverage data (definitive) ──────────────────────────────
    // If an LCOV/coverage report exists and is fresh for THIS validation run
    // (mtime at/after this command started), consult it. A grounded symbol whose
    // line range was EXECUTED is Confirmed regardless of imports/text. A symbol
    // whose range was NOT executed is Irrelevant — the definitive answer the
    // static gate could only approximate.
    if let Some(cov) = discover_coverage(root, run_started_at) {
        if !grounding.symbol_ranges.is_empty() {
            let mut any_executed = false;
            let mut any_not_executed = false;
            for (file, start, end) in &grounding.symbol_ranges {
                match cov.symbol_executed(file, *start, *end) {
                    CoverageVerdict::Executed => any_executed = true,
                    CoverageVerdict::NotExecuted => any_not_executed = true,
                    CoverageVerdict::FileNotInReport | CoverageVerdict::RangeNotInReport => {}
                }
            }
            if any_executed {
                return ProofRelevance::Confirmed;
            }
            // No symbol was executed, but at least one was instrumented and
            // NOT hit → the test ran but never executed the grounded behavior.
            if any_not_executed {
                return ProofRelevance::Irrelevant;
            }
            // File not in the coverage report, or symbol range not instrumented
            // → fall through to the static heuristic.
        }
    }

    // ── TIER 2: Static import + symbol-usage analysis (heuristic) ───────
    let named = command_source_files(root, command);
    if named.is_empty() {
        return ProofRelevance::Unconfirmed;
    }
    if named.iter().any(|f| grounded.contains(f.as_str())) {
        return ProofRelevance::Confirmed; // the grounded file IS the test target
    }
    let mut roots = named.clone();
    for f in &named {
        roots.extend(conftest_chain(root, f));
    }
    let reachable = transitive_imports(root, &roots);
    let file_reachable = reachable.iter().any(|f| grounded.contains(f.as_str()));

    let target_symbols: Vec<String> = {
        let mut set = std::collections::HashSet::new();
        for loc in &grounding.locators {
            let sym = crate::repo::last_identifier(loc);
            if !sym.is_empty() {
                set.insert(sym);
            }
        }
        set.into_iter().collect()
    };

    let mut raw_imports = 0usize;
    let mut resolved_imports = 0usize;
    let mut read_any = false;
    let mut symbol_used = false;
    for f in &named {
        let Ok(content) = std::fs::read_to_string(root.join(f)) else {
            continue;
        };
        read_any = true;
        raw_imports += count_raw_imports(&content, f);
        resolved_imports += crate::repo::extract_physical_facts(root, f, &content)
            .imports
            .len();
        if file_reachable && !symbol_used && !target_symbols.is_empty() {
            for sym in &target_symbols {
                if symbol_used_in_source_file(root, f, sym) {
                    symbol_used = true;
                    break;
                }
            }
        }
    }

    if file_reachable && symbol_used {
        return ProofRelevance::Confirmed;
    }
    if !read_any {
        return ProofRelevance::Unconfirmed;
    }
    if raw_imports > resolved_imports {
        return ProofRelevance::Unconfirmed;
    }
    ProofRelevance::Irrelevant
}

/// Count of import-like statements in `content`, by language — used to detect
/// imports loom could NOT resolve (raw count > resolved count), so an unresolvable
/// import (src-layout alias, dynamic import) never causes a false-demote.
fn count_raw_imports(content: &str, file: &str) -> usize {
    let ext = std::path::Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    content
        .lines()
        .map(str::trim_start)
        .filter(|t| match ext {
            "py" => t.starts_with("import ") || t.starts_with("from "),
            "rs" => t.starts_with("use ") || t.starts_with("pub use "),
            "go" => t.starts_with("import ") || t.starts_with("\t\"") || t.starts_with("\t_ \""),
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => {
                t.starts_with("import ") || t.contains("require(")
            }
            "rb" => t.starts_with("require") || t.starts_with("require_relative"),
            _ => false,
        })
        .count()
}

/// Strip string literals and line/block comments from source text, replacing
/// them with whitespace so identifier positions remain contiguous. This is a
/// language-agnostic best-effort filter: it catches the common "symbol name in a
/// string/comment" forgeries without needing a full per-language parser.
fn strip_literals_and_comments(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(d) = chars.next() {
                    if d == '\\' {
                        chars.next();
                    } else if d == '"' {
                        break;
                    }
                }
                out.push(' ');
            }
            '\'' => {
                while let Some(d) = chars.next() {
                    if d == '\\' {
                        chars.next();
                    } else if d == '\'' {
                        break;
                    }
                }
                out.push(' ');
            }
            '/' if chars.peek() == Some(&'/') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '#' => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next(); // consume '*'
                loop {
                    match chars.next() {
                        Some('*') if chars.peek() == Some(&'/') => {
                            chars.next();
                            break;
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

/// Remove import-like lines (and their continuations) from source text. This lets
/// us ask "is the symbol USED in the test body?" rather than "is it imported?".
/// Multi-line `from x import (\n  a,\n  b,\n)` and similar bracketed forms are
/// skipped as one statement.
fn remove_import_lines(content: &str, file: &str) -> String {
    let ext = std::path::Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let import_starts: &[&str] = match ext {
        "py" => &["from ", "import "],
        "rs" => &["use ", "extern "],
        "go" | "java" | "kt" => &["import "],
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" => &["import ", "require("],
        "c" | "cpp" | "h" | "hpp" => &["#include ", "import "],
        _ => &["import "],
    };
    let mut out = Vec::new();
    let mut bracket_depth = 0i64;
    let mut in_import = false;
    for line in content.lines() {
        let trim = line.trim_start();
        if in_import {
            for c in line.chars() {
                match c {
                    '(' | '[' | '{' => bracket_depth += 1,
                    ')' | ']' | '}' => bracket_depth = bracket_depth.saturating_sub(1),
                    _ => {}
                }
            }
            let ends = line.trim_end();
            if bracket_depth <= 0
                && !ends.ends_with(',')
                && !ends.ends_with('\\')
                && !ends.ends_with("from ")
            {
                in_import = false;
            }
            continue;
        }
        if import_starts.iter().any(|p| trim.starts_with(p)) {
            bracket_depth = 0;
            for c in line.chars() {
                match c {
                    '(' | '[' | '{' => bracket_depth += 1,
                    ')' | ']' | '}' => bracket_depth = bracket_depth.saturating_sub(1),
                    _ => {}
                }
            }
            let ends = line.trim_end();
            if bracket_depth > 0
                || ends.ends_with(',')
                || ends.ends_with('\\')
                || ends.ends_with("from ")
            {
                in_import = true;
            }
            continue;
        }
        out.push(line);
    }
    out.join("\n")
}

/// True if `symbol` appears as an identifier in `file` outside of import
/// statements and outside of string/comment literals.
fn symbol_used_in_source_file(root: &std::path::Path, file: &str, symbol: &str) -> bool {
    if symbol.is_empty() {
        return false;
    }
    let Ok(content) = std::fs::read_to_string(root.join(file)) else {
        return false;
    };
    let body = remove_import_lines(&content, file);
    let code_text = strip_literals_and_comments(&body);
    crate::db::queries::symbol_match::contains_identifier_word(&code_text, symbol)
}

// ─────────────────────────────────────────────────────────────────────
// Coverage-based relevance (Tier 1 — definitive)
// ─────────────────────────────────────────────────────────────────────

/// A parsed LCOV report: file → (instrumented lines, executed lines).
/// Built from `SF:`/`DA:` records. Tracks ALL DA lines (not just hits) so we
/// can distinguish "instrumented but not executed" (→ NotExecuted) from "not
/// instrumented at all" (→ RangeNotInReport). Definitive for the question
/// "was line N of file F executed during the test run?".
struct CoverageReport {
    /// file → (all instrumented line numbers, executed line numbers)
    files: std::collections::HashMap<
        String,
        (
            std::collections::HashSet<usize>,
            std::collections::HashSet<usize>,
        ),
    >,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoverageVerdict {
    /// At least one line in the symbol's range was executed.
    Executed,
    /// The file is in the report and the symbol's range has instrumented lines,
    /// but NONE were executed — the test ran but never hit the grounded code.
    NotExecuted,
    /// The file is in the report but the symbol's line range has NO instrumented
    /// lines (the coverage tool didn't track this region — e.g., stripped or
    /// generated code). Can't conclude either way.
    RangeNotInReport,
    /// The grounded file is not in the coverage report at all.
    FileNotInReport,
}

impl CoverageReport {
    fn symbol_executed(&self, file: &str, start: usize, end: usize) -> CoverageVerdict {
        let Some((instrumented, executed)) = self.files.get(file) else {
            return CoverageVerdict::FileNotInReport;
        };
        // Is any line in the symbol's range instrumented?
        let range_instrumented = (start..=end).any(|ln| instrumented.contains(&ln));
        if !range_instrumented {
            return CoverageVerdict::RangeNotInReport;
        }
        // Is any line in the range executed?
        if (start..=end).any(|ln| executed.contains(&ln)) {
            return CoverageVerdict::Executed;
        }
        // The range was instrumented but no line was hit → the test ran but
        // never executed this symbol's code.
        CoverageVerdict::NotExecuted
    }
}

/// Parse an LCOV.info-format coverage report. Handles `SF:<path>` and
/// `DA:<line>,<count>` records. Paths are normalized to repo-relative.
fn parse_lcov(root: &std::path::Path, content: &str) -> CoverageReport {
    let mut files: std::collections::HashMap<
        String,
        (
            std::collections::HashSet<usize>,
            std::collections::HashSet<usize>,
        ),
    > = std::collections::HashMap::new();
    let mut current_file: Option<String> = None;
    for line in content.lines() {
        if let Some(path) = line.strip_prefix("SF:") {
            let rel = crate::repo::confine(root, std::path::Path::new(path.trim()))
                .unwrap_or_else(|| path.trim().to_string());
            current_file = Some(rel.clone());
            files.entry(rel).or_default();
        } else if let Some(rest) = line.strip_prefix("DA:") {
            // DA:<line>,<count>[,<checksum>]
            let mut parts = rest.split(',');
            if let (Some(line_str), Some(count_str)) = (parts.next(), parts.next()) {
                if let (Ok(ln), Ok(count)) = (
                    line_str.trim().parse::<usize>(),
                    count_str.trim().parse::<u64>(),
                ) {
                    if let Some(f) = &current_file {
                        let entry = files.entry(f.clone()).or_default();
                        entry.0.insert(ln); // instrumented
                        if count > 0 {
                            entry.1.insert(ln); // executed
                        }
                    }
                }
            }
        }
    }
    CoverageReport { files }
}

/// Discover a coverage report for this repo, consulting (in order):
///   1. `LOOM_COVERAGE_FILE` env var (explicit override)
///   2. Common LCOV paths relative to root (coverage.lcov, lcov.info, etc.)
///
/// Returns None if no report is found or it can't be parsed.
fn discover_coverage(
    root: &std::path::Path,
    not_before: std::time::SystemTime,
) -> Option<CoverageReport> {
    let candidates: Vec<std::path::PathBuf> =
        if let Some(env_path) = std::env::var_os("LOOM_COVERAGE_FILE") {
            vec![std::path::PathBuf::from(env_path)]
        } else {
            let mut v = Vec::new();
            for name in &["coverage.lcov", "lcov.info", "coverage.info", "cov.info"] {
                v.push(root.join(name));
            }
            // target/tarpaulin/coverage.lcov (Rust), htmlcov/coverage.lcov (Python)
            v.push(root.join("target/tarpaulin/coverage.lcov"));
            v.push(root.join("htmlcov/coverage.lcov"));
            // .coverage/ (custom convention)
            v.push(root.join(".coverage/coverage.lcov"));
            v
        };
    for path in &candidates {
        let Some(modified) = std::fs::metadata(path).ok().and_then(|m| m.modified().ok()) else {
            continue;
        };
        if modified < not_before {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(path) {
            // Basic sanity: must contain at least one SF: record.
            if content.contains("SF:") {
                return Some(parse_lcov(root, &content));
            }
        }
    }
    None
}

/// Existing source files under `root` named as path tokens in `command`.
fn command_source_files(root: &std::path::Path, command: &str) -> Vec<String> {
    const EXTS: &[&str] = &[
        ".py", ".rs", ".go", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".rb",
        ".java", ".kt", ".swift", ".dart",
    ];
    let mut out: Vec<String> = Vec::new();
    for raw in command.split_whitespace() {
        let tok =
            raw.trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '/' | '.' | '_' | '-'));
        if tok.is_empty() || tok.starts_with('-') {
            continue;
        }
        // strip a rust `path::module::test` / pytest `file::node` selector tail.
        let path_part = tok.split("::").next().unwrap_or(tok);
        if !EXTS.iter().any(|e| path_part.ends_with(e)) {
            continue;
        }
        if let Some(rel) = crate::repo::confine(root, std::path::Path::new(path_part)) {
            if root.join(&rel).is_file() && !out.contains(&rel) {
                out.push(rel);
            }
        }
    }
    out
}

/// `conftest.py` in the file's directory and each ancestor up to the root.
fn conftest_chain(root: &std::path::Path, file: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut dir = std::path::Path::new(file).parent();
    while let Some(d) = dir {
        let rel = if d.as_os_str().is_empty() {
            "conftest.py".to_string()
        } else {
            format!("{}/conftest.py", d.to_string_lossy())
        };
        if root.join(&rel).is_file() {
            out.push(rel);
        }
        if d.as_os_str().is_empty() {
            break;
        }
        dir = d.parent();
    }
    out
}

/// Transitive set of files reachable from `roots` via static imports (BFS, capped).
fn transitive_imports(
    root: &std::path::Path,
    roots: &[String],
) -> std::collections::HashSet<String> {
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<String> = roots.iter().cloned().collect();
    let mut steps = 0usize;
    while let Some(f) = queue.pop_front() {
        if !visited.insert(f.clone()) {
            continue;
        }
        steps += 1;
        if steps > 500 {
            break; // runaway guard on a pathological import graph
        }
        if let Ok(content) = std::fs::read_to_string(root.join(&f)) {
            for imp in crate::repo::extract_physical_facts(root, &f, &content).imports {
                if !visited.contains(&imp) {
                    queue.push_back(imp);
                }
            }
        }
    }
    visited
}

/// Per-validation grounding context: the grounded files, locators, and the
/// line ranges of the grounded symbols (from SymbolFacts). The line ranges
/// let the coverage-based relevance gate answer "was the grounded symbol's
/// code actually EXECUTED?" definitively — the only resolution that doesn't
/// guess via static import/text analysis.
#[derive(Clone, Default)]
struct Grounding {
    files: Vec<String>,
    locators: Vec<String>,
    /// (file, line_start, line_end) for each grounded symbol whose range
    /// could be resolved from the CodeFile's SymbolFacts.
    symbol_ranges: Vec<(String, usize, usize)>,
}

/// Map each validation to the grounding context of the intent it proves.
/// Carries symbol line ranges (from tree-sitter/heuristic SymbolFacts) so the
/// relevance gate can consult coverage data when available.
fn grounding_by_validation(
    snapshot: &crate::db::queries::QuerySnapshot,
) -> std::collections::HashMap<String, Grounding> {
    let mut facts_by_cf: std::collections::HashMap<&str, &[crate::types::SymbolFact]> =
        std::collections::HashMap::new();
    for cf in &snapshot.codefiles {
        facts_by_cf.insert(cf.id.as_str(), &cf.symbol_facts);
    }

    let mut by_intent: std::collections::HashMap<&str, Grounding> =
        std::collections::HashMap::new();
    for im in &snapshot.implements {
        let g = by_intent.entry(im.intent_id.as_str()).or_default();
        if !g.files.contains(&im.codefile_path) {
            g.files.push(im.codefile_path.clone());
        }
        if !im.locator.trim().is_empty() && !g.locators.contains(&im.locator) {
            g.locators.push(im.locator.clone());
        }
        // Resolve the locator to a symbol line range via the CodeFile's facts.
        if let Some(facts) = facts_by_cf.get(im.codefile_id.as_str()) {
            let loc_ident = crate::repo::last_identifier(&im.locator);
            if !loc_ident.is_empty() {
                for f in *facts {
                    if f.name == loc_ident && f.line_end > f.line_start {
                        g.symbol_ranges
                            .push((im.codefile_path.clone(), f.line_start, f.line_end));
                    }
                }
            }
        }
    }
    let mut out = std::collections::HashMap::new();
    for ve in &snapshot.validates {
        if let Some(g) = by_intent.get(ve.intent_id.as_str()) {
            out.insert(ve.validation_id.clone(), g.clone());
        }
    }
    out
}

/// node:test / TAP pass summary: a line like `# pass 2` or `ℹ pass 2` → 2.
fn tap_pass_count(line: &str) -> Option<u64> {
    let rest = line
        .trim_start_matches(['#', 'ℹ', ' '])
        .strip_prefix("pass ")?;
    Some(leading_count(rest))
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
    count_before(line, "passed")
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

/// True when a saga validation's command drives the built-in engine (`loom saga
/// run`), which reads its target from ambient `{{ env.X }}` values at invocation
/// — the only case the missing-env precheck guards. Any other command (a
/// self-contained script that brings its own target up) owns its environment, so
/// it runs like a normal proof and its exit code is the verdict.
fn saga_command_uses_builtin_engine(command: &str) -> bool {
    command.contains("loom saga run")
}

/// True when this validation's CURRENT verdict was recorded by a hand-mark
/// (`loom validation mark`) rather than an executor run — a sticky human
/// decision `loom validate` must not overwrite by re-running. It rests on the
/// documented invariant that a hand-mark stamps `last_run` but NEVER
/// `last_executed_run`, while an executor run stamps both to the same instant:
/// so a settled verdict whose two timestamps disagree was last set by hand.
/// not_run / blocked are handled on their own branches, so only passed/failed
/// can be sticky here.
fn manual_verdict_is_sticky(v: &crate::types::Validation) -> bool {
    (v.last_result == "passed" || v.last_result == "failed") && v.last_run != v.last_executed_run
}

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
    use super::{command_only_prints, proof_discrimination};

    #[test]
    fn forged_print_commands_run_nothing_real_runners_do() {
        // Pure print / no-op commands run no test — they can only FORGE a pass-string.
        for forged in [
            "echo '1 passed in 0.01s'",
            "printf 'test result: ok. 1 passed'",
            "cat results.txt; echo '1 passed'",
            "true; echo '--- PASS: TestX'",
            "FOO=1 echo '2 passing'",
            ":",
        ] {
            assert!(
                command_only_prints(forged),
                "must be flagged as non-executing: {forged}"
            );
        }
        // Real test invocations run something — never flagged.
        for real in [
            "pytest test_m.py -q",
            "python3 -m pytest",
            "cargo test",
            "go test ./...",
            "node --test",
            "./run_tests.sh",
            "make test",
            "npm test && echo done",
        ] {
            assert!(!command_only_prints(real), "must NOT be flagged: {real}");
        }
    }

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

    #[test]
    fn lcov_parser_reads_sf_and_da_records() {
        let root = std::path::Path::new("/tmp");
        let lcov = "SF:src/lib.rs\nDA:1,1\nDA:2,0\nDA:3,1\nend_of_record\nSF:src/other.rs\nDA:10,1\nend_of_record\n";
        let report = super::parse_lcov(root, lcov);
        // src/lib.rs: instrumented = {1,2,3}, executed = {1,3}
        let (inst, exec) = report.files.get("src/lib.rs").unwrap();
        assert!(inst.contains(&1) && inst.contains(&2) && inst.contains(&3));
        assert!(exec.contains(&1) && exec.contains(&3));
        assert!(!exec.contains(&2)); // line 2 was instrumented but not executed
                                     // A file not in the report.
        assert!(!report.files.contains_key("src/absent.rs"));
    }

    #[test]
    fn coverage_verdict_distinguishes_executed_not_executed_range_absent() {
        // mod.rs: line 5 instrumented+executed, line 10 instrumented+not-executed.
        let mut inst = std::collections::HashSet::new();
        inst.insert(5);
        inst.insert(10);
        let mut exec = std::collections::HashSet::new();
        exec.insert(5); // line 5 hit, line 10 not
        let mut m = std::collections::HashMap::new();
        m.insert("mod.rs".to_string(), (inst, exec));
        let report = super::CoverageReport { files: m };

        // Symbol on lines 5–7: line 5 was hit → Executed.
        assert_eq!(
            report.symbol_executed("mod.rs", 5, 7),
            super::CoverageVerdict::Executed
        );
        // Symbol on lines 10–12: line 10 instrumented but not hit → NotExecuted.
        assert_eq!(
            report.symbol_executed("mod.rs", 10, 12),
            super::CoverageVerdict::NotExecuted
        );
        // Symbol on lines 20–25: no instrumented lines in range → RangeNotInReport.
        assert_eq!(
            report.symbol_executed("mod.rs", 20, 25),
            super::CoverageVerdict::RangeNotInReport
        );
        // File not in the report at all → FileNotInReport.
        assert_eq!(
            report.symbol_executed("other.rs", 1, 5),
            super::CoverageVerdict::FileNotInReport
        );
    }
}
