use super::*;

pub(crate) fn journey_rehearse_cold(
    graph: Option<&Path>,
    journey_key: &str,
    json_output: bool,
) -> Result<()> {
    if !json_output {
        bail!("`loom journey rehearse-cold` requires --json");
    }
    let store = open_read(graph)?;
    let (journey, _, _) = load_registered_journey(&store, journey_key)?;
    let report = crate::release::rehearse_cold_journey(store.root(), &journey.name)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
pub(crate) fn journey_resume(
    graph: Option<&Path>,
    token: &str,
    choice: &str,
    human_decision: &str,
    free_form: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let pending = crate::journey_runtime::pending_continuation(token)?;
    let store = open(graph)?;
    if store.root() != pending.live_root {
        bail!(
            "Journey gate resume token belongs to a different graph root ('{}')",
            pending.live_root.display()
        );
    }
    let executor = store.execution_identity().actor();
    match crate::journey::resume_and_settle_compiled_validation(
        &store,
        token,
        crate::journey_gate::ResumeAnswer {
            choice_id: choice.to_string(),
            human_decision: human_decision.to_string(),
            free_form: free_form.map(str::to_string),
        },
        &executor,
    )? {
        crate::journey::InteractiveJourneyRun::Completed(report) => {
            emit_report(&report, json_output)
        }
        crate::journey::InteractiveJourneyRun::Pending(pending) => emit_runtime_value(
            serde_json::to_value(&pending)?,
            json_output,
            &format!(
                "Journey '{}:{}' is still waiting for a human decision",
                pending.binding.journey_id, pending.binding.profile
            ),
        ),
    }
}
#[derive(Clone)]
struct CompileSource {
    journey: Node,
    spec: crate::journey::JourneySpec,
    semantic_hash: String,
    surface: Node,
    surface_hash: String,
    setup: Option<crate::journey::SurfaceSetup>,
    bindings: Vec<crate::journey::SurfaceBinding>,
    operations: Vec<crate::journey::CliOperation>,
    derived_intents: Vec<Node>,
}

struct CompileProduct {
    proof: crate::journey_runtime::CompiledJourneyProof,
    spec: crate::journey::JourneySpec,
    validation_id: String,
    root: PathBuf,
    identity: crate::identity::ExecutionIdentity,
    artifact: PathBuf,
    cache_regenerated: bool,
}

#[derive(Clone, Default)]
struct DesiredExerciseTarget {
    surface_locator: Option<String>,
    operation_entries: Vec<crate::journey::JourneyOperationExerciseFacet>,
}

fn compile_source(store: &Store, journey_key: &str, profile: &str) -> Result<CompileSource> {
    let (journey, spec, semantic_hash) = load_registered_journey(store, journey_key)?;
    if !spec.profiles.contains_key(profile) {
        bail!("Journey '{}' has no profile '{profile}'", journey.name);
    }
    let readiness = crate::completeness::journey_readiness(store, &journey)?;
    if !readiness.derived {
        bail!(
            "Journey '{}' is not compile-ready: derivation is absent or stale. Run `loom journey derive {} --json`",
            journey.name,
            spec.id
        );
    }
    if !readiness.derivations_ratified {
        bail!(
            "Journey '{}' is not compile-ready: derivation acceptance is pending. Run `loom journey derive {} --json`, present its structured human_gate options, stop for the human's exact substantive answer, and only then use derive-accept",
            journey.name,
            spec.id
        );
    }
    if !readiness.implemented {
        bail!("Journey '{}' is not compile-ready: accepted technical intents are not implemented and realizing-grounded. Run `loom next --mode build --json`", journey.name);
    }

    let mut current_surfaces = Vec::new();
    for edge in store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)? {
        if !matches!(
            edge.status,
            InspectionStatus::Uninspected | InspectionStatus::Passing
        ) || store
            .get_facet(&edge.id, TargetKind::Edge, "journey_hash")?
            .as_deref()
            != Some(semantic_hash.as_str())
        {
            continue;
        }
        let surface = store
            .get_node(&edge.to_id)?
            .ok_or_else(|| anyhow!("accepted surface target '{}' is missing", edge.to_id))?;
        let bindings = facet_json::<Vec<crate::journey::SurfaceBinding>>(
            store,
            &edge.id,
            "operation_bindings",
        )?;
        let setup = store
            .get_facet(&edge.id, TargetKind::Edge, "setup")?
            .map(|raw| {
                serde_json::from_str::<crate::journey::SurfaceSetup>(&raw)
                    .with_context(|| format!("edge '{}' has invalid setup", edge.id))
            })
            .transpose()?;
        if let Some(setup) = &setup {
            setup.validate_for_store(store)?;
        }
        current_surfaces.push((edge, surface, setup, bindings));
    }
    let [(surface_edge, surface, setup, bindings)] = current_surfaces.as_slice() else {
        bail!(
            "Journey '{}' requires exactly one current hash-bound CLI surface (found {})",
            journey.name,
            current_surfaces.len()
        );
    };
    if surface.status == "quarantined" {
        bail!(
            "Journey '{}' executable CLI surface was imported and is quarantined; locally re-authorize the exact contract with `loom journey surface-accept {} --manifest <manifest.json>` before compile/run",
            journey.name,
            journey.name
        );
    }
    if surface.node_type != NodeType::InterfaceSurface
        || surface.body.get("schema").and_then(Value::as_str)
            != Some(crate::journey::INTERFACE_SURFACE_SCHEMA)
        || surface.body.get("kind").and_then(Value::as_str) != Some("cli")
    {
        bail!(
            "Journey '{}' accepted surface is not a reusable CLI",
            journey.name
        );
    }
    let operations: Vec<crate::journey::CliOperation> = serde_json::from_value(
        surface
            .body
            .get("operations")
            .cloned()
            .ok_or_else(|| anyhow!("InterfaceSurface '{}' has no operations", surface.name))?,
    )
    .with_context(|| format!("decoding InterfaceSurface '{}' operations", surface.name))?;
    let surface_hash = crate::journey::surface_projection_hash(store, &journey)?
        .ok_or_else(|| anyhow!("Journey '{}' has no surface projection hash", journey.name))?;

    let mut exposed_live = 0usize;
    for edge in store.edges_with(Some(EdgeKind::Exposes), Some(&surface.id), None)? {
        if !matches!(
            edge.status,
            InspectionStatus::Uninspected | InspectionStatus::Passing
        ) {
            continue;
        }
        let codefile = store.get_node(&edge.to_id)?.ok_or_else(|| {
            anyhow!(
                "InterfaceSurface exposes missing CodeFile '{}',",
                edge.to_id
            )
        })?;
        if codefile.node_type != NodeType::CodeFile || !store.root().join(&codefile.name).is_file()
        {
            continue;
        }
        exposed_live += 1;
    }
    if exposed_live == 0 {
        bail!(
            "Journey '{}' CLI surface exposes no live CodeFile",
            journey.name
        );
    }

    let mut derived_intents = Vec::new();
    for id in &readiness.derived_intent_ids {
        let intent = store
            .get_node(id)?
            .ok_or_else(|| anyhow!("derived Intent '{id}' is missing"))?;
        derived_intents.push(intent);
    }
    derived_intents.sort_by(|left, right| left.id.cmp(&right.id));

    // The facet is the compiler input; mentioning it here prevents a future
    // refactor from accidentally compiling another surface with the same body.
    let _ = surface_edge;
    Ok(CompileSource {
        journey,
        spec,
        semantic_hash,
        surface: surface.clone(),
        surface_hash,
        setup: setup.clone(),
        bindings: bindings.clone(),
        operations,
        derived_intents,
    })
}

