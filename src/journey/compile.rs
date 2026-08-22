use super::spec::{canonicalize_value, parse, JourneySpec};
use super::surface_ops::{CliOperation, SurfaceBinding};
use super::surface_setup::SurfaceSetup;
use super::{INTERFACE_SURFACE_SCHEMA, JOURNEY_COMPILER_VERSION};
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Deterministic content hash of a Journey's accepted reusable surface
/// projection. Consumers use this as a compiler/routing cache key; it is
/// derived on read and is deliberately not stored as another stale facet.
pub fn surface_projection_hash(
    store: &crate::store::Store,
    journey: &crate::model::Node,
) -> Result<Option<String>> {
    use crate::model::{EdgeKind, TargetKind};

    let surface_edges = store.edges_with(Some(EdgeKind::Surfaces), Some(&journey.id), None)?;
    if surface_edges.is_empty() {
        return Ok(None);
    }

    let mut surfaces: Vec<(String, Value)> = Vec::new();
    for edge in surface_edges {
        let surface = store.get_node(&edge.to_id)?.ok_or_else(|| {
            anyhow!(
                "Journey '{}' has a Surfaces edge to missing node '{}'",
                journey.name,
                edge.to_id
            )
        })?;
        let stable_id = surface
            .body
            .get("stable_id")
            .and_then(Value::as_str)
            .unwrap_or(&surface.id)
            .to_string();
        let operation_bindings =
            match store.get_facet(&edge.id, TargetKind::Edge, "operation_bindings")? {
                Some(raw) => canonicalize_value(serde_json::from_str(&raw).with_context(|| {
                    format!(
                        "Surfaces edge '{}' has malformed operation_bindings JSON",
                        edge.id
                    )
                })?),
                None => Value::Null,
            };
        let setup = match store.get_facet(&edge.id, TargetKind::Edge, "setup")? {
            Some(raw) => canonicalize_value(serde_json::from_str(&raw).with_context(|| {
                format!("Surfaces edge '{}' has malformed setup JSON", edge.id)
            })?),
            None => Value::Null,
        };

        let mut exposes: Vec<(String, Value)> = Vec::new();
        for exposed in store.edges_with(Some(EdgeKind::Exposes), Some(&surface.id), None)? {
            let codefile = store.get_node(&exposed.to_id)?.ok_or_else(|| {
                anyhow!(
                    "InterfaceSurface '{}' exposes missing node '{}'",
                    surface.name,
                    exposed.to_id
                )
            })?;
            let sort_key = format!("{}\0{}", codefile.name, codefile.id);
            exposes.push((
                sort_key,
                json!({
                    "codefile_name": codefile.name,
                    "codefile_id": codefile.id,
                    "locator": store.get_facet(&exposed.id, TargetKind::Edge, "locator")?,
                }),
            ));
        }
        exposes.sort_by(|a, b| a.0.cmp(&b.0));
        let sort_key = format!("{}\0{}", stable_id, surface.id);
        surfaces.push((
            sort_key,
            json!({
                "stable_id": stable_id,
                "surface_id": surface.id,
                "surface_body": canonicalize_value(surface.body),
                "journey_hash": store.get_facet(
                    &edge.id,
                    TargetKind::Edge,
                    "journey_hash"
                )?,
                "setup": setup,
                "operation_bindings": operation_bindings,
                "exposes": exposes.into_iter().map(|(_, row)| row).collect::<Vec<_>>(),
            }),
        ));
    }
    surfaces.sort_by(|a, b| a.0.cmp(&b.0));
    let projection = canonicalize_value(json!({
        "journey_semantic_hash": journey.body.get("semantic_hash"),
        "surfaces": surfaces.into_iter().map(|(_, row)| row).collect::<Vec<_>>(),
    }));
    Ok(Some(crate::artifact::fingerprint(&serde_json::to_string(
        &projection,
    )?)))
}

/// Compile the canonical proof of `journey`'s current accepted surface.
/// Settlement uses this to refuse observations of caller-authored proofs that
/// merely copied identity hashes.
fn compile_accepted_proof(
    store: &crate::store::Store,
    journey: &crate::model::Node,
    profile: &str,
) -> Result<crate::journey_runtime::CompiledJourneyProof> {
    let (_, proof) = compile_accepted_source(store, journey, profile)?;
    Ok(proof)
}

