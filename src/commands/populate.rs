//! `loom populate` — computed graph-population work.
//!
//! Population is graph construction, not product-code lifecycle. It backfills
//! derived structure from existing evidence after a schema/modeling upgrade.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::PopulateCmd;
use crate::db::{ensure_initialized, GraphReadRepository};
use crate::output::{pulse_json, Printer};
use crate::types::{interface_surface_name, CallsEdge, Intent, Validation};

const GAP_EXAMPLE_LIMIT: usize = 10;
pub(crate) const POPULATE_INTERFACES_FROM_SAGAS_CMD: &str = "loom populate interfaces --from-sagas";
pub(crate) const NO_INTERFACE_GAPS_MESSAGE: &str = "✓ No interface-plane gaps detected.";
const BOUNDARY_INTENT_WITHOUT_CALLS: &str = "boundary_intent_without_calls";

pub fn run(cmd: PopulateCmd, printer: &Printer) -> Result<()> {
    match cmd {
        PopulateCmd::Plan => plan(printer),
        PopulateCmd::Interfaces {
            from_sagas,
            dry_run,
        } => populate_interfaces(from_sagas, dry_run, printer),
        PopulateCmd::Kinds { dry_run } => populate_kinds(dry_run, printer),
    }
}

/// Backfill the mechanical relationship-kind tier onto grounded RELATES_TO
/// edges from existing evidence (imports/shares_file/shares_vocab/same_domain).
/// Judgment kinds already on an edge are preserved; only the mechanical tier is
/// recomputed — deriving the same signals the discovery queue uses, carried into
/// durable truth so the epistemic layer can weight grounding strength by kind.
fn populate_kinds(dry_run: bool, printer: &Printer) -> Result<()> {
    crate::gate::acting_in_lane(&crate::gate::lane::POPULATE_GRAPH, None)?;
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    store.ensure_owned("populate relationship kinds")?;

    let snapshot = store.query_snapshot()?;
    let discovery = crate::db::queries::DiscoverySnapshot::from_query(&snapshot)?;
    let by_id: HashMap<&str, &Intent> = snapshot
        .intents
        .iter()
        .map(|i| (i.id.as_str(), i))
        .collect();

    let mut changes: Vec<String> = Vec::new();
    for e in &snapshot.relates {
        let (Some(a), Some(b)) = (by_id.get(e.from_id.as_str()), by_id.get(e.to_id.as_str()))
        else {
            continue;
        };
        let mechanical: Vec<String> =
            crate::db::queries::mechanical_kinds_for_pair(&discovery, a, b)
                .into_iter()
                .map(|k| k.as_str().to_string())
                .collect();
        // Preserve judgment kinds (analyzer-asserted); replace the mechanical tier.
        let mut new_kinds: Vec<String> = e
            .kinds
            .iter()
            .filter(|k| {
                k.parse::<crate::types::RelationKind>()
                    .map(|rk| !rk.is_mechanical())
                    .unwrap_or(true)
            })
            .cloned()
            .collect();
        for m in mechanical {
            if !new_kinds.contains(&m) {
                new_kinds.push(m);
            }
        }
        new_kinds.sort();
        let mut current = e.kinds.clone();
        current.sort();
        if new_kinds != current {
            changes.push(format!(
                "{} × {}: [{}]",
                e.from_name,
                e.to_name,
                new_kinds.join(", ")
            ));
            if !dry_run {
                store.update_relates_to_kinds(&e.from_id, &e.to_id, &new_kinds)?;
            }
        }
    }

    let updated = changes.len();
    let gs = store.graph_state(&store.query_snapshot()?)?;
    if printer.json {
        const CAP: usize = 30;
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "dry_run": dry_run,
            "edges_updated": updated,
            "changes": changes.iter().take(CAP).collect::<Vec<_>>(),
            "changes_total": updated,
            "graph_state": pulse_json(&gs),
        }));
    } else {
        println!("── loom populate kinds ───────────────────────────────────────────────");
        if dry_run {
            println!("  [dry run] {updated} RELATES_TO edge(s) would get mechanical kinds.");
        } else {
            println!("  {updated} RELATES_TO edge(s) got mechanical kinds.");
        }
        for c in changes.iter().take(30) {
            println!("    {c}");
        }
        println!("  {}", crate::output::fmt_pulse(&gs));
    }
    Ok(())
}

