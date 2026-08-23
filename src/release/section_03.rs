fn expected_candidate_permits(
    batch_hash: &str,
    outer_run_id: &str,
    outer_proof_hash: &str,
) -> Vec<DerivationCandidatePermit> {
    let mut permits = Vec::new();
    for (phase, count) in [
        (ReleasePhase::IsolatedDogfood, 1usize),
        (ReleasePhase::FreshFixpoint, 2usize),
        (ReleasePhase::GatedPreparation, 2usize),
    ] {
        for ordinal in 0..count {
            let bytes = format!(
                "{batch_hash}\0{outer_run_id}\0{outer_proof_hash}\0{}\0{ordinal}",
                phase.as_str()
            );
            permits.push(DerivationCandidatePermit {
                phase,
                ordinal,
                permit_hash: crate::journey_gate::sha256_digest(bytes.as_bytes()),
            });
        }
    }
    permits
}

fn required_outer_context(
    root: &Path,
) -> Result<(OuterJourneyAttestation, BoundDerivationAuthority)> {
    let journey_id = std::env::var(OUTER_JOURNEY_ID_ENV)
        .context("release rehearsal requires reserved outer Journey id context")?;
    let profile = std::env::var(OUTER_JOURNEY_PROFILE_ENV)
        .context("release rehearsal requires reserved outer Journey profile context")?;
    let run_id = std::env::var(OUTER_JOURNEY_RUN_ID_ENV)
        .context("release rehearsal requires reserved outer Journey run-id context")?;
    let journey_hash = std::env::var(OUTER_JOURNEY_HASH_ENV)
        .context("release rehearsal requires reserved Journey semantic hash context")?;
    let surface_hash = std::env::var(OUTER_SURFACE_HASH_ENV)
        .context("release rehearsal requires reserved surface hash context")?;
    let compiler_version = std::env::var(OUTER_COMPILER_VERSION_ENV)
        .context("release rehearsal requires reserved compiler version context")?;
    let proof_hash = std::env::var(OUTER_PROOF_HASH_ENV)
        .context("release rehearsal requires reserved compiled proof hash context")?;
    let capsule_path = PathBuf::from(
        std::env::var(OUTER_CONTEXT_CAPSULE_ENV)
            .context("release rehearsal requires a runtime-owned context capsule")?,
    );
    if journey_id != RELEASE_JOURNEY_ID || profile != RELEASE_PROFILE {
        bail!(
            "release rehearsal requires exact outer Journey '{RELEASE_JOURNEY_ID}' profile '{RELEASE_PROFILE}'"
        );
    }
    let expected_prefix = format!("{RELEASE_JOURNEY_ID}.{RELEASE_PROFILE}.");
    if !run_id.starts_with(&expected_prefix) || run_id.len() == expected_prefix.len() {
        bail!("release rehearsal received malformed reserved outer Journey run-id context");
    }
    let metadata = fs::symlink_metadata(&capsule_path)
        .with_context(|| format!("reading release context capsule {}", capsule_path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("release context capsule must be a regular non-symlink file");
    }
    let canonical_capsule = capsule_path.canonicalize()?;
    let canonical_root = root.canonicalize()?;
    let temp_root = std::env::temp_dir().canonicalize()?;
    if canonical_capsule.starts_with(&canonical_root) || !canonical_capsule.starts_with(&temp_root)
    {
        bail!("release context capsule is not in a detached runtime root");
    }
    let capsule: OuterJourneyContextCapsule =
        serde_json::from_slice(&fs::read(&canonical_capsule)?)
            .context("release context capsule is malformed")?;
    if capsule.schema != "loom.release-outer-context/v1"
        || capsule.journey_id != journey_id
        || capsule.profile != profile
        || capsule.run_id != run_id
        || capsule.journey_hash != journey_hash
        || capsule.surface_hash != surface_hash
        || capsule.compiler_version != compiler_version
        || capsule.proof_hash != proof_hash
    {
        bail!("release context capsule does not match reserved runtime context");
    }
    validate_bound_derivation_authority(root, &capsule)?;
    if compiler_version != crate::journey::JOURNEY_COMPILER_VERSION {
        bail!("release context capsule uses a stale Journey compiler version");
    }
    let store = crate::store::Store::open_read(root)?;
    let journey = store.resolve_node(RELEASE_JOURNEY_ID, Some(NodeType::Journey))?;
    if journey
        .body
        .get("semantic_hash")
        .and_then(serde_json::Value::as_str)
        != Some(journey_hash.as_str())
    {
        bail!("release context Journey semantic hash is stale for the current graph");
    }
    let current_surface = crate::journey::surface_projection_hash(&store, &journey)?
        .ok_or_else(|| anyhow!("release-workflow has no current accepted surface"))?;
    if current_surface != surface_hash {
        bail!("release context surface hash is stale for the current graph");
    }
    let outer = OuterJourneyAttestation {
        journey_id,
        profile,
        run_id,
        journey_hash,
        surface_hash,
        compiler_version,
        proof_hash,
        excluded_from_nested_execution: true,
        exclusion_reason:
            "the exact outer release-workflow/proof run executes this profile; only its nested duplicate is suppressed"
                .into(),
        context_binding_limit:
            "same-user filesystem/process isolation is not a cryptographic authority boundary"
                .into(),
    };
    Ok((outer, capsule.derivation_authority))
}

fn validate_bound_derivation_authority(
    root: &Path,
    capsule: &OuterJourneyContextCapsule,
) -> Result<()> {
    let authority = &capsule.derivation_authority;
    if authority.schema != BOUND_DERIVATION_AUTHORITY_SCHEMA
        || authority.authority != "human"
        || authority.outer_journey_id != capsule.journey_id
        || authority.outer_profile != capsule.profile
        || authority.outer_run_id != capsule.run_id
        || authority.outer_journey_hash != capsule.journey_hash
        || authority.outer_surface_hash != capsule.surface_hash
        || authority.outer_compiler_version != capsule.compiler_version
        || authority.outer_proof_hash != capsule.proof_hash
        || authority.derivations.is_empty()
        || authority.gate_token_digest.len() != 64
        || !authority
            .gate_token_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || authority.gate_binding.journey_id != capsule.journey_id
        || authority.gate_binding.profile != capsule.profile
        || authority.gate_binding.journey_hash != capsule.journey_hash
        || authority.gate_binding.surface_hash != capsule.surface_hash
        || authority.gate_binding.step_id != "authorize-derivations"
        || authority.gate_binding.subject.kind != "release_derivation_batch"
        || authority.gate_binding.subject.hash != derivation_subject_hash(&authority.derivations)?
        || derivation_batch_hash(&authority.derivations, &authority.human_decision)?
            != authority.batch_hash
        || authority.candidate_permits
            != expected_candidate_permits(
                &authority.batch_hash,
                &authority.outer_run_id,
                &authority.outer_proof_hash,
            )
    {
        bail!("release context derivation authority is missing, stale, or malformed");
    }
    let specs: BTreeMap<String, crate::journey::JourneySpec> = journey_artifacts(root)?
        .into_iter()
        .map(|path| {
            let spec = crate::journey::parse(&path)?;
            Ok((spec.id.clone(), spec))
        })
        .collect::<Result<_>>()?;
    let mut seen = BTreeSet::new();
    for subject in &authority.derivations {
        if !seen.insert(subject.journey_id.as_str())
            || subject.manifest.journey_id != subject.journey_id
            || subject.manifest.journey_hash != subject.journey_hash
            || subject.manifest.proposal_id != subject.proposal_id
        {
            bail!("release derivation authority repeats or mismatches a subject");
        }
        let spec = specs.get(&subject.journey_id).ok_or_else(|| {
            anyhow!(
                "release derivation authority targets missing Journey '{}'",
                subject.journey_id
            )
        })?;
        subject
            .manifest
            .validate_for(spec, &spec.semantic_hash()?)?;
        let canonical = canonical_json(serde_json::to_value(&subject.manifest)?);
        let observed = crate::artifact::fingerprint(&serde_json::to_string(&canonical)?);
        if observed != subject.manifest_hash {
            bail!(
                "release derivation authority manifest for '{}' changed after approval",
                subject.journey_id
            );
        }
    }
    Ok(())
}

fn claim_candidate_permit(
    authority: &BoundDerivationAuthority,
    phase: ReleasePhase,
    ordinal: usize,
) -> Result<DerivationCandidatePermit> {
    let permit = authority
        .candidate_permits
        .iter()
        .find(|permit| permit.phase == phase && permit.ordinal == ordinal)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "release derivation authority has no permit for {} candidate {ordinal}",
                phase.as_str()
            )
        })?;
    let capsule = PathBuf::from(
        std::env::var(OUTER_CONTEXT_CAPSULE_ENV)
            .context("release candidate permit requires the outer context capsule")?,
    )
    .canonicalize()?;
    let parent = capsule
        .parent()
        .ok_or_else(|| anyhow!("release context capsule has no runtime parent"))?;
    let directory = parent.join("release-derivation-candidate-permits");
    fs::create_dir_all(&directory)?;
    let directory = directory.canonicalize()?;
    if !directory.starts_with(parent) {
        bail!("release derivation candidate permit store escaped the runtime capsule root");
    }
    let path = directory.join(&permit.permit_hash);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut file = options.open(&path).map_err(|error| {
        if path.exists() {
            anyhow!(
                "release derivation candidate permit for {} ordinal {ordinal} has already been consumed",
                phase.as_str()
            )
        } else {
            anyhow::Error::from(error)
        }
    })?;
    use std::io::Write;
    file.write_all(permit.permit_hash.as_bytes())?;
    file.sync_all()?;
    Ok(permit)
}