/// The canonical accepted-surface compilation for `journey`/`profile`, with
/// the authored spec it was compiled from. This is the single derivation the
/// Store-owned settlement entrypoints use for execution, and settlement uses
/// for re-derivation: caller-supplied specs and proofs are never inputs.
fn compile_accepted_source(
    store: &crate::store::Store,
    journey: &crate::model::Node,
    profile: &str,
) -> Result<(JourneySpec, crate::journey_runtime::CompiledJourneyProof)> {
    use crate::model::{EdgeKind, InspectionStatus, NodeType, TargetKind};

    let artifact = journey
        .body
        .get("artifact")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Journey '{}' has no artifact", journey.name))?;
    let spec = parse(&store.root().join(artifact))?;
    if spec.id != journey.name {
        bail!(
            "Journey artifact '{}' now declares stable id '{}', not '{}'",
            artifact,
            spec.id,
            journey.name
        );
    }
    let semantic_hash = spec.semantic_hash()?;
    let registered_hash = journey
        .body
        .get("semantic_hash")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Journey '{}' has no semantic_hash", journey.name))?;
    if registered_hash != semantic_hash {
        bail!(
            "Journey '{}' registration no longer matches its authored artifact",
            journey.name
        );
    }
    if !spec.profiles.contains_key(profile) {
        bail!("Journey '{}' has no profile '{profile}'", journey.name);
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
        let bindings = match store.get_facet(&edge.id, TargetKind::Edge, "operation_bindings")? {
            Some(raw) => serde_json::from_str::<Vec<SurfaceBinding>>(&raw)
                .with_context(|| format!("edge '{}' has invalid operation_bindings", edge.id))?,
            None => bail!(
                "Journey '{}' surface has no operation bindings",
                journey.name
            ),
        };
        let setup = store
            .get_facet(&edge.id, TargetKind::Edge, "setup")?
            .map(|raw| {
                serde_json::from_str::<SurfaceSetup>(&raw)
                    .with_context(|| format!("edge '{}' has invalid setup", edge.id))
            })
            .transpose()?;
        current_surfaces.push((surface, setup, bindings));
    }
    let [(surface, setup, bindings)] = current_surfaces.as_slice() else {
        bail!(
            "Journey '{}' requires exactly one current hash-bound CLI surface (found {})",
            journey.name,
            current_surfaces.len()
        );
    };
    if surface.status == "quarantined"
        || surface.node_type != NodeType::InterfaceSurface
        || surface.body.get("schema").and_then(Value::as_str) != Some(INTERFACE_SURFACE_SCHEMA)
        || surface.body.get("kind").and_then(Value::as_str) != Some("cli")
    {
        bail!(
            "Journey '{}' accepted surface is not a current reusable CLI",
            journey.name
        );
    }
    let operations: Vec<CliOperation> = serde_json::from_value(
        surface
            .body
            .get("operations")
            .cloned()
            .ok_or_else(|| anyhow!("InterfaceSurface '{}' has no operations", surface.name))?,
    )
    .with_context(|| format!("decoding InterfaceSurface '{}' operations", surface.name))?;
    let surface_hash = surface_projection_hash(store, journey)?
        .ok_or_else(|| anyhow!("Journey '{}' has no surface projection hash", journey.name))?;
    let proof = crate::journey_runtime::compile_surface(
        &spec,
        &surface_hash,
        profile,
        operations,
        setup.as_ref(),
        bindings,
    )?;
    Ok((spec, proof))
}