fn compile_internal(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
) -> Result<CompileProduct> {
    let store = open(graph)?;
    let root = store.root().to_path_buf();
    let identity = store.execution_identity();
    let source = compile_source(&store, journey_key, profile)?;
    let proof = crate::journey_runtime::compile_surface(
        &source.spec,
        &source.surface_hash,
        profile,
        source.operations.clone(),
        source.setup.as_ref(),
        &source.bindings,
    )?;
    let cache_regenerated = !crate::journey_runtime::cache_matches(&root, &proof)?;
    let validation_name = format!("journey:{}:{profile}", source.journey.name);
    let body = json!({
        "type": "journey",
        "journey_hash": source.semantic_hash,
        "surface_hash": source.surface_hash,
        "profile": profile,
        "compiler_version": crate::journey::JOURNEY_COMPILER_VERSION,
    });

    let mut candidates: Vec<Node> = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)?
        .into_iter()
        .filter(|node| node.name == validation_name)
        .collect();
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    if candidates
        .iter()
        .any(|node| node.body.get("type").and_then(Value::as_str) != Some("journey"))
    {
        bail!("Validation name '{validation_name}' is occupied by a non-Journey proof");
    }

    // One shared projection: the same canonical operation-exercise
    // expectation compile writes, grading checks, sync reconciles, and
    // doctor reports against. Preview and compile use it too, so their
    // covered-file evidence cannot disagree.
    let projection = crate::journey_exercises::expected_projection(&store, &source.journey)
        .with_context(|| {
            format!(
                "Journey '{}' has no current operation-exercise projection",
                source.journey.name
            )
        })?;
    let validation = {
        let tx = store.begin()?;
        let validation = match candidates.first() {
            Some(node) => node.clone(),
            None => store.add_node(
                NodeType::Validation,
                &validation_name,
                "compiler-owned Journey proof profile",
                "not_run",
                body.clone(),
            )?,
        };
        for duplicate in candidates.iter().skip(1) {
            store.delete_node(&duplicate.id)?;
        }
        let body_changed = validation.body != body;
        if body_changed {
            store.set_node_body(&validation.id, &body)?;
            store.reset_validation_status_for_sync(&validation.id)?;
        }

        let desired_validates: BTreeSet<String> = source
            .derived_intents
            .iter()
            .map(|intent| intent.id.clone())
            .collect();
        let mut exercise_targets: BTreeMap<String, DesiredExerciseTarget> = BTreeMap::new();
        for entry in &projection.public_entries {
            let target = exercise_targets
                .entry(entry.codefile_id.clone())
                .or_default();
            target.surface_locator = entry.locator.clone();
        }
        for exercise in &projection.exercises {
            let target = exercise_targets
                .entry(exercise.codefile_id.clone())
                .or_default();
            target
                .operation_entries
                .push(crate::journey::JourneyOperationExerciseFacet {
                    operation_id: exercise.operation_id.clone(),
                    exercise_id: exercise.exercise_id.clone(),
                    observed_by: exercise.observed_by.clone(),
                    locator: exercise.locator.clone(),
                });
        }
        for target in exercise_targets.values_mut() {
            target.operation_entries.sort_by(|left, right| {
                left.operation_id
                    .cmp(&right.operation_id)
                    .then_with(|| left.exercise_id.cmp(&right.exercise_id))
            });
        }
        let desired_exercises: BTreeSet<String> = exercise_targets.keys().cloned().collect();
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Proves,
            std::iter::once(source.journey.id.clone()).collect(),
            body_changed,
        )?;
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Validates,
            desired_validates,
            body_changed,
        )?;
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Calls,
            std::iter::once(source.surface.id.clone()).collect(),
            body_changed,
        )?;
        reconcile_topology(
            &store,
            &validation.id,
            EdgeKind::Exercises,
            desired_exercises,
            body_changed,
        )?;
        for (codefile_id, target) in &exercise_targets {
            let edge = store.ensure_edge(EdgeKind::Exercises, &validation.id, codefile_id)?;
            let mut locators = BTreeSet::new();
            if let Some(locator) = &target.surface_locator {
                locators.insert(locator.clone());
                store.set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "surface_locator",
                    locator,
                    TruthClass::Asserted,
                )?;
            } else {
                store.clear_facet(&edge.id, TargetKind::Edge, "surface_locator")?;
            }
            for entry in &target.operation_entries {
                locators.insert(entry.locator.clone());
            }
            let aggregated = locators.into_iter().collect::<Vec<_>>().join(";");
            store.set_facet(
                &edge.id,
                TargetKind::Edge,
                "locator",
                &aggregated,
                TruthClass::Asserted,
            )?;
            if target.operation_entries.is_empty() {
                store.clear_facet(&edge.id, TargetKind::Edge, "journey_operation_exercises")?;
            } else {
                store.set_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "journey_operation_exercises",
                    &serde_json::to_string(&target.operation_entries)?,
                    TruthClass::Asserted,
                )?;
            }
        }
        tx.commit()?;
        store
            .get_node(&validation.id)?
            .ok_or_else(|| anyhow!("compiled Validation vanished"))?
    };
    crate::journey::resettle_uninspected_compiler_topology(&store, &validation.id)?;
    let artifact = crate::journey_runtime::write_proof(&root, &proof)?;
    Ok(CompileProduct {
        proof,
        spec: source.spec,
        validation_id: validation.id,
        root,
        identity,
        artifact,
        cache_regenerated,
    })
}