pub(crate) fn plan_with_repo(db: &dyn GraphReadRepository, root: &Path) -> Result<PopulatePlan> {
    Ok(PopulatePlan {
        interface_from_sagas: interface_from_sagas_plan(db, root)?,
        interface_gaps: interface_gaps_plan(db)?,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct PopulatePlan {
    pub interface_from_sagas: InterfacePopulatePlan,
    pub interface_gaps: InterfaceGapPlan,
}

impl PopulatePlan {
    pub(crate) fn pending_count(&self) -> usize {
        self.interface_from_sagas.sagas_needing_repopulate + self.interface_gaps.total()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InterfacePopulatePlan {
    pub sagas_total: usize,
    pub sagas_ready: usize,
    pub sagas_skipped: Vec<SkippedSaga>,
    pub expected_calls: usize,
    pub existing_calls: usize,
    pub missing_surfaces: usize,
    pub stale_call_sets: usize,
    pub sagas_needing_repopulate: usize,
}

impl InterfacePopulatePlan {
    pub(crate) fn is_pending(&self) -> bool {
        self.sagas_needing_repopulate > 0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SkippedSaga {
    pub validation_id: String,
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub(crate) struct InterfaceGapPlan {
    pub surface_without_calls: usize,
    pub boundary_intent_without_calls: usize,
    pub call_without_validates: usize,
    pub examples: Vec<InterfaceGap>,
}

impl InterfaceGapPlan {
    pub(crate) fn total(&self) -> usize {
        self.surface_without_calls
            + self.boundary_intent_without_calls
            + self.call_without_validates
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.total() > 0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InterfaceGap {
    pub kind: &'static str,
    pub summary: String,
    pub surface_id: String,
    pub surface: String,
    pub intent_id: String,
    pub intent: String,
    pub validation_id: String,
    pub validation: String,
    pub suggested_action: String,
}

#[derive(Debug, Clone)]
struct SagaExpectation {
    validation_id: String,
    validation_name: String,
    calls: Vec<ExpectedCall>,
}

#[derive(Debug, Clone)]
struct ExpectedCall {
    method: String,
    target: String,
    surface_name: String,
    step_index: usize,
    step_name: String,
    intent_id: String,
}

fn plan(printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    let plan = plan_with_repo(&store, &cwd)?;
    render_plan(&plan, true, printer)
}

fn populate_interfaces(from_sagas: bool, dry_run: bool, printer: &Printer) -> Result<()> {
    if !from_sagas {
        anyhow::bail!(
            "`loom populate interfaces` currently requires --from-sagas. \
             This v1 backfills InterfaceSurface/CALLS from existing saga specs."
        );
    }

    crate::gate::acting_in_lane(&crate::gate::lane::POPULATE_GRAPH, None)?;
    let cwd = crate::db::resolve_root()?;
    ensure_initialized(&cwd)?;
    let store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(&cwd))?;
    store.ensure_owned("populate derived graph structure")?;

    if dry_run {
        let plan = plan_with_repo(&store, &cwd)?;
        return render_plan(&plan, true, printer);
    }

    let snapshot = store.query_snapshot()?;
    let sagas: Vec<Validation> = snapshot
        .validations
        .iter()
        .filter(|validation| validation.validation_type == "saga")
        .cloned()
        .collect();
    let mut skipped = Vec::new();
    let mut expectations = Vec::new();
    for saga in &sagas {
        match saga_expectation(&snapshot, &cwd, saga) {
            Ok(expectation) => expectations.push(expectation),
            Err(err) => skipped.push(SkippedSaga {
                validation_id: saga.id.clone(),
                name: saga.name.clone(),
                reason: err.to_string(),
            }),
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut known_surfaces: HashSet<(String, String, String)> = store
        .list_interface_surfaces()?
        .into_iter()
        .map(|surface| (surface.surface_kind, surface.method, surface.target))
        .collect();
    let mut deleted_calls = 0usize;
    let mut calls_written = 0usize;
    let mut surfaces_created = 0usize;
    let mut sagas_processed = 0usize;

    for expectation in &expectations {
        deleted_calls += store.delete_calls_for_validation(&expectation.validation_id)?;
        for call in &expectation.calls {
            let key = (
                "http_endpoint".to_string(),
                call.method.clone(),
                call.target.clone(),
            );
            if !known_surfaces.contains(&key) {
                surfaces_created += 1;
                known_surfaces.insert(key);
            }
            let description = format!(
                "HTTP endpoint called by saga '{}'",
                expectation.validation_name
            );
            let surface = store.get_or_create_interface_surface(
                "http_endpoint",
                &call.method,
                &call.target,
                &description,
                &now,
            )?;
            store.insert_call(
                &expectation.validation_id,
                &surface.id,
                call.step_index,
                &call.step_name,
                &call.intent_id,
                &now,
            )?;
            calls_written += 1;
        }
        sagas_processed += 1;
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "kind": "interface_from_sagas",
            "dry_run": false,
            "sagas_total": sagas.len(),
            "sagas_processed": sagas_processed,
            "sagas_skipped": skipped_json(&skipped),
            "deleted_calls": deleted_calls,
            "interface_surfaces_created": surfaces_created,
            "calls_written": calls_written,
            "next_step": "`loom interface list`; then `loom export` after graph changes",
        }));
        return Ok(());
    }

    println!("✓ Populated interfaces from sagas");
    println!("  Sagas processed: {sagas_processed} / {}", sagas.len());
    println!(
        "  CALLS replaced: deleted {deleted_calls}, wrote {calls_written}; interface surfaces created: {surfaces_created}"
    );
    if !skipped.is_empty() {
        println!("  Skipped saga(s):");
        for skip in &skipped {
            println!(
                "    - {} ({}) — {}",
                skip.name, skip.validation_id, skip.reason
            );
        }
    }
    println!("  → Next: `loom interface list`; then `loom export` after graph changes");
    Ok(())
}

fn interface_from_sagas_plan(
    db: &dyn GraphReadRepository,
    root: &Path,
) -> Result<InterfacePopulatePlan> {
    let snapshot = db.query_snapshot()?;
    let sagas: Vec<&Validation> = snapshot
        .validations
        .iter()
        .filter(|validation| validation.validation_type == "saga")
        .collect();
    let surfaces = db.list_interface_surfaces()?;
    let calls = db.list_all_calls()?;
    let surface_keys: HashSet<(String, String, String)> = surfaces
        .iter()
        .map(|surface| {
            (
                surface.surface_kind.clone(),
                surface.method.clone(),
                surface.target.clone(),
            )
        })
        .collect();
    let calls_by_validation = calls_by_validation(&calls);

    let mut skipped = Vec::new();
    let mut expected_calls = 0usize;
    let mut missing_surfaces = BTreeSet::new();
    let mut stale_call_sets = 0usize;
    let mut sagas_needing_repopulate = 0usize;
    let mut sagas_ready = 0usize;

    for saga in &sagas {
        match saga_expectation(&snapshot, root, saga) {
            Ok(expectation) => {
                sagas_ready += 1;
                expected_calls += expectation.calls.len();
                let mut saga_needs_repopulate = false;
                for call in &expectation.calls {
                    let key = (
                        "http_endpoint".to_string(),
                        call.method.clone(),
                        call.target.clone(),
                    );
                    if !surface_keys.contains(&key) {
                        missing_surfaces.insert(key);
                        saga_needs_repopulate = true;
                    }
                }
                let existing = calls_by_validation
                    .get(expectation.validation_id.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if !calls_match_expected(existing, &expectation.calls) {
                    stale_call_sets += 1;
                    saga_needs_repopulate = true;
                }
                if saga_needs_repopulate {
                    sagas_needing_repopulate += 1;
                }
            }
            Err(err) => skipped.push(SkippedSaga {
                validation_id: saga.id.clone(),
                name: saga.name.clone(),
                reason: err.to_string(),
            }),
        }
    }

    let existing_calls = sagas
        .iter()
        .map(|saga| {
            calls_by_validation
                .get(saga.id.as_str())
                .map(Vec::len)
                .unwrap_or(0)
        })
        .sum();

    Ok(InterfacePopulatePlan {
        sagas_total: sagas.len(),
        sagas_ready,
        sagas_skipped: skipped,
        expected_calls,
        existing_calls,
        missing_surfaces: missing_surfaces.len(),
        stale_call_sets,
        sagas_needing_repopulate,
    })
}

pub(crate) fn interface_gaps_with_repo(db: &dyn GraphReadRepository) -> Result<InterfaceGapPlan> {
    interface_gaps_plan(db)
}

fn interface_gaps_plan(db: &dyn GraphReadRepository) -> Result<InterfaceGapPlan> {
    let snapshot = db.query_snapshot()?;
    let surfaces = db.list_interface_surfaces()?;
    let calls = db.list_all_calls()?;

    let calls_by_interface = calls_by_interface(&calls);
    let calls_by_intent = calls_by_intent(&calls);
    let validates_pairs: HashSet<(&str, &str)> = snapshot
        .validates
        .iter()
        .map(|edge| (edge.validation_id.as_str(), edge.intent_id.as_str()))
        .collect();

    let mut examples = Vec::new();
    let mut surface_without_calls = 0usize;
    for surface in &surfaces {
        if !calls_by_interface.contains_key(surface.id.as_str()) {
            surface_without_calls += 1;
            push_gap_example(
                &mut examples,
                InterfaceGap {
                    kind: "surface_without_calls",
                    summary: format!("interface surface '{}' has no CALLS edge", surface.name),
                    surface_id: surface.id.clone(),
                    surface: surface.name.clone(),
                    intent_id: String::new(),
                    intent: String::new(),
                    validation_id: String::new(),
                    validation: String::new(),
                    suggested_action:
                        "bind this surface through a saga step, or remove/repair the stale interface surface"
                            .to_string(),
                },
            );
        }
    }

    let mut boundary_intent_without_calls = 0usize;
    for intent in snapshot
        .intents
        .iter()
        .filter(|intent| boundary_intent(intent))
    {
        if !calls_by_intent.contains_key(intent.id.as_str()) {
            boundary_intent_without_calls += 1;
            push_gap_example(
                &mut examples,
                InterfaceGap {
                    kind: BOUNDARY_INTENT_WITHOUT_CALLS,
                    summary: format!(
                        "{} boundary intent '{}' has no CALLS binding",
                        intent.boundary, intent.name
                    ),
                    surface_id: String::new(),
                    surface: String::new(),
                    intent_id: intent.id.clone(),
                    intent: intent.name.clone(),
                    validation_id: String::new(),
                    validation: String::new(),
                    suggested_action:
                        "add/repair a saga step for this boundary behavior, or clear the intent boundary if it is internal"
                            .to_string(),
                },
            );
        }
    }

    let mut call_without_validates = 0usize;
    for call in &calls {
        if !validates_pairs.contains(&(call.validation_id.as_str(), call.intent_id.as_str())) {
            call_without_validates += 1;
            push_gap_example(
                &mut examples,
                InterfaceGap {
                    kind: "call_without_validates",
                    summary: format!(
                        "CALLS step '{}' binds validation '{}' to intent '{}' without a VALIDATES edge",
                        call.step_name, call.validation_name, call.intent_name
                    ),
                    surface_id: call.interface_id.clone(),
                    surface: call.interface_name.clone(),
                    intent_id: call.intent_id.clone(),
                    intent: call.intent_name.clone(),
                    validation_id: call.validation_id.clone(),
                    validation: call.validation_name.clone(),
                    suggested_action:
                        "repair the saga registration so the validation VALIDATES the same intent named by CALLS"
                            .to_string(),
                },
            );
        }
    }

    Ok(InterfaceGapPlan {
        surface_without_calls,
        boundary_intent_without_calls,
        call_without_validates,
        examples,
    })
}

fn boundary_intent(intent: &Intent) -> bool {
    let lifecycle = if intent.lifecycle.is_empty() {
        "implemented"
    } else {
        intent.lifecycle.as_str()
    };
    lifecycle == "implemented"
        && intent.status != "deprecated"
        && matches!(intent.boundary.as_str(), "inbound" | "outbound")
}

fn calls_by_interface(calls: &[CallsEdge]) -> HashMap<&str, Vec<&CallsEdge>> {
    let mut out: HashMap<&str, Vec<&CallsEdge>> = HashMap::new();
    for call in calls {
        out.entry(call.interface_id.as_str())
            .or_default()
            .push(call);
    }
    out
}

fn calls_by_intent(calls: &[CallsEdge]) -> HashMap<&str, Vec<&CallsEdge>> {
    let mut out: HashMap<&str, Vec<&CallsEdge>> = HashMap::new();
    for call in calls {
        out.entry(call.intent_id.as_str()).or_default().push(call);
    }
    out
}

fn push_gap_example(examples: &mut Vec<InterfaceGap>, gap: InterfaceGap) {
    if examples.len() < GAP_EXAMPLE_LIMIT {
        examples.push(gap);
    }
}

fn saga_expectation(
    snapshot: &crate::db::queries::QuerySnapshot,
    root: &Path,
    saga: &Validation,
) -> Result<SagaExpectation> {
    let spec_path = crate::commands::saga::spec_path_of(saga).ok_or_else(|| {
        anyhow::anyhow!(
            "missing `spec:<path>` in saga validation description; re-register with `loom saga add <file>`"
        )
    })?;
    let path = root.join(&spec_path);
    let spec = crate::saga::spec::load_spec_file(&path)
        .with_context(|| format!("cannot load recorded saga spec '{}'", spec_path))?;
    let mut calls = Vec::new();
    for (idx, step) in spec.steps.iter().enumerate() {
        let intent_id = crate::db::queries::resolve_intent_from_snapshot(snapshot, &step.intent)
            .with_context(|| {
                format!(
                    "step {} ('{}') intent '{}' does not resolve",
                    idx + 1,
                    step.name,
                    step.intent
                )
            })?;
        let method = step.request.method.trim().to_uppercase();
        let target = crate::commands::saga::normalize_step_target(&step.request.url);
        calls.push(ExpectedCall {
            surface_name: interface_surface_name("http_endpoint", &method, &target),
            method,
            target,
            step_index: idx + 1,
            step_name: step.name.clone(),
            intent_id,
        });
    }
    Ok(SagaExpectation {
        validation_id: saga.id.clone(),
        validation_name: saga.name.clone(),
        calls,
    })
}

fn calls_by_validation(calls: &[CallsEdge]) -> HashMap<&str, Vec<&CallsEdge>> {
    let mut out: HashMap<&str, Vec<&CallsEdge>> = HashMap::new();
    for call in calls {
        out.entry(call.validation_id.as_str())
            .or_default()
            .push(call);
    }
    out
}

fn calls_match_expected(existing: &[&CallsEdge], expected: &[ExpectedCall]) -> bool {
    if existing.len() != expected.len() {
        return false;
    }
    expected.iter().all(|want| {
        existing.iter().any(|got| {
            got.step_index == want.step_index
                && got.step_name == want.step_name
                && got.intent_id == want.intent_id
                && got.interface_name == want.surface_name
        })
    })
}

fn render_plan(plan: &PopulatePlan, dry_run: bool, printer: &Printer) -> Result<()> {
    let p = &plan.interface_from_sagas;
    let gaps = &plan.interface_gaps;
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "dry_run": dry_run,
            "populate": {
                "interface_from_sagas": {
                    "pending": p.is_pending(),
                    "sagas_total": p.sagas_total,
                    "sagas_ready": p.sagas_ready,
                    "sagas_skipped": skipped_json(&p.sagas_skipped),
                    "expected_calls": p.expected_calls,
                    "existing_calls": p.existing_calls,
                    "missing_surfaces": p.missing_surfaces,
                    "stale_call_sets": p.stale_call_sets,
                    "sagas_needing_repopulate": p.sagas_needing_repopulate,
                    "command": POPULATE_INTERFACES_FROM_SAGAS_CMD,
                },
                "interface_gaps": {
                    "pending": gaps.is_pending(),
                    "total": gaps.total(),
                    "surface_without_calls": gaps.surface_without_calls,
                    "boundary_intent_without_calls": gaps.boundary_intent_without_calls,
                    "call_without_validates": gaps.call_without_validates,
                    "examples": interface_gap_examples_json(&gaps.examples),
                    "command": "loom interface gaps",
                }
            }
        }));
        return Ok(());
    }

    println!("── Populate Plan ───────────────────────────────────────────────");
    println!("  interfaces from sagas:");
    println!(
        "    sagas: {} ready, {} skipped, {} total",
        p.sagas_ready,
        p.sagas_skipped.len(),
        p.sagas_total
    );
    println!(
        "    calls: {} expected, {} existing; missing surfaces: {}; stale saga call sets: {}",
        p.expected_calls, p.existing_calls, p.missing_surfaces, p.stale_call_sets
    );
    if p.is_pending() {
        println!("    → Run: {POPULATE_INTERFACES_FROM_SAGAS_CMD}");
    } else {
        println!("    ✓ No deterministic interface backfill pending.");
    }
    if !p.sagas_skipped.is_empty() {
        println!("  skipped saga(s):");
        for skip in &p.sagas_skipped {
            println!(
                "    - {} ({}) — {}",
                skip.name, skip.validation_id, skip.reason
            );
        }
    }
    println!("  interface gaps:");
    println!("    {}", interface_gap_totals_line(gaps));
    if gaps.is_pending() {
        println!("    → Inspect: loom interface gaps");
    } else {
        println!("    {NO_INTERFACE_GAPS_MESSAGE}");
    }
    Ok(())
}

fn skipped_json(skipped: &[SkippedSaga]) -> Vec<serde_json::Value> {
    skipped
        .iter()
        .map(|skip| {
            serde_json::json!({
                "validation_id": skip.validation_id,
                "name": skip.name,
                "reason": skip.reason,
            })
        })
        .collect()
}

pub(crate) fn interface_gaps_json(gaps: &InterfaceGapPlan) -> serde_json::Value {
    serde_json::json!({
        "pending": gaps.is_pending(),
        "total": gaps.total(),
        "surface_without_calls": gaps.surface_without_calls,
        "boundary_intent_without_calls": gaps.boundary_intent_without_calls,
        "call_without_validates": gaps.call_without_validates,
        "examples": interface_gap_examples_json(&gaps.examples),
    })
}

pub(crate) fn interface_gap_totals_line(gaps: &InterfaceGapPlan) -> String {
    format!(
        "total: {} · surfaces without calls: {} · boundary intents without calls: {} · calls without validates: {}",
        gaps.total(),
        gaps.surface_without_calls,
        gaps.boundary_intent_without_calls,
        gaps.call_without_validates
    )
}

fn interface_gap_examples_json(examples: &[InterfaceGap]) -> Vec<serde_json::Value> {
    examples
        .iter()
        .map(|gap| {
            serde_json::json!({
                "kind": gap.kind,
                "summary": gap.summary,
                "surface_id": gap.surface_id,
                "surface": gap.surface,
                "intent_id": gap.intent_id,
                "intent": gap.intent,
                "validation_id": gap.validation_id,
                "validation": gap.validation,
                "suggested_action": gap.suggested_action,
            })
        })
        .collect()
}

pub(crate) fn render_next(
    db: &dyn GraphReadRepository,
    root: &Path,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let gs = db.graph_state(&snapshot)?;
    let plan = plan_with_repo(db, root)?;
    let p = &plan.interface_from_sagas;
    let gaps = &plan.interface_gaps;

    if !p.is_pending() {
        if gaps.is_pending() {
            return render_next_interface_gap(gaps, &gs, printer);
        }
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": "empty",
                "mode": "populate",
                "message": "No deterministic graph-population work or interface-plane gaps are pending.",
                "skipped_sagas": skipped_json(&p.sagas_skipped),
                "interface_gaps": interface_gaps_json(gaps),
                "next_step": gs.next_action,
                "graph_state": pulse_json(&gs),
            }));
        } else {
            println!("✓ No deterministic graph-population work is pending.");
            if !p.sagas_skipped.is_empty() {
                println!("  Skipped saga(s) need manual repair; run `loom populate plan --json` for details.");
            }
            println!();
            println!("  {}", crate::output::fmt_pulse(&gs));
            println!("  → Next: {}", gs.next_action);
        }
        return Ok(());
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode": "populate",
            "kind": "interface_from_sagas",
            "owner_role": "builder",
            "effort": "low",
            "dispatch": "this is builder work — fills derived graph structure from existing evidence. Whoever takes it declares `LOOM_AGENT=llm:builder` (or stay bare `llm` for solo); its queue is `loom next --mode populate`.",
            "priority_score": p.sagas_needing_repopulate,
            "sagas_needing_repopulate": p.sagas_needing_repopulate,
            "missing_surfaces": p.missing_surfaces,
            "stale_call_sets": p.stale_call_sets,
            "expected_calls": p.expected_calls,
            "existing_calls": p.existing_calls,
            "skipped_sagas": skipped_json(&p.sagas_skipped),
            "suggested_action": POPULATE_INTERFACES_FROM_SAGAS_CMD,
            "graph_state": pulse_json(&gs),
        }));
        return Ok(());
    }

    println!(
        "── Next Populate Item  [interface_from_sagas  priority={}] ─────────────",
        p.sagas_needing_repopulate
    );
    println!();
    println!("Backfill InterfaceSurface/CALLS from registered saga specs.");
    println!(
        "  missing surfaces: {} · stale saga call sets: {} · expected calls: {} · existing calls: {}",
        p.missing_surfaces, p.stale_call_sets, p.expected_calls, p.existing_calls
    );
    println!();
    println!("  Run: {POPULATE_INTERFACES_FROM_SAGAS_CMD}");
    Ok(())
}

