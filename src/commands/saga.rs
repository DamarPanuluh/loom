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
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::output::Printer;
use crate::saga::diagnose::{diagnose_missing_env, diagnose_report, SagaDiagnosis};
use crate::saga::spec::{load_spec_file, SagaSpec};
use crate::saga::{run_saga, SagaRunReport};
use crate::types::{CodeFile, Intent, Validation};

pub fn run(cmd: SagaCmd, printer: &Printer) -> Result<()> {
    match cmd {
        SagaCmd::Add {
            file,
            spawn_missing,
            under,
        } => add_sqlite(&file, spawn_missing, under.as_deref(), printer),
        SagaCmd::Run { saga } => execute_sqlite(&saga, printer),
        SagaCmd::Diagnose { saga } => diagnose_sqlite(&saga, printer),
        SagaCmd::List => list(printer),
    }
}

// ---------------------------------------------------------------------------
// add — declare the proof
// ---------------------------------------------------------------------------

fn add_sqlite(
    file: &str,
    spawn_missing: bool,
    under: Option<&str>,
    printer: &Printer,
) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::ADD_SAGA, None)?;
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;

    if spawn_missing {
        crate::gate::acting_in_lane(&crate::gate::lane::SPAWN_JOURNEY_INTENTS, None)?;
        store.ensure_owned("spawn planned intents from a journey (a promise to build the code)")?;
    }
    let parent_id = match under {
        Some(parent) => Some(crate::db::queries::resolve_intent_from_snapshot(
            &store.query_snapshot()?,
            parent,
        )?),
        None => None,
    };

    let rel = relative_to_root(file, &cwd)?;
    let spec = load_spec_file(&cwd.join(&rel))?;
    let now = chrono::Utc::now().to_rfc3339();
    let required_env = crate::saga::spec::required_env(&spec);
    let (step_intents, spawned) =
        resolve_step_intents_sqlite(&mut store, &spec, spawn_missing, parent_id.as_deref(), &now)?;

    let command = format!("loom saga run {rel}");
    let env_line = if required_env.is_empty() {
        String::new()
    } else {
        format!("\nrequires env: {}", required_env.join(", "))
    };
    let description =
        format!(
        "{}{}Consumer saga proof — {} step(s), run by the built-in engine.\nspec:{rel}{env_line}",
        spec.description.trim(),
        if spec.description.trim().is_empty() { "" } else { "\n" },
        spec.steps.len(),
    );
    let existing = store
        .list_validations()?
        .into_iter()
        .find(|validation| validation.name == spec.saga);
    let (validation_id, created) = match existing {
        Some(v) => {
            if v.validation_type != "saga" {
                anyhow::bail!(
                    "A validation named '{}' already exists with type '{}'. Saga names share the validation namespace.",
                    spec.saga,
                    v.validation_type
                );
            }
            store.update_validation_definition(&v.id, Some(&command), Some(&description))?;
            (v.id, false)
        }
        None => {
            let id = Uuid::new_v4().to_string();
            store.insert_validation(&Validation {
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

    let already: std::collections::HashSet<String> = store
        .query_snapshot()?
        .validates
        .into_iter()
        .filter(|edge| edge.validation_id == validation_id)
        .map(|edge| edge.intent_id)
        .collect();
    let mut linked = 0usize;
    let mut seen = std::collections::HashSet::new();
    for (iid, _) in &step_intents {
        if seen.insert(iid.clone()) && !already.contains(iid) {
            store.insert_validates(&validation_id, iid, "", &now)?;
            linked += 1;
        }
    }

    let mut path_edges = 0usize;
    for pair in step_intents.windows(2) {
        let (a, b) = (&pair[0].0, &pair[1].0);
        if a != b {
            store.get_or_create_relates_to(a, b, &now)?;
            path_edges += 1;
        }
    }

    let mut interface_calls = 0usize;
    let mut interface_surfaces = Vec::new();
    for (idx, step) in spec.steps.iter().enumerate() {
        let method = step.request.method.trim().to_uppercase();
        let target = normalize_step_target(&step.request.url);
        let description = format!("HTTP endpoint called by saga '{}'", spec.saga);
        let surface = store.get_or_create_interface_surface(
            "http_endpoint",
            &method,
            &target,
            &description,
            &now,
        )?;
        store.insert_call(
            &validation_id,
            &surface.id,
            idx + 1,
            &step.name,
            &step_intents[idx].0,
            &now,
        )?;
        interface_surfaces.push(surface);
        interface_calls += 1;
    }

    let mut registered_spec = false;
    if !store
        .list_codefiles()?
        .iter()
        .any(|codefile| codefile.path == rel || codefile.id == rel)
    {
        let abs = cwd.join(&rel);
        let last_modified = crate::repo::mtime_rfc3339(&abs).ok_or_else(|| {
            anyhow::anyhow!(
                "Cannot read mtime for {} — ensure the spec file exists under the graph root, or re-register: `loom saga add <file>`.",
                abs.display()
            )
        })?;
        let content_hash = std::fs::read(&abs)
            .map(|bytes| crate::repo::content_hash(&bytes))
            .with_context(|| format!("Cannot read bytes for {}", abs.display()))?;
        store.insert_codefile(&CodeFile {
            id: Uuid::new_v4().to_string(),
            path: rel.clone(),
            language: "yaml".to_string(),
            last_modified,
            imports: Vec::new(),
            symbols: Vec::new(),
            symbol_facts: Vec::new(),
            content_hash,
        })?;
        registered_spec = true;
    }

    if printer.json {
        let mut next_steps = Vec::new();
        if !spawned.is_empty() {
            next_steps.push(format!(
                "{} planned intent(s) spawned from the journey — `loom next --mode build` realizes them (the saga is their acceptance test).",
                spawned.len()
            ));
        }
        next_steps.push(format!(
            "Run it: `{}`.",
            run_invocation(&spec.saga, &required_env)
        ));
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
            "spawned_intents": spawned.iter().map(|(id, name)| serde_json::json!({
                "id": id, "name": name, "lifecycle": "planned", "visibility": "user_visible",
            })).collect::<Vec<_>>(),
            "validates_linked": linked,
            "path_edges_ensured": path_edges,
            "interface_calls": interface_calls,
            "interfaces": interface_surfaces.iter().map(|surface| serde_json::json!({
                "id": surface.id,
                "name": surface.name,
                "kind": surface.surface_kind,
                "method": surface.method,
                "target": surface.target,
            })).collect::<Vec<_>>(),
            "spec_registered_as_codefile": registered_spec,
            "requires_env": required_env,
            "next_steps": next_steps,
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
        if !spawned.is_empty() {
            println!("  Spawned from the journey (planned, user_visible — the build queue realizes them):");
            for (id, name) in &spawned {
                println!("    + '{}' ({})", name, id);
            }
            if under.is_none() {
                println!(
                    "    ⚠ spawned as roots — link them: `loom edge hierarchy <parent> <child>`"
                );
            }
        }
        println!(
            "  VALIDATES edges added: {linked} · path RELATES_TO ensured: {path_edges} · interface CALLS recorded: {interface_calls}"
        );
        if registered_spec {
            println!("  Spec registered as a CodeFile — ground it under a consumer-journeys intent when you have one.");
        }
        if !required_env.is_empty() {
            println!(
                "  Requires env at invocation (the live target — never stored in the graph): {}",
                required_env.join(", ")
            );
        }
        println!(
            "  → Run it: `{}`",
            run_invocation(&spec.saga, &required_env)
        );
    }
    Ok(())
}

fn execute_sqlite(arg: &str, printer: &Printer) -> Result<()> {
    let agent = crate::gate::acting_in_lane(&crate::gate::lane::RUN_SAGA, None)?;
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;

    let rel = resolve_saga_spec_arg(&store, arg, &cwd)?;
    let spec = load_spec_file(&cwd.join(&rel))?;
    let validation = store
        .list_validations()?
        .into_iter()
        .find(|validation| validation.name == spec.saga && validation.validation_type == "saga")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Saga '{}' is not registered in the graph yet. Run `loom saga add {rel}` first.",
                spec.saga
            )
        })?;
    let mut resolver = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let (step_intents, _) = resolve_step_intents_sqlite(&mut resolver, &spec, false, None, "")?;

    let missing = crate::saga::spec::missing_env(&spec);
    if !missing.is_empty() {
        anyhow::bail!(
            "Saga '{name}' needs environment value(s) this invocation didn't set: {missing}.\n\
             Pass them on the command line:\n\n  {invocation}\n\nNothing was run or recorded. If the target cannot run yet, record it honestly:\n\n  loom validation mark {name} --result blocked --reason \"waiting on <what>\"",
            name = spec.saga,
            missing = missing.join(", "),
            invocation = run_invocation(&spec.saga, &missing),
        );
    }

    drop(store);
    drop(resolver);
    let report = run_saga(&spec)?;

    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let now = chrono::Utc::now().to_rfc3339();
    let result = if report.passed { "passed" } else { "failed" };
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
    store.mark_validation_result(
        &validation.id,
        result,
        if report.passed { "passing" } else { "failing" },
        &summary,
        &agent,
        &now,
    )?;

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
        store.get_or_create_relates_to(a_id, b_id, &now)?;
        if o_a.passed && o_b.passed {
            let evidence = format!(
                "runtime: saga '{}' run {now}: step {} ('{}' {} {}) → step {} ('{}' {} {}) both passed against the live surface",
                report.saga, o_a.step, o_a.name, o_a.method, o_a.url,
                o_b.step, o_b.name, o_b.method, o_b.url,
            );
            store
                .update_relates_to_ground(a_id, b_id, &criterion, &evidence, 0.95, &agent, &now)?;
            stamped_passing += 1;
        } else if o_a.passed && !o_b.passed {
            let evidence = format!(
                "runtime: saga '{}' run {now}: step {} ('{}' {} {}) failed — {}",
                report.saga, o_b.step, o_b.name, o_b.method, o_b.url, o_b.detail,
            );
            store.update_relates_to_issue(a_id, b_id, &criterion, &evidence, 0.95, &agent, &now)?;
            stamped_failing += 1;
        }
    }

    print_report_sqlite(&report, stamped_passing, stamped_failing, &store, printer)?;
    if !report.passed {
        std::process::exit(1);
    }
    Ok(())
}