/// Settle a compiled Journey Validation from a sealed runtime observation.
///
/// The observation can only be minted by the compiler-owned Journey executor,
/// and only the Store-owned guarded entrypoints
/// ([`run_and_settle_compiled_validation`], [`run_interactive_and_settle_compiled_validation`],
/// [`resume_and_settle_compiled_validation`]) mark one trusted for settlement.
/// The public execution APIs ([`crate::journey_runtime::execute`],
/// [`crate::journey_runtime::execute_observed`],
/// [`crate::journey_runtime::execute_interactive`],
/// [`crate::journey_runtime::resume_interactive`]) mint ordinary untrusted
/// observations that are refused here.
///
/// Every trust-relevant input is re-derived from the store and compared with
/// what the runtime persisted at execution time: the canonical proof, the
/// operation-exercise projection, the covered-file hashes (persisted exactly
/// as captured before execution — never resampled into evidence), the
/// execution root, and the executable boundary. A mismatch in any of them —
/// or a caller-selected root, proof, projection, or coverage — fails closed.
pub fn settle_compiled_validation(
    store: &crate::store::Store,
    validation_id: &str,
    observed: &crate::journey_runtime::JourneyObservation,
) -> Result<()> {
    use crate::model::{Claim, EdgeKind, InspectionStatus, NodeType, RunProducer};
    use crate::store::{Assertion, Subject};

    // Trust provenance: only the Store-owned guarded runtime may mint
    // evidence eligible for trusted assertion provenance.
    if !observed.is_trusted() {
        bail!(
            "compiled Journey observation was not minted by the Store-owned guarded runtime; \
             the public execution APIs produce ordinary untrusted reports that cannot settle \
             trusted assertion provenance"
        );
    }
    let anchors = observed
        .anchors()
        .ok_or_else(|| anyhow!("compiled Journey observation carries no execution anchors"))?;
    let canonical_root = store
        .root()
        .canonicalize()
        .with_context(|| format!("canonicalizing graph root {}", store.root().display()))?;
    if anchors.execution_root != canonical_root {
        bail!(
            "compiled Journey observation was executed at a different root \
             ('{}', not this store's '{}')",
            anchors.execution_root.display(),
            canonical_root.display()
        );
    }

    let report = observed.report();
    let proof = observed.proof();
    let validation = store
        .get_node(validation_id)?
        .ok_or_else(|| anyhow!("validation '{validation_id}' is missing"))?;
    if validation.node_type != NodeType::Validation
        || validation
            .body
            .get("type")
            .and_then(serde_json::Value::as_str)
            != Some("journey")
        || validation
            .body
            .get("profile")
            .and_then(serde_json::Value::as_str)
            != Some(proof.profile.as_str())
        || validation
            .body
            .get("compiler_version")
            .and_then(serde_json::Value::as_str)
            != Some(JOURNEY_COMPILER_VERSION)
        || proof.compiler_version != JOURNEY_COMPILER_VERSION
        || validation
            .body
            .get("journey_hash")
            .and_then(serde_json::Value::as_str)
            != Some(proof.journey_hash.as_str())
        || validation
            .body
            .get("surface_hash")
            .and_then(serde_json::Value::as_str)
            != Some(proof.surface_hash.as_str())
        || report.journey_id != proof.journey_id
        || report.profile != proof.profile
        || report.journey_hash != proof.journey_hash
        || report.surface_hash != proof.surface_hash
        || (report.status != crate::journey_runtime::RuntimeStatus::Blocked
            && !observed.matches_compiled_proof())
    {
        bail!(
            "compiled Journey observation does not match validation '{}' compiled proof",
            validation.name
        );
    }

    let proves = store.edges_with(Some(EdgeKind::Proves), Some(validation_id), None)?;
    let [proves] = proves.as_slice() else {
        bail!(
            "compiled Journey validation '{}' must prove exactly one Journey",
            validation.name
        );
    };
    let journey = store
        .get_node(&proves.to_id)?
        .ok_or_else(|| anyhow!("compiled Journey target is missing"))?;
    if journey.node_type != NodeType::Journey
        || journey
            .body
            .get("semantic_hash")
            .and_then(serde_json::Value::as_str)
            != Some(proof.journey_hash.as_str())
        || proof.journey_id != journey.name
    {
        bail!(
            "compiled Journey observation does not match the Journey proved by '{}'",
            validation.name
        );
    }

    let canonical = compile_accepted_proof(store, &journey, &proof.profile)?;
    if crate::journey_runtime::canonical_bytes(proof)?
        != crate::journey_runtime::canonical_bytes(&canonical)?
    {
        bail!(
            "compiled Journey observation is not the canonical accepted-surface proof for '{}'",
            validation.name
        );
    }

    let projection = crate::journey_exercises::expected_projection(store, &journey)?;
    let covered_files = projection.covered_files();

    // The persisted covered set must be exactly the store's current
    // projection: no caller-selected coverage, and no drift since execution.
    let persisted: BTreeSet<&str> = anchors.covered_hashes.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = covered_files.iter().map(String::as_str).collect();
    if persisted != expected {
        bail!(
            "compiled Journey observation does not match the current operation-exercise \
             projection (projection changed between execution and settlement)"
        );
    }
    // The covered files must still hash to the execution-time hashes. The
    // persisted hashes are the evidence; settlement never resamples them.
    for file in &covered_files {
        let current = std::fs::read_to_string(store.root().join(file))
            .with_context(|| format!("reading covered Journey file '{file}' during settlement"))
            .map(|content| crate::artifact::fingerprint(&content))?;
        if anchors.covered_hashes.get(file) != Some(&current) {
            bail!(
                "covered file '{}' changed between execution and settlement; refusing to settle",
                file
            );
        }
    }
    // The executable boundary must still match: every executed operation's
    // declared token, resolved executable, and content fingerprint.
    verify_executed_boundary(store.root(), proof, report, &anchors.executed_boundary)?;

    let (node_status, edge_status) = match report.status {
        crate::journey_runtime::RuntimeStatus::Passed => ("passed", InspectionStatus::Passing),
        crate::journey_runtime::RuntimeStatus::Failed => ("failed", InspectionStatus::Failing),
        crate::journey_runtime::RuntimeStatus::Blocked => ("blocked", InspectionStatus::Blocked),
    };
    let evidence = match &report.detail {
        Some(detail) => format!(
            "compiled Journey '{}:{}' observed {}: {detail}",
            report.journey_id, report.profile, node_status
        ),
        None => format!(
            "compiled Journey '{}:{}' observed {} with {} typed assertion(s)",
            report.journey_id, report.profile, node_status, report.assertions_passed
        ),
    };
    let run = if report.status == crate::journey_runtime::RuntimeStatus::Blocked
        || !observed.matches_compiled_proof()
    {
        None
    } else {
        let stdout = crate::journey_runtime::report_observation_json(report)?;
        let mut run = crate::runner::record_with_covered(
            RunProducer::Journey,
            &format!(
                "loom journey run {} --profile {}",
                report.journey_id, report.profile
            ),
            anchors.covered_hashes.clone(),
            report.assertions_passed,
            if report.status == crate::journey_runtime::RuntimeStatus::Passed {
                0
            } else {
                1
            },
            &stdout,
            report.detail.as_deref().unwrap_or("").as_bytes(),
        );
        run.observed_assertions = report
            .passed_assertions
            .iter()
            .map(|passed| crate::evidence::ObservedAssertion {
                group: passed.operation_id.clone(),
                assertion: passed.assertion_id.clone(),
            })
            .collect();
        run.assertion_trust = crate::evidence::AssertionTrust::LocallyMinted;
        run.locally_minted = true;
        Some(run)
    };

    // Compile owns the Proves/Validates/Calls/Exercises closure. Generic edge
    // verdicts cannot inspect those edges, so the observed run is the only
    // writer that can take them off `uninspected`.
    for kind in [
        EdgeKind::Validates,
        EdgeKind::Proves,
        EdgeKind::Calls,
        EdgeKind::Exercises,
    ] {
        for edge in store.edges_with(Some(kind), Some(validation_id), None)? {
            let mut assertion = Assertion::new(
                Subject::Edge(edge.id),
                Claim::Verdict,
                edge_status.as_str(),
                "loom",
            )
            .criterion("compiled Journey profile")
            .confidence(1.0)
            .cited(crate::evidence::cite(store.root(), &evidence)?);
            if let Some(run) = &run {
                assertion = assertion.observed(run.clone());
            }
            store.assert_fact(assertion)?;
        }
    }
    store.record_proof_stability(validation_id, node_status)?;
    store.set_node_status(validation_id, node_status)?;
    store.append_journal(
        "journey_run",
        validation_id,
        json!({
            "journey_id": report.journey_id,
            "profile": report.profile,
            "outcome": node_status,
            "journey_hash": report.journey_hash,
            "surface_hash": report.surface_hash,
            "assertions_passed": report.assertions_passed,
            "assertions_failed": report.assertions_failed,
            "detail": report.detail,
        }),
    )?;
    regrade_compiled_validation(store, validation_id)
}