fn reconcile_topology(
    store: &Store,
    validation_id: &str,
    kind: EdgeKind,
    desired: BTreeSet<String>,
    reset: bool,
) -> Result<()> {
    let existing = store.edges_with(Some(kind), Some(validation_id), None)?;
    for edge in &existing {
        let current = matches!(
            edge.status,
            InspectionStatus::Uninspected | InspectionStatus::Passing
        );
        if reset || !desired.contains(&edge.to_id) || !current {
            store.delete_edge(&edge.id)?;
        }
    }
    for target in desired {
        store.ensure_edge(kind, validation_id, &target)?;
    }
    Ok(())
}

fn facet_json<T: serde::de::DeserializeOwned>(
    store: &Store,
    edge_id: &str,
    key: &str,
) -> Result<T> {
    let raw = store
        .get_facet(edge_id, TargetKind::Edge, key)?
        .ok_or_else(|| anyhow!("edge '{}' has no {key} facet", edge_id))?;
    serde_json::from_str(&raw).with_context(|| format!("edge '{}' has invalid {key}", edge_id))
}

pub(crate) fn journey_compile(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    json_output: bool,
) -> Result<()> {
    let product = compile_internal(graph, journey_key, profile)?;
    emit_runtime_value(
        json!({
            "compiled": true,
            "journey_id": product.proof.journey_id,
            "profile": product.proof.profile,
            "journey_hash": product.proof.journey_hash,
            "surface_hash": product.proof.surface_hash,
            "compiler_version": product.proof.compiler_version,
            "validation_id": product.validation_id,
            "artifact": product.artifact.strip_prefix(&product.root).unwrap_or(&product.artifact),
            "cache_regenerated": product.cache_regenerated,
        }),
        json_output,
        "compiled Journey proof",
    )
}