fn diagnose_sqlite(arg: &str, printer: &Printer) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::RUN_SAGA, None)?;
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;

    let rel = resolve_saga_spec_arg(&store, arg, &cwd)?;
    let spec = load_spec_file(&cwd.join(&rel))?;
    let missing = crate::saga::spec::missing_env(&spec);
    if !missing.is_empty() {
        let diagnosis = diagnose_missing_env(&spec, &missing, run_invocation(&spec.saga, &missing));
        print_diagnosis_sqlite(&diagnosis, &store, printer)?;
        std::process::exit(1);
    }

    drop(store);
    let report = run_saga(&spec)?;
    let diagnosis = diagnose_report(&spec, &report);
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    print_diagnosis_sqlite(&diagnosis, &store, printer)?;
    if !diagnosis.passed {
        std::process::exit(1);
    }
    Ok(())
}

fn print_diagnosis_sqlite(
    diagnosis: &SagaDiagnosis,
    store: &crate::db::sqlite::SqliteGraphStore,
    printer: &Printer,
) -> Result<()> {
    let next_step = if diagnosis.passed {
        "`loom saga run <saga>` can stamp the passing proof if this was only a diagnosis run"
            .to_string()
    } else {
        "fix the first failed root cause, then rerun `loom saga diagnose` or `loom saga run`"
            .to_string()
    };
    if printer.json {
        printer.print_json(&crate::output::with_read_anchor(
            serde_json::json!({
                "status": if diagnosis.passed { "passed" } else { "failed" },
                "diagnosis": diagnosis,
            }),
            store,
            &next_step,
        )?);
        return Ok(());
    }

    println!(
        "── Saga: {} ─────────────────────────────────────────",
        diagnosis.saga
    );
    for step in &diagnosis.steps {
        match step.outcome.as_str() {
            "passed" => println!(
                "  Step {} ✓ ({} {}) — {}",
                step.step, step.method, step.url, step.detail
            ),
            "skipped" => {
                println!("  Step {} ⊘ (skipped)", step.step);
                if let Some(root) = &step.root_cause {
                    println!("    Root cause: {}", root.title);
                    for field in &root.fields {
                        println!("      {:<12} {}", format!("{}:", field.name), field.value);
                    }
                    println!("      {:<12} {}", "Fix:", root.fix);
                }
            }
            _ => {
                let status = step
                    .http_status
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "no response".to_string());
                println!("  Step {} ✗ ({status})", step.step);
                if let Some(root) = &step.root_cause {
                    println!();
                    println!("  Root cause: {}", root.title);
                    for field in &root.fields {
                        println!("    {:<14} {}", format!("{}:", field.name), field.value);
                    }
                    println!("    {:<14} {}", "Fix:", root.fix);
                } else {
                    println!("    {}", step.detail);
                }
            }
        }
    }
    println!();
    println!("── Summary ─────────────────────────────────────────");
    println!("  {} saga diagnosed", diagnosis.summary.diagnosed_sagas);
    println!("  Failed:        {}", diagnosis.summary.failed);
    println!("  Passed:        {}", diagnosis.summary.passed);
    println!("  Skipped steps: {}", diagnosis.summary.skipped_steps);
    for item in &diagnosis.summary.by_kind {
        println!("  {:<14} {}", format!("{}:", item.kind), item.count);
    }
    if !diagnosis.summary.suggested_order.is_empty() {
        println!(
            "  Suggested order: {}",
            diagnosis.summary.suggested_order.join(" → ")
        );
    }
    println!("  → Next: {next_step}");
    let snapshot = store.query_snapshot()?;
    let graph_state = store.graph_state(&snapshot)?;
    println!("  {}", crate::output::fmt_pulse(&graph_state));
    Ok(())
}