/// Copy a passing Journey run onto compiler-owned Calls/Exercises that are
/// still `uninspected`. Compile can recreate those edges while the Validation
/// stays `passed`; generic `edge verdict` cannot inspect them.
pub fn resettle_uninspected_compiler_topology(
    store: &crate::store::Store,
    validation_id: &str,
) -> Result<()> {
    use crate::evidence::Evidence;
    use crate::model::{Claim, EdgeKind, InspectionStatus, NodeType};
    use crate::store::{Assertion, Subject};

    let Some(validation) = store.get_node(validation_id)? else {
        return Ok(());
    };
    if validation.node_type != NodeType::Validation || validation.status != "passed" {
        return Ok(());
    }

    let mut donor = None;
    for kind in [EdgeKind::Validates, EdgeKind::Proves] {
        for edge in store.edges_with(Some(kind), Some(validation_id), None)? {
            if edge.status != InspectionStatus::Passing {
                continue;
            }
            let Some(view) = store.fact(&Subject::Edge(edge.id.clone()), Claim::Verdict)? else {
                continue;
            };
            for row in &view.evidence {
                if let Evidence::Run(run) = &row.payload {
                    if run.locally_minted {
                        donor = Some(run.clone());
                        break;
                    }
                }
            }
            if donor.is_some() {
                break;
            }
        }
        if donor.is_some() {
            break;
        }
    }
    let Some(run) = donor else {
        return Ok(());
    };
    let evidence = format!(
        "compiled Journey '{}' already passed; copied locally-minted observation onto uninspected compiler topology",
        validation.name
    );

    for kind in [EdgeKind::Calls, EdgeKind::Exercises] {
        for edge in store.edges_with(Some(kind), Some(validation_id), None)? {
            if edge.status != InspectionStatus::Uninspected {
                continue;
            }
            let assertion = Assertion::new(
                Subject::Edge(edge.id),
                Claim::Verdict,
                InspectionStatus::Passing.as_str(),
                "loom",
            )
            .criterion("compiled Journey profile")
            .confidence(1.0)
            .cited(crate::evidence::cite(store.root(), &evidence)?)
            .observed(run.clone());
            store.assert_fact(assertion)?;
        }
    }
    Ok(())
}