pub(crate) fn journey_run(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    json_output: bool,
) -> Result<()> {
    let product = compile_internal(graph, journey_key, profile)
        .map_err(|error| run_failure(json_output, journey_key, profile, "compile", error))?;
    let store = crate::store::Store::open_with_identity(&product.root, product.identity)
        .map_err(|error| run_failure(json_output, journey_key, profile, "open", error))?;
    match crate::journey::run_interactive_and_settle_compiled_validation(
        &store,
        &product.validation_id,
        &BTreeMap::new(),
    ) {
        Ok(crate::journey::InteractiveJourneyRun::Completed(report)) => {
            emit_report(&report, json_output)
        }
        Ok(crate::journey::InteractiveJourneyRun::Pending(pending)) => emit_runtime_value(
            serde_json::to_value(&pending)?,
            json_output,
            &format!(
                "Journey '{}:{}' is waiting for a human decision",
                pending.binding.journey_id, pending.binding.profile
            ),
        ),
        Err(error) => Err(run_failure(
            json_output,
            journey_key,
            profile,
            "settle",
            error,
        )),
    }
}

/// `journey run` failures must stay machine-readable under `--json`: attach the
/// staged envelope so `main` prints one stdout document (journey, profile,
/// stage, detail) instead of scraping stderr. Human stderr still carries the
/// same context line as before.
fn run_failure(
    json_output: bool,
    journey_key: &str,
    profile: &str,
    stage: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    let detail = format!("{error:#}");
    let context = format!("journey run '{journey_key}:{profile}' failed during {stage}");
    if json_output {
        return super::JsonErrorEnvelope::new(
            json!({
                "status": "error",
                "journey": journey_key,
                "profile": profile,
                "stage": stage,
                "detail": detail,
            }),
            format!("{context}: {detail}"),
        )
        .into_error();
    }
    error.context(context)
}

