//! `loom saga` — the consumer plane.
//!
//! `add` declares the proof: a Validation node (type=saga) VALIDATES-linked to
//! every step's intent, the RELATES_TO path edges between consecutive step
//! intents (uninspected — execution earns them), and the spec file registered
//! as a CodeFile so it travels in the export and counts in coverage.
//!
//! `run` executes the chain (DB closed while HTTP runs, mirroring `loom
//! validate`'s lock discipline) and translates per-step outcomes into graph
//! verdicts — the failure-semantics contract:
//!   - pairs of consecutive steps that BOTH ran and passed → their RELATES_TO
//!     edge goes `passing` with runtime evidence (execution, not just reading);
//!   - the boundary into the failing step → `failing`, evidence = the exact
//!     broken expectation ("expected 200, got 502");
//!   - pairs beyond the failure → UNTOUCHED (never reached is not failing);
//!   - the Validation node + all its VALIDATES edges carry the run verdict.

use anyhow::{Context, Result};
use uuid::Uuid;

use crate::cli::SagaCmd;
use crate::db::queries::{
    get_codefile_by_id_or_path, get_or_create_relates_to, get_validation, insert_codefile,
    insert_validates, insert_validation, list_all_validates, list_validations, resolve_intent,
    resolve_validation, set_validates_status_for_validation, stamp_relates_to_runtime,
    update_validation_result,
};
use crate::db::schema::role;
use crate::db::{ensure_initialized, GrafeoDb};
use crate::output::Printer;
use crate::saga::spec::{load_spec_file, SagaSpec};
use crate::saga::{run_saga, SagaRunReport};
use crate::types::{CodeFile, Validation};

pub fn run(cmd: SagaCmd, printer: &Printer) -> Result<()> {
    match cmd {
        SagaCmd::Add { file } => add(&file, printer),
        SagaCmd::Run { saga } => execute(&saga, printer),
        SagaCmd::List => list(printer),
    }
}

// ---------------------------------------------------------------------------
// add — declare the proof
// ---------------------------------------------------------------------------