fn regrade_compiled_validation(store: &crate::store::Store, validation_id: &str) -> Result<()> {
    use crate::model::EdgeKind;
    let Some(validation) = store.get_node(validation_id)? else {
        return Ok(());
    };
    let callgraph = crate::callgraph::build(store)?;
    let mut best: Option<crate::proofstrength::StrengthWitness> = None;
    for edge in store.edges_with(Some(EdgeKind::Validates), Some(validation_id), None)? {
        let witness =
            crate::proofstrength::grade(store, store.root(), &validation, &edge.to_id, &callgraph)?;
        let stronger = best
            .as_ref()
            .map(|current| {
                crate::proofstrength::Strength::parse(&witness.grade)
                    > crate::proofstrength::Strength::parse(&current.grade)
            })
            .unwrap_or(true);
        if stronger {
            best = Some(witness);
        }
    }
    if let Some(witness) = best {
        crate::proofstrength::store_witness(store, validation_id, &witness)?;
    }
    Ok(())
}

/// The outcome of a Store-owned interactive Journey run: either settled, or
/// paused at a human gate for a later one-shot resume.
#[derive(Debug)]
pub enum InteractiveJourneyRun {
    Completed(crate::journey_runtime::RuntimeReport),
    Pending(crate::journey_gate::PendingHuman),
}

/// Everything the Store-owned settlement derives before execution, from the
/// store alone: the canonical proof, the authored spec it compiled from, the
/// operation-exercise projection, and the store's own coordinates.
struct TrustedCompile {
    spec: JourneySpec,
    proof: crate::journey_runtime::CompiledJourneyProof,
    covered_files: Vec<String>,
    validation_body: Value,
}

fn trusted_compile(store: &crate::store::Store, validation_id: &str) -> Result<TrustedCompile> {
    use crate::model::{EdgeKind, NodeType};
    let validation = store
        .get_node(validation_id)?
        .ok_or_else(|| anyhow!("validation '{validation_id}' is missing"))?;
    if validation.node_type != NodeType::Validation
        || validation
            .body
            .get("type")
            .and_then(serde_json::Value::as_str)
            != Some("journey")
    {
        bail!("validation '{validation_id}' is not a compiler-owned Journey proof");
    }
    let profile = validation
        .body
        .get("profile")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("validation '{}' has no Journey profile", validation.name))?;
    if validation
        .body
        .get("compiler_version")
        .and_then(serde_json::Value::as_str)
        != Some(JOURNEY_COMPILER_VERSION)
    {
        bail!(
            "validation '{}' was compiled by a different compiler version",
            validation.name
        );
    }
    let proves = store.edges_with(Some(EdgeKind::Proves), Some(validation_id), None)?;
    let [proves] = proves.as_slice() else {
        bail!(
            "compiled Journey validation '{}' must prove exactly one Journey",
            validation.name
        );
    };
    let journey = store
        .get_node(&proves.to_id)?
        .ok_or_else(|| anyhow!("compiled Journey target is missing"))?;
    if journey.node_type != NodeType::Journey {
        bail!(
            "compiled Journey target of '{}' is not a Journey",
            validation.name
        );
    }
    let (spec, proof) = compile_accepted_source(store, &journey, profile)?;
    if proof.journey_id != journey.name
        || validation
            .body
            .get("journey_hash")
            .and_then(serde_json::Value::as_str)
            != Some(proof.journey_hash.as_str())
        || validation
            .body
            .get("surface_hash")
            .and_then(serde_json::Value::as_str)
            != Some(proof.surface_hash.as_str())
    {
        bail!(
            "validation '{}' does not match the canonical accepted-surface proof",
            validation.name
        );
    }
    let projection =
        crate::journey_exercises::expected_projection(store, &journey).with_context(|| {
            format!(
                "Journey '{}' has no current operation-exercise projection",
                journey.name
            )
        })?;
    let covered_files = projection.covered_files();
    Ok(TrustedCompile {
        spec,
        proof,
        covered_files,
        validation_body: validation.body.clone(),
    })
}

fn reacquire_graph_lock_after_failure(
    store: &crate::store::Store,
    error: anyhow::Error,
) -> anyhow::Error {
    match store.reacquire_graph_lock() {
        Ok(()) => error,
        Err(lock_error) => {
            anyhow!("{error:#}; additionally failed to reacquire the graph lock: {lock_error:#}")
        }
    }
}