fn render_next_interface_gap(
    gaps: &InterfaceGapPlan,
    gs: &crate::db::queries::GraphState,
    printer: &Printer,
) -> Result<()> {
    if printer.json {
        printer.print_json(&serde_json::json!({
            "mode": "populate",
            "kind": "interface_gaps",
            "owner_role": "builder",
            "effort": "mid",
            "dispatch": "this is builder work — repair the populated interface plane or hand ambiguous binding back through graph notes. Whoever takes it declares `LOOM_AGENT=llm:builder` (or stays bare `llm` for solo); its queue is `loom next --mode populate`.",
            "priority_score": gaps.total(),
            "interface_gaps": interface_gaps_json(gaps),
            "suggested_action": "loom interface gaps",
            "graph_state": pulse_json(gs),
        }));
        return Ok(());
    }

    println!(
        "── Next Populate Item  [interface_gaps  priority={}] ─────────────",
        gaps.total()
    );
    println!();
    println!("Audit the populated InterfaceSurface/CALLS plane.");
    println!(
        "  surfaces without calls: {} · boundary intents without calls: {} · calls without validates: {}",
        gaps.surface_without_calls,
        gaps.boundary_intent_without_calls,
        gaps.call_without_validates
    );
    if let Some(gap) = gaps.examples.first() {
        println!("  top: {} — {}", gap.kind, gap.summary);
    }
    println!();
    println!("  Inspect: loom interface gaps");
    Ok(())
}
