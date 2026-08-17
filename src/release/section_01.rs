/// The sole Journey-facing release façade. Source validation is deliberately
/// performed before candidate allocation; all candidate and trust-boundary
/// machinery remains private to this module.
/// Refuse a cold rehearsal in a repository whose shape the release plane does
/// not support, before anything expensive or obscure happens.
///
/// The release plane assumes loom's own layout in ways that are not policy a
/// target repository can adopt: surface manifests live at a fixed path; the
/// canonical reserved and secret lists cannot be relaxed by a manifest
/// (`validate_inventory_manifest` refuses that outright); and snapshot
/// verification walks the working tree without reading `.gitignore`, while the
/// candidate file plan asks Git for `--exclude-standard`. Any repository with
/// ignored build output outside the eight reserved components therefore has two
/// irreconcilable file sets and no manifest can satisfy both — loom's own repo
/// escapes only because its build output is confined to `target/`.
///
/// A conforming target repository met four validator errors in sequence, each
/// more obscure than the last, before reaching the one with no workaround. One
/// honest sentence up front is worth more than that trail.
fn refuse_unsupported_cold_rehearsal_layout(root: &Path) -> Result<()> {
    if root.join(SURFACE_MANIFEST_ROOT).is_dir() {
        return Ok(());
    }
    bail!(
        "cold rehearsal is not supported in this repository's layout: it expects surface \
         manifests at '{SURFACE_MANIFEST_ROOT}/', which does not exist here.\n\
         \n\
         `journey rehearse-cold` currently assumes loom's own repository shape, and the \
         assumptions are not ones a target repository can declare its way out of:\n\
         \x20 - surface manifests must live at '{SURFACE_MANIFEST_ROOT}/'\n\
         \x20 - no tracked file may enter a reserved component ({reserved}) or match a \
         secret pattern such as '.env.*'\n\
         \x20 - ignored build output must be confined to those reserved components, because \
         snapshot verification walks the working tree without reading .gitignore while the \
         candidate file plan asks Git to honour it\n\
         \n\
         The rest of loom — lint, compile, run, status, sync — has no such requirement. Use \
         `loom journey lint` and `loom journey diagnose` here instead.",
        reserved = INVENTORY_RESERVED_COMPONENTS.join(", "),
    )
}