pub(crate) fn journey_diagnose(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    raw_inputs: &[String],
    json_output: bool,
) -> Result<()> {
    let overrides = crate::journey_runtime::parse_overrides(raw_inputs)?;
    let product = compile_internal(graph, journey_key, profile)?;
    let report = match crate::harness::acquire(&product.root, "journey diagnose", &product.identity)
    {
        Ok(_harness) => crate::journey_runtime::execute(
            &product.root,
            &product.spec,
            &product.proof,
            &overrides,
        ),
        Err(error) => blocked_report(&product.proof, error.to_string()),
    };
    emit_report(&report, json_output)
}

pub(crate) fn journey_freeze(
    graph: Option<&Path>,
    journey_key: &str,
    profile: &str,
    json_output: bool,
) -> Result<()> {
    let product = compile_internal(graph, journey_key, profile)?;
    let report = match crate::harness::acquire(&product.root, "journey freeze", &product.identity) {
        Ok(_harness) => crate::journey_runtime::execute(
            &product.root,
            &product.spec,
            &product.proof,
            &BTreeMap::new(),
        ),
        Err(error) => blocked_report(&product.proof, error.to_string()),
    };
    if report.status != crate::journey_runtime::RuntimeStatus::Passed {
        return emit_report(&report, json_output);
    }
    let baseline = crate::journey_runtime::write_baseline(&product.root, &report)?;
    emit_runtime_value(
        json!({
            "frozen": true,
            "journey_id": report.journey_id,
            "profile": report.profile,
            "baseline": baseline.strip_prefix(&product.root).unwrap_or(&baseline),
            "assertions_passed": report.assertions_passed,
        }),
        json_output,
        "froze Journey baseline",
    )
}