fn execute_and_settle_interactive<F>(
    store: &crate::store::Store,
    validation_id: &str,
    compiled: &TrustedCompile,
    purpose: &'static str,
    drift_phase: &'static str,
    pending_error: Option<&'static str>,
    execute: F,
) -> Result<InteractiveJourneyRun>
where
    F: FnOnce(&Path) -> Result<crate::journey_runtime::ExecutionOutcome>,
{
    let root = store.root().to_path_buf();
    let identity = store.execution_identity();
    // Child loom processes must be able to open the graph during execution;
    // every preparation failure retakes the lock before it can escape.
    store.release_graph_lock();
    let _guard = match crate::harness::acquire(&root, purpose, &identity) {
        Ok(guard) => guard,
        Err(error) => return Err(reacquire_graph_lock_after_failure(store, error)),
    };
    if let Err(error) = store.append_journal(
        crate::audit::PROOF_EXECUTION_STARTED_EVENT,
        validation_id,
        json!({ "purpose": purpose, "pid": std::process::id() }),
    ) {
        return Err(reacquire_graph_lock_after_failure(store, error));
    }

    let outcome = match (execute(&root), store.reacquire_graph_lock()) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(lock_error)) => return Err(lock_error),
        (Err(error), Err(lock_error)) => {
            return Err(anyhow!(
                "{error:#}; additionally failed to reacquire the graph lock: {lock_error:#}"
            ));
        }
    };
    store.append_journal(
        crate::audit::PROOF_EXECUTION_ENDED_EVENT,
        validation_id,
        json!({ "purpose": purpose, "pid": std::process::id() }),
    )?;

    match outcome? {
        crate::journey_runtime::ExecutionOutcome::Pending(pending) => {
            if let Some(message) = pending_error {
                bail!("{message}");
            }
            Ok(InteractiveJourneyRun::Pending(pending))
        }
        crate::journey_runtime::ExecutionOutcome::Completed {
            report,
            mut observation,
            human_decisions,
        } => {
            if store
                .get_node(validation_id)?
                .map(|node| node.body)
                .as_ref()
                != Some(&compiled.validation_body)
            {
                bail!(
                    "validation '{}' changed during Journey {}; refusing settlement",
                    validation_id,
                    drift_phase
                );
            }
            observation.mark_trusted();
            settle_compiled_validation(store, validation_id, &observation)?;
            for decision in human_decisions {
                store.append_journal("journey_human_decision", validation_id, decision)?;
            }
            Ok(InteractiveJourneyRun::Completed(report))
        }
    }
}

/// Store-owned compile → execute → settle for a machine-only Journey.
///
/// The canonical proof, the execution root, the covered-file evidence, and the
/// executable boundary are all derived from `store`; no caller-selected root,
/// proof, projection, or coverage is an input. The harness guard stays alive
/// across compilation, execution, the post-execution recheck, and settlement.
/// The graph write lock is released only for the execution window (compiled
/// operations may spawn child `loom` processes) and re-taken before any write;
/// settlement re-derives every trust-relevant input and refuses on drift.
pub fn run_and_settle_compiled_validation(
    store: &crate::store::Store,
    validation_id: &str,
    overrides: &BTreeMap<String, Value>,
) -> Result<crate::journey_runtime::RuntimeReport> {
    match run_interactive_and_settle_compiled_validation(store, validation_id, overrides)? {
        InteractiveJourneyRun::Completed(report) => Ok(report),
        InteractiveJourneyRun::Pending(_) => bail!(
            "compiled Journey requires host-mediated execution; use the interactive runtime \
             (`loom journey run` / `loom journey resume`)"
        ),
    }
}

/// Store-owned compile → execute → settle that may pause at a human gate.
/// Returns [`InteractiveJourneyRun::Pending`] without settling when the
/// Journey reaches a host-mediated decision; the resume entrypoint continues
/// it under the same boundary.
pub fn run_interactive_and_settle_compiled_validation(
    store: &crate::store::Store,
    validation_id: &str,
    overrides: &BTreeMap<String, Value>,
) -> Result<InteractiveJourneyRun> {
    let compiled = trusted_compile(store, validation_id)?;
    execute_and_settle_interactive(
        store,
        validation_id,
        &compiled,
        "journey run",
        "execution",
        None,
        |root| {
            Ok(crate::journey_runtime::execute_interactive_with_anchors(
                root,
                &compiled.spec,
                &compiled.proof,
                overrides,
                Some(&compiled.covered_files),
            ))
        },
    )
}