pub(crate) fn rehearse_cold_journey(
    root: &Path,
    journey_id: &str,
) -> Result<ColdJourneyRehearsalReport> {
    let root = root.canonicalize()?;
    refuse_unsupported_cold_rehearsal_layout(&root)?;
    let inventory = load_source_inventory(&root)?.0;
    if journey_id == RELEASE_JOURNEY_ID {
        bail!("Journey 'release-workflow' cannot be cold-rehearsed");
    }
    let export = inspect_candidate_export(&root)?
        .ok_or_else(|| anyhow!("cold rehearsal requires the source-controlled v12 export"))?;
    let registered: Vec<_> = export
        .nodes
        .iter()
        .filter(|node| node.node_type == NodeType::Journey && node.name == journey_id)
        .collect();
    if registered.len() != 1 {
        bail!("cold rehearsal requires exactly one registered Journey '{journey_id}'");
    }
    let artifact = registered[0]
        .body
        .get("artifact")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("Journey '{journey_id}' has no artifact"))?;
    let artifact_path = confined_inventory_file(&root, &inventory, artifact)?;
    let artifact_source_hash = fingerprint_bytes(&fs::read(&artifact_path)?);
    let spec = crate::journey::parse(&artifact_path)?;
    if spec.id != journey_id || !spec.profiles.contains_key(RELEASE_PROFILE) {
        bail!("cold rehearsal requires the exact registered Journey '{journey_id}' proof profile");
    }
    let semantic_hash = spec.semantic_hash()?;
    if registered[0]
        .body
        .get("semantic_hash")
        .and_then(serde_json::Value::as_str)
        != Some(semantic_hash.as_str())
    {
        bail!("cold rehearsal Journey export is stale against its confined artifact");
    }
    let manifest_path = Path::new(SURFACE_MANIFEST_ROOT).join(format!("{journey_id}.surface.json"));
    let manifest_path_source =
        confined_inventory_file(&root, &inventory, &manifest_path.to_string_lossy())?;
    let manifest_source_hash = fingerprint_bytes(&fs::read(&manifest_path_source)?);
    let manifest = SurfaceManifest::parse_json(&manifest_path_source)?;
    if manifest
        .bindings
        .iter()
        .any(|binding| binding.human_decision().is_some())
    {
        bail!(
            "Journey '{journey_id}' declares a human_decision binding and cannot be cold-rehearsed"
        );
    }
    manifest.validate_for(&spec, &spec.semantic_hash()?)?;
    let plan = crate::candidate_surface_policy::inspect_manifest(
        &spec,
        &manifest,
        crate::candidate_surface_policy::PolicyMode::Runtime,
    )?;
    if plan.inspections().iter().any(|inspection| {
        inspection.capability == crate::candidate_surface_policy::DerivedCapability::DetachedProcess
            || inspection.nested.iter().any(|nested| {
                nested.capability
                    == crate::candidate_surface_policy::DerivedCapability::DetachedProcess
            })
    }) {
        bail!("cold Journey rehearsal cannot invoke release or nested cold-rehearsal operations");
    }

    let before = LiveState::capture(&root)?;
    let mut ledger = Vec::new();
    let (candidate, source_inventory) =
        DetachedCandidate::copy(&root, "journey-cold", &mut ledger)?;
    let (candidate_inventory, candidate_inventory_hash) = load_source_inventory(candidate.path())?;
    if candidate_inventory_hash != source_inventory.manifest_hash {
        bail!("cold rehearsal inventory changed during candidate materialization");
    }
    let candidate_artifact =
        confined_inventory_file(candidate.path(), &candidate_inventory, artifact)?;
    let candidate_manifest_path = confined_inventory_file(
        candidate.path(),
        &candidate_inventory,
        &manifest_path.to_string_lossy(),
    )?;
    if fingerprint_bytes(&fs::read(&candidate_artifact)?) != artifact_source_hash
        || fingerprint_bytes(&fs::read(&candidate_manifest_path)?) != manifest_source_hash
    {
        bail!("cold rehearsal Journey source changed during candidate materialization");
    }
    let spec = crate::journey::parse(&candidate_artifact)?;
    let manifest = SurfaceManifest::parse_json(&candidate_manifest_path)?;
    let candidate_hash = source_inventory.inventory_hash.clone();
    let dependency_cache = DependencyCacheGuard::open(&root, &inventory.cache_root_environment)?;
    let sandbox = ProcessSandbox::create(candidate.path(), &dependency_cache)?;
    let binary = std::env::current_exe().context("locating current Loom executable")?;
    let mut executor = SystemReleaseExecutor;
    run_loom(
        candidate.path(),
        &binary,
        &["init", ".", "--name", "loom-journey-cold", "--json"],
        &mut executor,
        &sandbox,
        &mut ledger,
    )?;
    run_loom(
        candidate.path(),
        &binary,
        &["import", "loom.graph.json", "--json"],
        &mut executor,
        &sandbox,
        &mut ledger,
    )?;
    // Re-author only the requested canonical source and surface after import.
    run_loom(
        candidate.path(),
        &binary,
        &["journey", "add", artifact, "--json"],
        &mut executor,
        &sandbox,
        &mut ledger,
    )?;
    run_loom(
        candidate.path(),
        &binary,
        &[
            "journey",
            "surface-accept",
            journey_id,
            "--manifest",
            &manifest_path.to_string_lossy(),
            "--json",
        ],
        &mut executor,
        &sandbox,
        &mut ledger,
    )?;
    let candidate_store = Store::open(candidate.path())?;
    let candidate_journey = candidate_store.resolve_node(journey_id, Some(NodeType::Journey))?;
    let surface_hash =
        crate::journey::surface_projection_hash(&candidate_store, &candidate_journey)?
            .ok_or_else(|| anyhow!("cold rehearsal candidate has no accepted surface"))?;
    drop(candidate_store);
    let proof = crate::journey_runtime::compile_surface(
        &spec,
        &surface_hash,
        RELEASE_PROFILE,
        manifest.surface.operations.clone(),
        manifest.setup.as_ref(),
        &manifest.bindings,
    )?;
    let runtime =
        crate::journey_runtime::execute(candidate.path(), &spec, &proof, &BTreeMap::new());
    let cache = dependency_cache.attest()?;
    if !cache.unchanged {
        bail!("cold Journey rehearsal changed the dependency cache");
    }
    if before != LiveState::capture(&root)? {
        bail!("cold Journey rehearsal changed caller state");
    }
    Ok(ColdJourneyRehearsalReport {
        schema: COLD_JOURNEY_REHEARSAL_SCHEMA.into(),
        status: runtime.status,
        journey_id: journey_id.into(),
        profile: RELEASE_PROFILE.into(),
        candidate_hash,
        source_inventory,
        dependency_cache: cache,
        runtime,
        settled: false,
        caller_changed: false,
    })
}