fn passed_workspace(probed: bool) -> WorkspaceAttestation {
    WorkspaceAttestation {
        detached: true,
        source_excludes: SOURCE_EXCLUDES.iter().map(ToString::to_string).collect(),
        initially_empty: true,
        nonempty_probe: if probed { "rejected" } else { "not_run" }.into(),
        preinitialized_probe: if probed { "rejected" } else { "not_run" }.into(),
    }
}

fn probe_empty_workspace_policy(
    root: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<WorkspaceAttestation> {
    let nonempty = DetachedCandidate::allocate(root, "nonempty-probe")?;
    fs::write(nonempty.path().join("sentinel"), b"occupied")?;
    let nonempty_result = copy_candidate(root, nonempty.path(), ledger);
    let nonempty_rejected = nonempty_result.as_ref().is_err_and(|error| {
        validate_workspace_probe_failure("nonempty", &error.to_string()).is_ok()
    });
    ledger.push(ArgvLedgerEntry {
        source: "empty_workspace_probe:nonempty".into(),
        executable: "internal-copy-plan".into(),
        argv: vec![nonempty.path().to_string_lossy().into_owned()],
        policy: "destination_must_be_empty".into(),
        attempted: true,
        outcome: if nonempty_rejected {
            "rejected".into()
        } else {
            "unexpectedly_accepted".into()
        },
    });
    if !nonempty_rejected {
        bail!("nonempty workspace rejection probe did not observe the exact policy failure");
    }
    let initialized = DetachedCandidate::allocate(root, "initialized-probe")?;
    fs::create_dir_all(initialized.path().join(crate::LOOM_DIR))?;
    let initialized_result = copy_candidate(root, initialized.path(), ledger);
    let initialized_rejected = initialized_result.as_ref().is_err_and(|error| {
        validate_workspace_probe_failure("preinitialized", &error.to_string()).is_ok()
    });
    ledger.push(ArgvLedgerEntry {
        source: "empty_workspace_probe:preinitialized".into(),
        executable: "internal-copy-plan".into(),
        argv: vec![initialized.path().to_string_lossy().into_owned()],
        policy: "destination_must_not_be_initialized".into(),
        attempted: true,
        outcome: if initialized_rejected {
            "rejected".into()
        } else {
            "unexpectedly_accepted".into()
        },
    });
    if !initialized_rejected {
        bail!("preinitialized workspace rejection probe did not observe the exact policy failure");
    }
    Ok(passed_workspace(true))
}

#[doc(hidden)]
pub fn validate_workspace_probe_failure(probe: &str, detail: &str) -> Result<()> {
    let required = match probe {
        "nonempty" => "must be empty",
        "preinitialized" => "preinitialized .loom",
        _ => bail!("unknown release workspace probe '{probe}'"),
    };
    if !detail.contains(required) {
        bail!("workspace probe '{probe}' observed an unrelated failure instead of '{required}'");
    }
    Ok(())
}

fn run_isolated_gate(
    root: &Path,
    candidate_permit: &DerivationCandidatePermit,
    runtime: &mut GateRuntime<'_>,
    ledger: &mut Vec<ArgvLedgerEntry>,
    observed_inventory: &mut Option<SourceInventoryAttestation>,
) -> Result<GateResult> {
    let expected_journey_bindings = current_journey_bindings(root)?;
    let (candidate, source_inventory) = DetachedCandidate::copy(root, "candidate", ledger)?;
    if source_inventory.manifest_hash != runtime.inventory_manifest_hash {
        bail!("release inventory changed between policy validation and candidate materialization");
    }
    *observed_inventory = Some(source_inventory.clone());
    let candidate_hash = source_inventory.inventory_hash.clone();
    let sandbox = ProcessSandbox::create(candidate.path(), runtime.dependency_cache)?;
    let export = inspect_candidate_export(candidate.path())?;
    let imported_surfaces = export
        .as_ref()
        .map(imported_executable_surfaces)
        .transpose()?
        .unwrap_or_default();
    let manifests = candidate_manifest_attestations(
        candidate.path(),
        &imported_surfaces,
        runtime.outer,
        ledger,
    )?;

    run_code_gates(
        candidate.path(),
        runtime.code_gates,
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    let binary = candidate.path().join("target/debug/loom");
    run_loom(
        candidate.path(),
        &binary,
        &["init", ".", "--name", "loom-release-rehearsal", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    if export.is_some() {
        run_loom(
            candidate.path(),
            &binary,
            &["import", crate::GRAPH_EXPORT, "--json"],
            runtime.executor,
            &sandbox,
            ledger,
        )?;
    }
    for manifest in &manifests {
        run_loom(
            candidate.path(),
            &binary,
            &[
                "journey",
                "surface-accept",
                &manifest.journey_id,
                "--manifest",
                &manifest.path,
                "--json",
            ],
            runtime.executor,
            &sandbox,
            ledger,
        )?;
    }
    seed_candidate_graph(
        candidate.path(),
        &binary,
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    replay_derivation_authority(
        candidate.path(),
        runtime.derivation_authority,
        candidate_permit,
        &mut CandidateExec {
            binary: &binary,
            executor: runtime.executor,
            sandbox: &sandbox,
            ledger,
        },
    )?;
    let journey_summaries = run_candidate_journeys(
        candidate.path(),
        runtime.outer,
        &expected_journey_bindings,
        &mut CandidateExec {
            binary: &binary,
            executor: runtime.executor,
            sandbox: &sandbox,
            ledger,
        },
    )?;
    run_loom(
        candidate.path(),
        &binary,
        &["doctor", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    let coverage = run_loom(
        candidate.path(),
        &binary,
        &["coverage", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    require_clean_coverage(&coverage.stdout)?;
    run_loom(
        candidate.path(),
        &binary,
        &["export", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    run_loom(
        candidate.path(),
        &binary,
        &["export", "--check", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    let drift = run_loom(
        candidate.path(),
        &binary,
        &["journey", "drift", "--json"],
        runtime.executor,
        &sandbox,
        ledger,
    )?;
    require_clean_drift_excusing_gates(&drift.stdout, &human_gated_journeys(candidate.path())?)?;

    let result_hash = semantic_result_hash(
        candidate.path(),
        &manifests,
        runtime.outer,
        &journey_summaries,
    )?;
    Ok(GateResult {
        candidate_hash,
        result_hash,
        imported_surfaces_quarantined: imported_surfaces.len(),
        manifests_reauthorized: manifests,
        source_inventory,
    })
}

fn inspect_candidate_export(root: &Path) -> Result<Option<crate::travel::Export>> {
    let path = root.join(crate::GRAPH_EXPORT);
    if !path.exists() {
        return Ok(None);
    }
    crate::travel::read_export(&path).map(Some).with_context(|| {
        "release rehearsal refuses legacy or malformed exports; only an exact v12 export may enter the detached verifier"
    })
}