/// Store-owned resume of a paused interactive Journey: re-derives the
/// canonical proof from the store (never from the caller), rechecks the
/// execution-time anchors persisted by the paused run, executes the remaining
/// steps, and settles under the same boundary as a fresh run.
pub fn resume_and_settle_compiled_validation(
    store: &crate::store::Store,
    token: &str,
    answer: crate::journey_gate::ResumeAnswer,
    executor: &str,
) -> Result<InteractiveJourneyRun> {
    let pending = crate::journey_runtime::pending_continuation(token)?;
    let binding = &pending.binding;
    let validation_id = resolve_journey_validation(store, &binding.journey_id, &binding.profile)?;
    let compiled = trusted_compile(store, &validation_id)?;
    execute_and_settle_interactive(
        store,
        &validation_id,
        &compiled,
        "journey resume",
        "resume",
        Some("resumed Journey unexpectedly paused at a second human gate"),
        |root| {
            crate::journey_runtime::resume_interactive(
                root,
                &compiled.spec,
                &compiled.proof,
                token,
                answer,
                executor,
            )
        },
    )
}

/// The compiler-owned Validation node for a Journey id/profile pair, by the
/// same naming convention the compiler writes.
fn resolve_journey_validation(
    store: &crate::store::Store,
    journey_id: &str,
    profile: &str,
) -> Result<String> {
    use crate::model::NodeType;
    let name = format!("journey:{journey_id}:{profile}");
    let mut candidates: Vec<String> = store
        .list_nodes(Some(NodeType::Validation), usize::MAX)?
        .into_iter()
        .filter(|node| node.name == name)
        .map(|node| node.id)
        .collect();
    candidates.sort();
    match candidates.len() {
        0 => bail!("no compiled Journey validation for '{journey_id}:{profile}'"),
        1 => Ok(candidates.remove(0)),
        _ => bail!("compiled Journey validation name '{name}' is ambiguous"),
    }
}