fn confined_inventory_file(
    root: &Path,
    inventory: &SourceInventoryManifest,
    relative: &str,
) -> Result<PathBuf> {
    let path = Path::new(relative);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || !inventory
            .files
            .iter()
            .any(|entry| entry.path == relative && entry.mode != "absent")
    {
        bail!("cold rehearsal path '{relative}' is not a declared normalized inventory file");
    }
    let joined = root.join(path);
    let metadata = fs::symlink_metadata(&joined)
        .with_context(|| format!("reading cold rehearsal path '{relative}'"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("cold rehearsal path '{relative}' is not a regular non-symlink file");
    }
    let canonical = joined.canonicalize()?;
    if !canonical.starts_with(root) {
        bail!("cold rehearsal path '{relative}' escapes the repository");
    }
    Ok(canonical)
}

/// Run one release-rehearsal phase and return a single structured attestation.
/// Expected policy/readiness failures are represented as `blocked`; filesystem
/// errors still return `Err` because no trustworthy attestation can be formed.
pub fn rehearse(root: &Path, phase: ReleasePhase) -> Result<ReleaseRehearsalReport> {
    let mut executor = SystemReleaseExecutor;
    rehearse_with_executor(root, phase, &mut executor)
}

fn discard_snapshot_stage(stage: &Path) {
    // Cleanup cannot replace the failure that explains why no snapshot was installed.
    if let Err(cleanup_error) = fs::remove_dir_all(stage) {
        eprintln!(
            "failed to remove release snapshot staging directory {}: {cleanup_error}",
            stage.display()
        );
    }
}

/// Materialize the exact source-controlled release inventory into an empty
/// destination without reading or creating Loom graph state.
pub fn snapshot(root: &Path, destination: &Path) -> Result<SourceSnapshotReport> {
    let root = root.canonicalize()?;
    let destination_metadata = fs::symlink_metadata(destination)?;
    if destination_metadata.file_type().is_symlink() || !destination_metadata.is_dir() {
        bail!("release snapshot destination must be a regular directory");
    }
    let destination = destination.canonicalize().with_context(|| {
        format!(
            "canonicalizing snapshot destination {}",
            destination.display()
        )
    })?;
    if destination.starts_with(&root) {
        bail!("release snapshot destination must be outside the source repository");
    }
    if fs::read_dir(&destination)?.next().is_some() {
        bail!("release snapshot destination must be empty");
    }
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("release snapshot destination has no parent"))?;
    let destination_identity = crate::artifact::fingerprint(&destination.to_string_lossy());
    let stage = parent.join(format!(
        ".loom-release-snapshot-stage-{}-{destination_identity}",
        std::process::id(),
    ));
    let backup = parent.join(format!(
        ".loom-release-snapshot-empty-{}-{destination_identity}",
        std::process::id(),
    ));
    if stage.exists() || backup.exists() {
        bail!("release snapshot staging path already exists");
    }
    fs::create_dir(&stage)?;
    let mut ledger = Vec::new();
    let source_inventory = match copy_candidate(&root, &stage, &mut ledger) {
        Ok(inventory) => inventory,
        Err(error) => {
            discard_snapshot_stage(&stage);
            return Err(error);
        }
    };
    if let Err(error) = fs::rename(&destination, &backup) {
        discard_snapshot_stage(&stage);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&stage, &destination) {
        // Rollback failures are secondary to the activation failure but must remain visible.
        if let Err(rollback_error) = fs::rename(&backup, &destination) {
            eprintln!(
                "failed to restore release snapshot destination {} from {}: {rollback_error}",
                destination.display(),
                backup.display()
            );
        }
        discard_snapshot_stage(&stage);
        return Err(error.into());
    }
    fs::remove_dir(&backup)?;
    Ok(SourceSnapshotReport {
        schema: "loom.release-snapshot/v1".into(),
        status: "passed".into(),
        candidate_hash: source_inventory.inventory_hash.clone(),
        source_inventory,
        workspace: passed_workspace(false),
    })
}