fn print_report_sqlite(
    report: &SagaRunReport,
    stamped_passing: usize,
    stamped_failing: usize,
    store: &crate::db::sqlite::SqliteGraphStore,
    printer: &Printer,
) -> Result<()> {
    let next_step = if report.passed {
        "`loom next --mode validate` continues the proof queue".to_string()
    } else {
        "the failing edge carries the evidence: `loom next --mode fix` will serve it.".to_string()
    };
    if printer.json {
        printer.print_json(&crate::output::with_read_anchor(
            serde_json::json!({
                "saga": report.saga,
                "result": if report.passed { "passed" } else { "failed" },
                "executed": report.executed,
                "total_steps": report.total_steps,
                "steps": report.outcomes,
                "relates_to_stamped_passing": stamped_passing,
                "relates_to_stamped_failing": stamped_failing,
            }),
            store,
            &next_step,
        )?);
        return Ok(());
    }
    for o in &report.outcomes {
        let mark = if o.passed { "✓" } else { "✗" };
        println!(
            "  {} {}. {} ({} {}) — {}",
            mark, o.step, o.name, o.method, o.url, o.detail
        );
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
        match report.failure() {
            Some(f) => println!(
                "✗ Saga '{}' FAILED at step {}/{} ('{}').",
                report.saga, f.step, report.total_steps, f.name
            ),
            None => println!(
                "✗ Saga '{}' FAILED ({}/{} steps).",
                report.saga, report.executed, report.total_steps
            ),
        }
        println!(
            "  {} path edge(s) stamped passing (they ran), {} stamped failing (the broken boundary).",
            stamped_passing, stamped_failing
        );
    }
    let snapshot = store.query_snapshot()?;
    let graph_state = store.graph_state(&snapshot)?;
    println!("  → Next: {next_step}");
    println!("  {}", crate::output::fmt_pulse(&graph_state));
    Ok(())
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let db = GraphReadHandle::open(&cwd)?;
    run_list_with_db(&db, printer)
}