fn add(file: &str, printer: &Printer) -> Result<()> {
    crate::gate::acting_in_lane(
        "register a saga proof",
        &[role::BUILDER, role::VALIDATOR],
        None,
    )?;
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    let rel = relative_to_root(file, &cwd)?;
    let spec = load_spec_file(&cwd.join(&rel))?;

    // Resolve every step's intent binding up front — a typo fails the add,
    // not a run three weeks later.
    let step_intents = resolve_step_intents(&db, &spec)?;

    let now = chrono::Utc::now().to_rfc3339();

    // Atomic declaration: the Validation, its VALIDATES links, the path
    // edges, and the spec's CodeFile registration land together — a saga
    // half-declared (proof node without its path) would mislead the compass.
    let required_env = crate::saga::spec::required_env(&spec);
    let (validation_id, created, linked, path_edges, registered_spec) =
        crate::db::with_transaction(&db, || {
    // The Validation node, keyed by the saga's name. Re-adding reconciles.
    let existing = list_validations(&db)?
        .into_iter()
        .find(|v| v.name == spec.saga);
    let command = format!("loom saga run {rel}");
    let env_line = if required_env.is_empty() {
        String::new()
    } else {
        // Travels in the node so list/show/next surface the dependency without
        // re-parsing the spec: a saga consumes a LIVE target, and the values
        // are passed at invocation, never stored in the graph.
        format!("\nrequires env: {}", required_env.join(", "))
    };
    let description = format!(
        "{}{}Consumer saga proof — {} step(s), run by the built-in engine.\nspec:{rel}{env_line}",
        spec.description.trim(),
        if spec.description.trim().is_empty() { "" } else { "\n" },
        spec.steps.len(),
    );
    let (validation_id, created) = match existing {
        Some(v) => {
            if v.validation_type != "saga" {
                anyhow::bail!(
                    "A validation named '{}' already exists with type '{}'. \
                     Saga names share the validation namespace — rename the saga or that validation.",
                    spec.saga, v.validation_type
                );
            }
            crate::db::queries::update_validation_definition(
                &db, &v.id, Some(&command), Some(&description),
            )?;
            (v.id, false)
        }
        None => {
            let id = Uuid::new_v4().to_string();
            insert_validation(&db, &Validation {
                id: id.clone(),
                name: spec.saga.clone(),
                description,
                validation_type: "saga".to_string(),
                command,
                last_run: String::new(),
                last_result: "not_run".to_string(),
            })?;
            (id, true)
        }
    };

    // VALIDATES: one edge per distinct step intent (missing ones only).
    let already: std::collections::HashSet<String> = list_all_validates(&db)?
        .into_iter()
        .filter(|e| e.validation_id == validation_id)
        .map(|e| e.intent_id)
        .collect();
    let mut linked = 0usize;
    let mut seen = std::collections::HashSet::new();
    for (iid, _) in &step_intents {
        if seen.insert(iid.clone()) && !already.contains(iid) {
            insert_validates(&db, &validation_id, iid, "", &now)?;
            linked += 1;
        }
    }

    // The intent path: RELATES_TO between consecutive distinct step intents,
    // created uninspected — declaring the path is structure; green is earned
    // by running.
    let mut path_edges = 0usize;
    for pair in step_intents.windows(2) {
        let (a, b) = (&pair[0].0, &pair[1].0);
        if a != b {
            get_or_create_relates_to(&db, a, b, &now)?;
            path_edges += 1;
        }
    }

    // The spec itself is part of the graph's physical plane.
    let mut registered_spec = false;
    if get_codefile_by_id_or_path(&db, &rel)?.is_none() {
        let abs = cwd.join(&rel);
        let last_modified = crate::repo::mtime_rfc3339(&abs).ok_or_else(|| anyhow::anyhow!(
            "Cannot read mtime for {} — ensure the spec file exists under the graph root, \
             or re-register: `loom saga add <file>`.",
            abs.display()
        ))?;
        let content_hash = std::fs::read(&abs)
            .map(|b| crate::repo::content_hash(&b))
            .with_context(|| format!("Cannot read bytes for {}", abs.display()))?;
        insert_codefile(&db, &CodeFile {
            id: Uuid::new_v4().to_string(),
            path: rel.clone(),
            language: "yaml".to_string(),
            last_modified,
            imports: Vec::new(),
            content_hash,
        })?;
        registered_spec = true;
    }
    Ok((validation_id, created, linked, path_edges, registered_spec))
    })?;

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "saga": spec.saga,
            "validation_id": validation_id,
            "created": created,
            "spec": rel,
            "steps": spec.steps.len(),
            "intents": step_intents.iter().map(|(id, name)| serde_json::json!({
                "id": id, "name": name,
            })).collect::<Vec<_>>(),
            "validates_linked": linked,
            "path_edges_ensured": path_edges,
            "spec_registered_as_codefile": registered_spec,
            "requires_env": required_env,
            "next_steps": [format!("Run it: `{}`.", run_invocation(&spec.saga, &required_env))],
        }));
    } else {
        println!(
            "✓ Saga '{}' {}  ({} step(s), spec: {rel})",
            spec.saga,
            if created { "registered" } else { "reconciled" },
            spec.steps.len(),
        );
        for (i, (_, name)) in step_intents.iter().enumerate() {
            println!("  {}. {} → intent '{}'", i + 1, spec.steps[i].name, name);
        }
        println!("  VALIDATES edges added: {linked} · path RELATES_TO ensured: {path_edges}");
        if registered_spec {
            println!("  Spec registered as a CodeFile — ground it under a consumer-journeys intent when you have one.");
        }
        if !required_env.is_empty() {
            println!(
                "  Requires env at invocation (the live target — never stored in the graph): {}",
                required_env.join(", ")
            );
        }
        println!("  → Run it: `{}`", run_invocation(&spec.saga, &required_env));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// run — execute and stamp
// ---------------------------------------------------------------------------

fn execute(arg: &str, printer: &Printer) -> Result<()> {
    let agent = crate::gate::acting_in_lane(
        "run a saga proof",
        &[role::VALIDATOR],
        None,
    )?;
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;

    // Phase 1 (DB open): locate the spec, resolve every binding.
    let rel = if cwd.join(arg).is_file() || std::path::Path::new(arg).is_file() {
        relative_to_root(arg, &cwd)?
    } else {
        let vid = resolve_validation(&db, arg)?;
        let v = get_validation(&db, &vid)?
            .ok_or_else(|| anyhow::anyhow!(
                "Validation '{vid}' not found. Run `loom saga list` (or `loom validation list`)."
            ))?;
        if v.validation_type != "saga" {
            anyhow::bail!(
                "'{}' is a {} validation, not a saga. Run it via `loom validate <intent>`.",
                v.name, v.validation_type
            );
        }
        spec_path_of(&v).ok_or_else(|| anyhow::anyhow!(
            "Saga validation '{}' has no `spec:` line in its description — \
             re-register it: `loom saga add <file>`.", v.name
        ))?
    };
    let spec = load_spec_file(&cwd.join(&rel))?;

    let validation = list_validations(&db)?
        .into_iter()
        .find(|v| v.name == spec.saga && v.validation_type == "saga")
        .ok_or_else(|| anyhow::anyhow!(
            "Saga '{}' is not registered in the graph yet. Run `loom saga add {rel}` first.",
            spec.saga
        ))?;
    let step_intents = resolve_step_intents(&db, &spec)?;

    // Pre-flight: a saga consumes a LIVE target, and its `{{ env.X }}` values
    // arrive at invocation. Missing values are an ENVIRONMENT problem, not a
    // failed proof — refuse to run (nothing is stamped) and say exactly how to
    // invoke, instead of failing on the first reference mid-chain.
    let missing = crate::saga::spec::missing_env(&spec);
    if !missing.is_empty() {
        anyhow::bail!(
            "Saga '{name}' needs environment value(s) this invocation didn't set: {missing}.\n\
             The spec references them as {{{{ env.<NAME> }}}} — pass them on the command line:\n\
             \n  {invocation}\n\
             \n(The values point at the LIVE target the consumer talks to; loom never stores them \
             in the graph.) Nothing was run or recorded. If the target cannot run yet at all, \
             record that honestly instead:\n\
             \n  loom validation mark {name} --result blocked --reason \"waiting on <what>\"",
            name = spec.saga,
            missing = missing.join(", "),
            invocation = run_invocation(&spec.saga, &missing),
        );
    }

    // Phase 2 (DB CLOSED): consume the live surface. HTTP can be slow and the
    // graph lock must not be held across it (same discipline as `loom validate`).
    drop(db);
    let report = run_saga(&spec)?;

    // Phase 3 (DB reopened): translate outcomes into graph verdicts.
    let db = GrafeoDb::open(&db_file)?;
    let now = chrono::Utc::now().to_rfc3339();
    let result = if report.passed { "passed" } else { "failed" };
    update_validation_result(&db, &validation.id, result, &now)?;

    let summary = match report.failure() {
        None => format!(
            "saga {}: all {} step(s) passed at {now}",
            report.saga, report.total_steps
        ),
        Some(f) => format!(
            "saga {} failed at step {}/{} ('{}', {} {}): {}. Steps before it passed; steps after were never reached.",
            report.saga, f.step, report.total_steps, f.name, f.method, f.url, f.detail
        ),
    };
    set_validates_status_for_validation(
        &db,
        &validation.id,
        if report.passed { "passing" } else { "failing" },
        &summary,
    )?;

    // The path stamps: consecutive distinct step intents among EXECUTED steps.
    let mut stamped_passing = 0usize;
    let mut stamped_failing = 0usize;
    for i in 0..report.executed.saturating_sub(1) {
        let (a_id, a_name) = &step_intents[i];
        let (b_id, b_name) = &step_intents[i + 1];
        if a_id == b_id {
            continue;
        }
        let (o_a, o_b) = (&report.outcomes[i], &report.outcomes[i + 1]);
        let criterion = format!(
            "consumer saga '{}': step '{}' ({}) feeds step '{}' ({}) and the chain executes end-to-end",
            report.saga, o_a.name, a_name, o_b.name, b_name
        );
        get_or_create_relates_to(&db, a_id, b_id, &now)?;
        if o_a.passed && o_b.passed {
            let evidence = format!(
                "runtime: saga '{}' run {now}: step {} ('{}' {} {}) → step {} ('{}' {} {}) both passed against the live surface",
                report.saga, o_a.step, o_a.name, o_a.method, o_a.url,
                o_b.step, o_b.name, o_b.method, o_b.url,
            );
            stamp_relates_to_runtime(&db, a_id, b_id, "passing", &criterion, &evidence, 0.95, &agent, &now)?;
            stamped_passing += 1;
        } else if o_a.passed && !o_b.passed {
            let evidence = format!(
                "runtime: saga '{}' run {now}: step {} ('{}' {} {}) failed — {}",
                report.saga, o_b.step, o_b.name, o_b.method, o_b.url, o_b.detail,
            );
            stamp_relates_to_runtime(&db, a_id, b_id, "failing", &criterion, &evidence, 0.95, &agent, &now)?;
            stamped_failing += 1;
        }
    }

    // The run verdict moves the phase — anchor while the session is still
    // open (the pulse reads the graph), THEN close it: `process::exit` skips
    // destructors and grafeo persists on Drop, so the session must be closed
    // before exiting or the failure stamps never reach disk.
    print_report(&report, stamped_passing, stamped_failing, &db, printer)?;
    drop(db);

    if !report.passed {
        // Non-zero exit: `loom validate` (and CI) read this as the proof failing.
        std::process::exit(1);
    }
    Ok(())
}

fn print_report(
    report: &SagaRunReport,
    stamped_passing: usize,
    stamped_failing: usize,
    db: &dyn crate::db::LoomDb,
    printer: &Printer,
) -> Result<()> {
    // Result-sensitive anchor for both modes.
    let next_step = if report.passed {
        "`loom next --mode validate` continues the proof queue".to_string()
    } else {
        "the failing edge carries the evidence: `loom next --mode fix` will serve it.".to_string()
    };
    if printer.json {
        printer.print_json(&crate::output::with_anchor(serde_json::json!({
            "saga": report.saga,
            "result": if report.passed { "passed" } else { "failed" },
            "executed": report.executed,
            "total_steps": report.total_steps,
            "steps": report.outcomes,
            "relates_to_stamped_passing": stamped_passing,
            "relates_to_stamped_failing": stamped_failing,
        }), db, &next_step)?);
        return Ok(());
    }
    for o in &report.outcomes {
        let mark = if o.passed { "✓" } else { "✗" };
        println!("  {} {}. {} ({} {}) — {}", mark, o.step, o.name, o.method, o.url, o.detail);
        for (var, val) in &o.captured {
            println!("      captured {var} = {val}");
        }
    }
    for skipped in report.executed + 1..=report.total_steps {
        println!("  · {skipped}. (never reached)");
    }
    println!();
    if report.passed {
        println!(
            "✓ Saga '{}' passed ({}/{} steps). Runtime evidence stamped on {} path edge(s).",
            report.saga, report.executed, report.total_steps, stamped_passing
        );
    } else {
        let f = report.failure().expect("failed report has a failing step");
        println!(
            "✗ Saga '{}' FAILED at step {}/{} ('{}').",
            report.saga, f.step, report.total_steps, f.name
        );
        println!(
            "  {} path edge(s) stamped passing (they ran), {} stamped failing (the broken boundary).",
            stamped_passing, stamped_failing
        );
    }
    crate::output::print_anchor(db, &next_step)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let db_file = ensure_initialized(&cwd)?;
    let db = GrafeoDb::open(&db_file)?;
    let sagas: Vec<Validation> = list_validations(&db)?
        .into_iter()
        .filter(|v| v.validation_type == "saga")
        .collect();
    if printer.json {
        let rows: Vec<serde_json::Value> = sagas.iter().map(|v| serde_json::json!({
            "id": v.id,
            "name": v.name,
            "spec": spec_path_of(v),
            "requires_env": required_env_of(v),
            "run_with": run_invocation(&v.name, &required_env_of(v)),
            "last_result": v.last_result,
            "last_run": v.last_run,
        })).collect();
        printer.print_json(&rows);
    } else if sagas.is_empty() {
        println!("(no sagas registered — `loom saga add <spec.yaml>`)");
    } else {
        println!("  {:<10}  {:<28}  {:<32}  last run", "RESULT", "NAME", "SPEC");
        println!("  {}", "-".repeat(96));
        for v in &sagas {
            println!(
                "  [{:<8}]  {:<28}  {:<32}  {}",
                v.last_result,
                v.name,
                spec_path_of(v).unwrap_or_else(|| "?".to_string()),
                if v.last_run.is_empty() { "(never)" } else { &v.last_run },
            );
            let env = required_env_of(v);
            if !env.is_empty() {
                println!("              run with: {}", run_invocation(&v.name, &env));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Resolve each step's `intent:` binding to (intent_id, intent_name), in step
/// order. Fails on the first unknown/ambiguous binding, naming the step.
fn resolve_step_intents(
    db: &GrafeoDb,
    spec: &SagaSpec,
) -> Result<Vec<(String, String)>> {
    let mut out = Vec::with_capacity(spec.steps.len());
    for (i, step) in spec.steps.iter().enumerate() {
        let iid = resolve_intent(db, &step.intent).with_context(|| {
            format!(
                "step {} ('{}'): cannot resolve intent '{}' — every step must bind to an existing intent \
                 (create it first: `loom intent add`)",
                i + 1, step.name, step.intent
            )
        })?;
        let name = crate::db::queries::get_intent(db, &iid)?
            .map(|x| x.name)
            .unwrap_or_else(|| iid.clone());
        out.push((iid, name));
    }
    Ok(out)
}

/// The exact command line to run a saga, with required env vars as
/// placeholders to fill: `BASE_URL=<value> loom saga run checkout-flow`.
fn run_invocation(saga_name: &str, env_vars: &[String]) -> String {
    let prefix: String = env_vars.iter().map(|v| format!("{v}=<value> ")).collect();
    format!("{prefix}loom saga run {saga_name}")
}

/// The spec path recorded in a saga validation's description (`spec:<path>`).
pub fn spec_path_of(v: &Validation) -> Option<String> {
    v.description
        .lines()
        .find_map(|l| l.strip_prefix("spec:"))
        .map(|s| s.trim().to_string())
}

/// The env vars recorded in a saga validation's description
/// (`requires env: A, B`) — readable without re-parsing the spec file.
pub fn required_env_of(v: &Validation) -> Vec<String> {
    v.description
        .lines()
        .find_map(|l| l.strip_prefix("requires env:"))
        .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default()
}

/// Normalize a user-supplied path to the graph root (how CodeFiles are keyed).
fn relative_to_root(file: &str, root: &std::path::Path) -> Result<String> {
    let abs = if std::path::Path::new(file).is_absolute() {
        std::path::PathBuf::from(file)
    } else if root.join(file).exists() {
        root.join(file)
    } else {
        std::env::current_dir()?.join(file)
    };
    let abs = std::fs::canonicalize(&abs)
        .with_context(|| format!("Saga spec '{}' not found", file))?;
    let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    Ok(abs
        .strip_prefix(&root_canon)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| abs.display().to_string()))
}