/// Return the canonical manifest identity used by snapshot and rehearsal
/// attestations without constructing or copying a candidate.
pub fn source_inventory_manifest_hash(root: &Path) -> Result<String> {
    let (_, manifest_hash) = load_source_inventory(root)?;
    Ok(manifest_hash)
}

#[doc(hidden)]
pub fn rehearse_with_executor(
    root: &Path,
    phase: ReleasePhase,
    executor: &mut dyn ReleaseExecutor,
) -> Result<ReleaseRehearsalReport> {
    let root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing release candidate {}", root.display()))?;
    let before = LiveState::capture(&root)?;
    let (outer, derivation_authority) = match required_outer_context(&root) {
        Ok(context) => context,
        Err(error) => {
            let mut report = ReleaseRehearsalReport::blocked(
                phase,
                missing_outer_attestation(),
                format!("{error:#}"),
            );
            report.effects = before.compare(&LiveState::capture(&root)?, &[]);
            return Ok(report);
        }
    };
    let (inventory, inventory_manifest_hash) = match load_source_inventory(&root) {
        Ok(loaded) => loaded,
        Err(error) => {
            let mut report = ReleaseRehearsalReport::blocked(phase, outer, format!("{error:#}"));
            report.effects = before.compare(&LiveState::capture(&root)?, &[]);
            return Ok(report);
        }
    };

    let dependency_cache =
        match DependencyCacheGuard::open(&root, &inventory.cache_root_environment) {
            Ok(cache) => cache,
            Err(error) => {
                let mut report =
                    ReleaseRehearsalReport::blocked(phase, outer, format!("{error:#}"));
                report.effects = before.compare(&LiveState::capture(&root)?, &[]);
                return Ok(report);
            }
        };
    let mut ledger = Vec::new();
    let mut source_inventory = None;
    let outcome = {
        let mut runtime = GateRuntime {
            outer: &outer,
            derivation_authority: &derivation_authority,
            executor,
            dependency_cache: &dependency_cache,
            code_gates: &inventory.code_gates,
            inventory_manifest_hash: &inventory_manifest_hash,
        };
        rehearse_inner(
            &root,
            phase,
            &mut runtime,
            &mut ledger,
            &mut source_inventory,
        )
    };
    let cache_attestation = dependency_cache.attest()?;
    let outcome = if cache_attestation.unchanged {
        outcome
    } else {
        Err(anyhow!(
            "release rehearsal changed the explicit dependency cache"
        ))
    };
    let after = LiveState::capture(&root)?;
    let effects = before.compare(&after, &ledger);
    let mut report = match outcome {
        Ok(report) => report,
        Err(error) => ReleaseRehearsalReport::blocked(phase, outer, format!("{error:#}")),
    };
    report.execution_ledger = ledger;
    if report.source_inventory.is_none() {
        report.source_inventory = source_inventory;
    }
    report.dependency_cache = Some(cache_attestation);
    report.effects = effects;
    if caller_state_changed(&report.effects) {
        report.status = ReleaseStatus::Blocked;
        report.detail = Some("release rehearsal changed caller state".into());
    }
    Ok(report)
}