/// Re-derive the executable boundary from the store side and refuse if it
/// differs from what the runtime recorded at execution time: every executed
/// operation must have declared exactly the compiled argv0 token, and every
/// recorded executable must still resolve under the same Store-derived
/// trusted execution policy (relative literals inside the trusted root with
/// no symlink escape, bare names only through the approved toolchain
/// boundary) to the exact canonical path and content fingerprint the runtime
/// hashed before its spawn.
fn verify_executed_boundary(
    root: &Path,
    proof: &crate::journey_runtime::CompiledJourneyProof,
    report: &crate::journey_runtime::RuntimeReport,
    recorded: &[crate::journey_runtime::ExecutableBoundary],
) -> Result<()> {
    use crate::journey_runtime::ExecutableBoundary;
    let mut declared: Vec<(&str, &str, String)> = Vec::new();
    if let Some(setup) = &proof.setup {
        for executed in &report.setup {
            if let Some(operation) = setup
                .operations
                .iter()
                .find(|operation| operation.operation_id == executed.operation_id)
            {
                declared.push((
                    executed.operation_id.as_str(),
                    operation.argv[0].as_str(),
                    format!("setup operation '{}'", executed.operation_id),
                ));
            }
        }
    }
    for executed in &report.steps {
        if executed.operation_id == "human-decision" {
            continue;
        }
        if let Some(step) = proof
            .steps
            .iter()
            .find(|step| step.operation_id == executed.operation_id)
        {
            declared.push((
                executed.operation_id.as_str(),
                step.argv[0].as_str(),
                format!(
                    "step '{}' (operation '{}')",
                    executed.step_id, executed.operation_id
                ),
            ));
        }
    }
    declared.sort_unstable_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    let declared_pairs: Vec<(&str, &str)> = declared
        .iter()
        .map(|(operation_id, argv0, _)| (*operation_id, *argv0))
        .collect();
    let mut got: Vec<(&str, &str)> = recorded
        .iter()
        .map(|entry| (entry.operation_id.as_str(), entry.declared.as_str()))
        .collect();
    got.sort_unstable();
    if declared_pairs != got {
        // Walk both sorted multisets and name every operation that differs,
        // so the refusal shows which step mismatched and what each side saw.
        let mut differences = Vec::new();
        let mut declared_iter = declared.iter().peekable();
        let mut recorded_iter = got.iter().peekable();
        loop {
            match (declared_iter.peek(), recorded_iter.peek()) {
                (None, None) => break,
                (Some(entry), None) => {
                    differences.push(format!(
                        "{} declares argv0 '{}' but no executed boundary was recorded",
                        entry.2, entry.1
                    ));
                    declared_iter.next();
                }
                (None, Some(entry)) => {
                    differences.push(format!(
                        "operation '{}' recorded argv0 '{}' but appears in no compiled step",
                        entry.0, entry.1
                    ));
                    recorded_iter.next();
                }
                (Some(declared_entry), Some(recorded_entry)) => {
                    match declared_entry.0.cmp(recorded_entry.0) {
                        std::cmp::Ordering::Less => {
                            differences.push(format!(
                                "{} declares argv0 '{}' but no executed boundary was recorded",
                                declared_entry.2, declared_entry.1
                            ));
                            declared_iter.next();
                        }
                        std::cmp::Ordering::Greater => {
                            differences.push(format!(
                                "operation '{}' recorded argv0 '{}' but appears in no compiled step",
                                recorded_entry.0, recorded_entry.1
                            ));
                            recorded_iter.next();
                        }
                        std::cmp::Ordering::Equal => {
                            if declared_entry.1 != recorded_entry.1 {
                                differences.push(format!(
                                    "{} declares argv0 '{}' but the run recorded '{}'",
                                    declared_entry.2, declared_entry.1, recorded_entry.1
                                ));
                            }
                            declared_iter.next();
                            recorded_iter.next();
                        }
                    }
                }
            }
        }
        bail!(
            "executed operation boundary does not match the compiled proof ({}); refusing settlement",
            differences.join("; ")
        );
    }

    let mut by_operation: BTreeMap<&str, &ExecutableBoundary> = BTreeMap::new();
    for entry in recorded {
        by_operation.insert(entry.operation_id.as_str(), entry);
    }
    for (operation_id, declared_token, _label) in declared {
        let entry = by_operation
            .get(operation_id)
            .ok_or_else(|| anyhow!("executed operation '{operation_id}' has no boundary"))?;
        // Re-derive the approved executable exactly as the guarded runtime
        // did before its spawn, and require both the canonical path and the
        // pre-execution content fingerprint to match. This refuses symlink
        // escapes, bare names that only exist on a caller-mutated PATH, and
        // executables that were missing, replaced, or self-modified between
        // execution and settlement.
        let derived = crate::journey_runtime::resolve_trusted_executable(root, declared_token)
            .with_context(|| {
                format!("re-deriving the executable boundary for operation '{operation_id}'")
            })?;
        if Path::new(&entry.resolved) != derived.path || entry.hash != derived.hash {
            bail!(
                "operation '{operation_id}' executed '{}' (fingerprint {}), not the \
                 Store-approved '{}' (fingerprint {}); refusing settlement",
                entry.resolved,
                entry.hash,
                derived.path.display(),
                derived.hash
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn boundary_mismatch_refusal_names_the_step_and_both_argv0_tokens() {
        use crate::journey_runtime::{
            CompiledJourneyProof, CompiledProfileShape, CompiledStep, ExecutableBoundary,
            RuntimeReport, RuntimeStatus, StepReport,
        };

        let proof = CompiledJourneyProof {
            schema: "loom.journey.proof/v1".into(),
            compiler_version: "6".into(),
            journey_id: "checkout".into(),
            journey_hash: "journey-hash".into(),
            surface_hash: "surface-hash".into(),
            profile: "smoke".into(),
            profile_shape: CompiledProfileShape {
                input_ids: Vec::new(),
                setup_directories: Vec::new(),
                setup_files: Vec::new(),
                setup_env: Vec::new(),
            },
            setup: None,
            steps: vec![CompiledStep {
                step_id: "checkout".into(),
                operation_id: "gridctl-reject".into(),
                argv: vec!["gridctl".into()],
                environment: Vec::new(),
                read_only: false,
                timeout_seconds: Some(30),
                expected_exit: 11,
                arguments: Vec::new(),
                captures: Vec::new(),
                assertions: Vec::new(),
                redact: Vec::new(),
                human_decision: None,
            }],
        };
        let report = RuntimeReport {
            journey_id: "checkout".into(),
            profile: "smoke".into(),
            journey_hash: "journey-hash".into(),
            surface_hash: "surface-hash".into(),
            status: RuntimeStatus::Passed,
            assertions_passed: 1,
            assertions_failed: 0,
            detail: None,
            setup: Vec::new(),
            file_transitions: Vec::new(),
            steps: vec![StepReport {
                step_id: "checkout".into(),
                operation_id: "gridctl-reject".into(),
                argv: vec!["shim".into()],
                exit_code: 11,
                output: json!({"ok": false}),
                assertions_passed: 1,
                assertions_failed: 0,
            }],
            captures: BTreeMap::new(),
            passed_assertions: Vec::new(),
            failed_assertions: Vec::new(),
        };
        let recorded = vec![ExecutableBoundary {
            operation_id: "gridctl-reject".into(),
            declared: "shim".into(),
            argv0: "shim".into(),
            resolved: "/tmp/shim".into(),
            hash: "fingerprint".into(),
        }];

        let error = verify_executed_boundary(Path::new("/"), &proof, &report, &recorded)
            .expect_err("a recorded argv0 that differs from the compiled step must refuse");
        let message = format!("{error:#}");
        assert!(
            message.contains("step 'checkout' (operation 'gridctl-reject')"),
            "refusal must name the step: {message}"
        );
        assert!(
            message.contains("declares argv0 'gridctl' but the run recorded 'shim'"),
            "refusal must show both argv0 tokens: {message}"
        );
        assert!(message.contains("refusing settlement"), "{message}");
    }
}