fn run_list_with_db(db: &dyn GraphReadRepository, printer: &Printer) -> Result<()> {
    let sagas: Vec<Validation> = db
        .list_validations()?
        .into_iter()
        .filter(|v| v.validation_type == "saga")
        .collect();
    if printer.json {
        let rows: Vec<serde_json::Value> = sagas
            .iter()
            .map(|v| {
                serde_json::json!({
                    "id": v.id,
                    "name": v.name,
                    "spec": spec_path_of(v),
                    "requires_env": required_env_of(v),
                    "run_with": run_invocation(&v.name, &required_env_of(v)),
                    "last_result": v.last_result,
                    "last_run": v.last_run,
                })
            })
            .collect();
        printer.print_json(&serde_json::json!({
            "sagas": rows,
            "total": rows.len(),
            "truncated": false,
        }));
    } else if sagas.is_empty() {
        println!("(no sagas registered — `loom saga add <spec.yaml>`)");
    } else {
        println!(
            "  {:<10}  {:<28}  {:<32}  last run",
            "RESULT", "NAME", "SPEC"
        );
        println!("  {}", "-".repeat(96));
        for v in &sagas {
            println!(
                "  [{:<8}]  {:<28}  {:<32}  {}",
                v.last_result,
                v.name,
                spec_path_of(v).unwrap_or_else(|| "?".to_string()),
                if v.last_run.is_empty() {
                    "(never)"
                } else {
                    &v.last_run
                },
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

type StepIntentBindings = (Vec<(String, String)>, Vec<(String, String)>);

fn resolve_validation_sqlite(
    store: &crate::db::sqlite::SqliteGraphStore,
    key: &str,
) -> Result<Validation> {
    let validations = store.list_validations()?;
    if let Some(validation) = validations.iter().find(|validation| validation.id == key) {
        return Ok(validation.clone());
    }
    let kl = key.to_lowercase();
    let exact: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase() == kl)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    let subs: Vec<_> = validations
        .iter()
        .filter(|validation| validation.name.to_lowercase().contains(&kl))
        .collect();
    match subs.len() {
        1 => Ok(subs[0].clone()),
        0 => anyhow::bail!(
            "No validation matches '{}' (by id, name, or fragment). Run `loom saga list` or `loom validation list`.",
            key
        ),
        _ => anyhow::bail!(
            "'{}' is ambiguous — matches {} validations. Use the id.",
            key,
            subs.len()
        ),
    }
}

fn resolve_saga_spec_arg(
    store: &crate::db::sqlite::SqliteGraphStore,
    arg: &str,
    cwd: &std::path::Path,
) -> Result<String> {
    if cwd.join(arg).is_file() || std::path::Path::new(arg).is_file() {
        return relative_to_root(arg, cwd);
    }
    let validation = resolve_validation_sqlite(store, arg)?;
    if validation.validation_type != "saga" {
        anyhow::bail!(
            "'{}' is a {} validation, not a saga. Run it via `loom validate <intent>`.",
            validation.name,
            validation.validation_type
        );
    }
    let recorded = spec_path_of(&validation).ok_or_else(|| {
        anyhow::anyhow!(
            "Saga validation '{}' has no `spec:` line in its description — re-register it: `loom saga add <file>`.",
            validation.name
        )
    })?;
    relative_to_root(&recorded, cwd).with_context(|| {
        format!(
            "Saga validation '{}' records spec path '{}'. Re-register it with `loom saga add <file>` using a file under the graph root.",
            validation.name, recorded
        )
    })
}

fn resolve_step_intents_sqlite(
    store: &mut crate::db::sqlite::SqliteGraphStore,
    spec: &SagaSpec,
    spawn_missing: bool,
    parent_id: Option<&str>,
    now: &str,
) -> Result<StepIntentBindings> {
    let mut out = Vec::with_capacity(spec.steps.len());
    let mut spawned: Vec<(String, String)> = Vec::new();
    for (i, step) in spec.steps.iter().enumerate() {
        let key = step.intent.trim();
        if key.is_empty() {
            anyhow::bail!(
                "step {} ('{}'): `intent:` must not be empty — name the behavior this step exercises.",
                i + 1,
                step.name
            );
        }
        let snapshot = store.query_snapshot()?;
        let resolved = crate::db::queries::try_resolve_intent_from_snapshot(&snapshot, key)
            .with_context(|| {
                format!("step {} ('{}'): intent binding '{}'", i + 1, step.name, key)
            })?;
        let iid = match resolved {
            Some(iid) => iid,
            None if spawned.iter().any(|(_, n)| n.eq_ignore_ascii_case(key)) => spawned
                .iter()
                .find(|(_, n)| n.eq_ignore_ascii_case(key))
                .map(|(id, _)| id.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "internal: step {} ('{}') matched a spawned intent that then vanished",
                        i + 1,
                        step.name
                    )
                })?,
            None if spawn_missing => {
                let id = Uuid::new_v4().to_string();
                store.insert_intent(&Intent {
                    id: id.clone(),
                    name: key.to_string(),
                    description: format!(
                        "Journey '{}' step {}: {} — {} {}. Spawned from the narrated journey; sharpen into a falsifiable criterion when realizing.",
                        spec.saga, i + 1, step.name, step.request.method, step.request.url
                    ),
                    criterion: String::new(),
                    abstraction_level: "feature".to_string(),
                    domain: String::new(),
                    layer: String::new(),
                    source_refs: Vec::new(),
                    status: "proposed".to_string(),
                    aspect: String::new(),
                    tags: Vec::new(),
                    visibility: "user_visible".to_string(),
                    boundary: String::new(),
                    lifecycle: "planned".to_string(),
                    created_at: now.to_string(),
                    updated_at: now.to_string(),
                })?;
                if let Some(parent) = parent_id {
                    store.insert_hierarchy(parent, &id, "", now)?;
                }
                spawned.push((id.clone(), key.to_string()));
                id
            }
            None => {
                anyhow::bail!(
                    "step {} ('{}'): cannot resolve intent '{}' — every step binds to an intent. Create it first (`loom intent add`), or let the journey spawn it: re-run with `--spawn-missing [--under <parent>]`.",
                    i + 1,
                    step.name,
                    step.intent
                )
            }
        };
        let name = store
            .get_intent(&iid)?
            .map(|intent| intent.name)
            .or_else(|| {
                spawned
                    .iter()
                    .find(|(sid, _)| *sid == iid)
                    .map(|(_, name)| name.clone())
            })
            .unwrap_or_else(|| iid.clone());
        out.push((iid, name));
    }
    Ok((out, spawned))
}

/// The exact command line to run a saga, with required env vars as
/// placeholders to fill: `BASE_URL=<value> loom saga run checkout-flow`.
fn run_invocation(saga_name: &str, env_vars: &[String]) -> String {
    let prefix: String = env_vars.iter().map(|v| format!("{v}=<value> ")).collect();
    format!("{prefix}loom saga run {saga_name}")
}

pub(crate) fn normalize_step_target(url: &str) -> String {
    let trimmed = url.trim();
    if let Ok(parsed) = reqwest::Url::parse(trimmed) {
        let mut target = parsed.path().to_string();
        if let Some(query) = parsed.query() {
            target.push('?');
            target.push_str(query);
        }
        if target.is_empty() {
            "/".to_string()
        } else {
            target
        }
    } else {
        trimmed.to_string()
    }
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
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
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
    let abs =
        std::fs::canonicalize(&abs).with_context(|| format!("Saga spec '{}' not found", file))?;
    let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let rel = abs.strip_prefix(&root_canon).with_context(|| {
        format!(
            "Saga spec '{}' is outside the graph root '{}'. Move the spec under the graph root, then run `loom saga add <file>` again.",
            abs.display(),
            root_canon.display()
        )
    })?;
    Ok(rel.display().to_string())
}