fn caller_state_changed(effects: &EffectAttestation) -> bool {
    effects.live_source_changed
        || effects.live_graph_changed
        || effects.live_target_changed
        || effects.live_git_changed
        || effects.live_git_head_changed
        || effects.live_git_index_changed
        || effects.live_git_remotes_changed
        || effects.installed_binary_changed
}

fn run_fixpoint_gates(
    root: &Path,
    phase: ReleasePhase,
    runtime: &mut GateRuntime<'_>,
    ledger: &mut Vec<ArgvLedgerEntry>,
    source_inventory: &mut Option<SourceInventoryAttestation>,
    disagreement: &str,
) -> Result<(GateResult, WorkspaceAttestation, FixpointAttestation)> {
    let first_permit = claim_candidate_permit(runtime.derivation_authority, phase, 0)?;
    let second_permit = claim_candidate_permit(runtime.derivation_authority, phase, 1)?;
    let probes = probe_empty_workspace_policy(root, ledger)?;
    let first = run_isolated_gate(root, &first_permit, runtime, ledger, source_inventory)?;
    let second = run_isolated_gate(root, &second_permit, runtime, ledger, source_inventory)?;
    if first.candidate_hash != second.candidate_hash || first.result_hash != second.result_hash {
        bail!("{disagreement}");
    }
    Ok((
        second,
        probes,
        FixpointAttestation {
            performed: true,
            candidate_hash_equal: true,
            result_hash_equal: true,
        },
    ))
}

fn rehearse_inner(
    root: &Path,
    phase: ReleasePhase,
    runtime: &mut GateRuntime<'_>,
    ledger: &mut Vec<ArgvLedgerEntry>,
    source_inventory: &mut Option<SourceInventoryAttestation>,
) -> Result<ReleaseRehearsalReport> {
    let (gate, timeline, workspace, fixpoint) = match phase {
        ReleasePhase::IsolatedDogfood => {
            let permit = claim_candidate_permit(runtime.derivation_authority, phase, 0)?;
            let gate = run_isolated_gate(root, &permit, runtime, ledger, source_inventory)?;
            (
                gate,
                vec![event("isolated_dogfood", "passed")],
                passed_workspace(false),
                FixpointAttestation::default(),
            )
        }
        phase @ (ReleasePhase::FreshFixpoint | ReleasePhase::GatedPreparation) => {
            let (disagreement, timeline) = match phase {
                ReleasePhase::FreshFixpoint => (
                    "independent release rehearsals produced different semantic attestations",
                    vec![event("fresh_fixpoint", "passed")],
                ),
                ReleasePhase::GatedPreparation => (
                    "release preparation gates disagree on semantic readiness",
                    vec![
                        event("isolated_dogfood", "passed"),
                        event("fresh_fixpoint", "passed"),
                        event("mutation", "skipped_rehearsal"),
                    ],
                ),
                ReleasePhase::IsolatedDogfood => unreachable!(),
            };
            let (gate, workspace, fixpoint) =
                run_fixpoint_gates(root, phase, runtime, ledger, source_inventory, disagreement)?;
            (gate, timeline, workspace, fixpoint)
        }
    };

    Ok(ReleaseRehearsalReport {
        schema: RELEASE_REHEARSAL_SCHEMA.into(),
        phase,
        status: ReleaseStatus::Passed,
        candidate_hash: Some(gate.candidate_hash),
        result_hash: Some(gate.result_hash),
        workspace,
        source_inventory: Some(gate.source_inventory),
        graph: GraphAttestation {
            schema_version: crate::SCHEMA_VERSION,
            legacy_imported: false,
            legacy_migrated: false,
            imported_surfaces_quarantined: gate.imported_surfaces_quarantined,
            manifests_reauthorized: gate.manifests_reauthorized,
            authority_fail_closed: true,
            authority_fabricated: false,
            outer_profile: runtime.outer.clone(),
        },
        fixpoint,
        timeline,
        execution_ledger: Vec::new(),
        dependency_cache: None,
        effects: EffectAttestation::default(),
        policy: ReleasePolicyAttestation {
            push_requires_explicit_human_decision: true,
            bitwise_reproducibility_claimed: false,
        },
        detail: None,
    })
}