pub(crate) fn journey_drift(
    graph: Option<&Path>,
    journey_key: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let journeys = match journey_key {
        Some(key) => vec![resolve_journey(&store, key)?],
        None => store.list_nodes(Some(NodeType::Journey), usize::MAX)?,
    };
    let mut rows = Vec::new();
    for journey in journeys {
        let artifact = journey
            .body
            .get("artifact")
            .and_then(Value::as_str)
            .unwrap_or("");
        let spec = match crate::journey::parse(&store.root().join(artifact)) {
            Ok(spec) => spec,
            Err(error) => {
                rows.push(json!({
                    "journey_id": journey.name,
                    "current": false,
                    "repair_class": "authored_artifact_absent_or_invalid",
                    "next_command": Value::Null,
                    "next_action": format!("Inspect product evidence and repair the authored Journey artifact at '{}'; no automatic mutation is safe", artifact),
                    "detail": error.to_string(),
                }));
                continue;
            }
        };
        let readiness = crate::completeness::journey_readiness(&store, &journey)?;
        for profile_id in spec.profiles.keys() {
            match compile_source(&store, &journey.id, profile_id).and_then(|source| {
                crate::journey_runtime::compile_surface(
                    &source.spec,
                    &source.surface_hash,
                    profile_id,
                    source.operations,
                    source.setup.as_ref(),
                    &source.bindings,
                )
            }) {
                Ok(proof) => {
                    let cache_current =
                        crate::journey_runtime::cache_matches(store.root(), &proof)?;
                    let baseline_current =
                        crate::journey_runtime::baseline_current(store.root(), &proof)?;
                    rows.push(json!({
                        "journey_id": journey.name,
                        "profile": profile_id,
                        "cache_current": cache_current,
                        "baseline_current": baseline_current,
                        "current": cache_current && baseline_current != Some(false),
                        "repair_class": if !cache_current { "compiled_cache_stale" } else if baseline_current == Some(false) { "baseline_stale" } else { "current" },
                        "next_command": if !cache_current {
                            Some(format!("loom journey compile {} --profile {}", crate::workitem::q(&journey.name), crate::workitem::q(profile_id)))
                        } else if baseline_current == Some(false) {
                            Some(format!("loom journey freeze {} --profile {}", crate::workitem::q(&journey.name), crate::workitem::q(profile_id)))
                        } else { None },
                    }));
                }
                Err(error) => {
                    let detail = error.to_string();
                    let (repair_class, next_command) =
                        classify_drift_repair(&journey.name, &readiness);
                    let next_action = next_command.is_none().then_some(
                        "Inspect the reported blocker and product/code evidence; no automatic mutation is safe",
                    );
                    rows.push(json!({
                        "journey_id": journey.name,
                        "profile": profile_id,
                        "current": false,
                        "repair_class": repair_class,
                        "next_command": next_command,
                        "next_action": next_action,
                        "detail": detail,
                    }))
                }
            }
        }
    }
    let stale = rows
        .iter()
        .filter(|row| row.get("current").and_then(Value::as_bool) != Some(true))
        .count();
    emit_runtime_value(
        json!({"journeys": rows, "stale": stale}),
        json_output,
        if stale == 0 {
            "Journey compiled artifacts are current"
        } else {
            "Journey compiled artifact drift detected"
        },
    )
}

fn classify_drift_repair(
    journey: &str,
    readiness: &crate::completeness::JourneyReadiness,
) -> (&'static str, Option<String>) {
    let journey = crate::workitem::q(journey);
    if !readiness.derived {
        // Derive is inspect-only. It may reveal an authority gate, but drift
        // must not request authority before Loom says acceptance is pending.
        return (
            "derivation_absent_or_stale",
            Some(format!("loom journey derive {journey} --json")),
        );
    }
    if !readiness.derivations_ratified {
        return (
            "derivation_acceptance_pending",
            Some(format!("loom journey derive {journey} --json")),
        );
    }
    if !readiness.implemented {
        return (
            "not_implemented_or_ungrounded",
            Some("loom next --mode build --json".into()),
        );
    }
    if !readiness.surfaced {
        return (
            "no_accepted_current_surface",
            Some(format!("loom journey surface {journey} --json")),
        );
    }
    ("inspection_required", None)
}

fn blocked_report(
    proof: &crate::journey_runtime::CompiledJourneyProof,
    detail: String,
) -> crate::journey_runtime::RuntimeReport {
    crate::journey_runtime::RuntimeReport {
        journey_id: proof.journey_id.clone(),
        profile: proof.profile.clone(),
        journey_hash: proof.journey_hash.clone(),
        surface_hash: proof.surface_hash.clone(),
        status: crate::journey_runtime::RuntimeStatus::Blocked,
        assertions_passed: 0,
        assertions_failed: 0,
        detail: Some(detail),
        setup: Vec::new(),
        file_transitions: Vec::new(),
        steps: Vec::new(),
        captures: BTreeMap::new(),
        passed_assertions: Vec::new(),
        failed_assertions: Vec::new(),
    }
}
