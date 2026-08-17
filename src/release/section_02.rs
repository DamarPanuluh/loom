fn event(id: &str, outcome: &str) -> ReleaseEvent {
    ReleaseEvent {
        id: id.into(),
        outcome: outcome.into(),
    }
}

fn missing_outer_attestation() -> OuterJourneyAttestation {
    OuterJourneyAttestation {
        journey_id: String::new(),
        profile: String::new(),
        run_id: String::new(),
        journey_hash: String::new(),
        surface_hash: String::new(),
        compiler_version: String::new(),
        proof_hash: String::new(),
        excluded_from_nested_execution: false,
        exclusion_reason: "reserved outer Journey context was unavailable".into(),
        context_binding_limit:
            "same-user filesystem/process isolation is not a cryptographic authority boundary"
                .into(),
    }
}

/// Seal the exact current derivation projection set after the host has
/// obtained a substantive human answer. This is read-only with respect to the
/// Loom graph: the only write is an opaque one-shot capsule under the runtime
/// temporary authority store.
pub fn authorize_derivations(
    root: &Path,
    manifest_dir: &Path,
    human_decision: String,
    executor: &str,
) -> Result<DerivationAuthorizationGrant> {
    let root = root.canonicalize()?;
    let manifest_dir = manifest_dir.canonicalize().with_context(|| {
        format!(
            "canonicalizing derivation manifest directory {}",
            manifest_dir.display()
        )
    })?;
    let metadata = fs::symlink_metadata(&manifest_dir)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("derivation manifest directory must be a regular directory");
    }
    if executor.trim().is_empty() || crate::model::is_placeholder(executor) {
        bail!("derivation authority executor must be substantive");
    }
    let human_decision = crate::ratification::HumanDecision::mediated(human_decision)?;
    let store = crate::store::Store::open_read(&root)?;
    let registered: BTreeMap<String, Node> = store
        .list_nodes(Some(NodeType::Journey), usize::MAX)?
        .into_iter()
        .map(|node| (node.name.clone(), node))
        .collect();
    if registered.is_empty() {
        bail!("derivation authorization requires registered Journeys");
    }
    let specs: BTreeMap<String, crate::journey::JourneySpec> = journey_artifacts(&root)?
        .into_iter()
        .map(|path| {
            let spec = crate::journey::parse(&path)?;
            Ok((spec.id.clone(), spec))
        })
        .collect::<Result<_>>()?;
    let proposals = store.list_nodes(Some(NodeType::Proposal), usize::MAX)?;
    let mut paths = regular_files_with_suffix(&manifest_dir, ".json")?;
    paths.sort();
    let mut derivations = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)
            .with_context(|| format!("parsing reviewed manifest {}", path.display()))?;
        if value.get("schema").and_then(serde_json::Value::as_str)
            != Some(crate::journey::DERIVATION_SCHEMA)
        {
            continue;
        }
        let manifest: crate::journey::DerivationManifest = serde_json::from_value(value)
            .with_context(|| format!("parsing {} as a derivation manifest", path.display()))?;
        if !seen.insert(manifest.journey_id.clone()) {
            bail!(
                "reviewed derivation batch repeats Journey '{}'",
                manifest.journey_id
            );
        }
        let journey = registered.get(&manifest.journey_id).ok_or_else(|| {
            anyhow!(
                "reviewed derivation targets unregistered Journey '{}'",
                manifest.journey_id
            )
        })?;
        let spec = specs.get(&manifest.journey_id).ok_or_else(|| {
            anyhow!(
                "registered Journey '{}' has no authored source",
                manifest.journey_id
            )
        })?;
        let journey_hash = spec.semantic_hash()?;
        manifest.validate_for(spec, &journey_hash)?;
        if journey
            .body
            .get("semantic_hash")
            .and_then(serde_json::Value::as_str)
            != Some(journey_hash.as_str())
        {
            bail!(
                "registered Journey '{}' is stale for its authored source",
                manifest.journey_id
            );
        }
        let canonical = canonical_json(serde_json::to_value(&manifest)?);
        let canonical_text = serde_json::to_string(&canonical)?;
        let manifest_hash = crate::artifact::fingerprint(&canonical_text);
        let matching: Vec<&Node> = proposals
            .iter()
            .filter(|proposal| {
                proposal.status == "adopted"
                    && proposal
                        .body
                        .get("source")
                        .and_then(serde_json::Value::as_str)
                        == Some("journey_derivation")
                    && proposal
                        .body
                        .get("journey_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(manifest.journey_id.as_str())
                    && proposal
                        .body
                        .get("journey_hash")
                        .and_then(serde_json::Value::as_str)
                        == Some(journey_hash.as_str())
                    && proposal
                        .body
                        .get("proposal_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(manifest.proposal_id.as_str())
                    && proposal
                        .body
                        .get("manifest_hash")
                        .and_then(serde_json::Value::as_str)
                        == Some(manifest_hash.as_str())
                    && proposal.body.get("raw").and_then(serde_json::Value::as_str)
                        == Some(canonical_text.as_str())
            })
            .collect();
        if matching.len() != 1 {
            bail!(
                "reviewed derivation for Journey '{}' has {} exact current adopted Proposals (expected one)",
                manifest.journey_id,
                matching.len()
            );
        }
        derivations.push(AuthorizedDerivation {
            journey_id: manifest.journey_id.clone(),
            journey_hash,
            proposal_id: manifest.proposal_id.clone(),
            manifest_hash,
            manifest,
        });
    }
    derivations.sort_by(|left, right| left.journey_id.cmp(&right.journey_id));
    let registered_ids: BTreeSet<&str> = registered.keys().map(String::as_str).collect();
    let authorized_ids: BTreeSet<&str> = derivations
        .iter()
        .map(|subject| subject.journey_id.as_str())
        .collect();
    if registered_ids != authorized_ids {
        let missing = registered_ids
            .difference(&authorized_ids)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        let unexpected = authorized_ids
            .difference(&registered_ids)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "reviewed derivation batch must cover every registered Journey exactly once (missing: [{missing}], unexpected: [{unexpected}])"
        );
    }
    let subject_hash = derivation_subject_hash(&derivations)?;
    let release_journey = registered.get(RELEASE_JOURNEY_ID).ok_or_else(|| {
        anyhow!("derivation authorization requires registered '{RELEASE_JOURNEY_ID}'")
    })?;
    let release_surface_hash = crate::journey::surface_projection_hash(&store, release_journey)?
        .ok_or_else(|| anyhow!("release-workflow has no current accepted surface"))?;
    let prompt = crate::journey_gate::HumanPrompt::new(
        format!(
            "Approve the exact {}-manifest derivation batch bound by SHA-256 {subject_hash}?",
            derivations.len()
        ),
        "Approve only if the listed Journey, semantic, proposal, and canonical manifest hashes match the reviewed batch.",
        vec![
            crate::journey_gate::HumanOption::new(
                "approve",
                "Approve exact batch",
                "Authorize only these exact current manifests for one outer release run.",
                false,
            ),
            crate::journey_gate::HumanOption::new(
                "revise",
                "Request revisions",
                "Do not authorize; revise one or more derivations.",
                true,
            ),
            crate::journey_gate::HumanOption::new(
                "defer",
                "Defer",
                "Do not authorize or mutate release state.",
                false,
            ),
        ],
    )?;
    let binding = crate::journey_gate::GateBinding {
        journey_id: RELEASE_JOURNEY_ID.into(),
        profile: RELEASE_PROFILE.into(),
        journey_hash: release_journey
            .body
            .get("semantic_hash")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("release-workflow lacks a semantic hash"))?
            .into(),
        surface_hash: release_surface_hash,
        step_id: "authorize-derivations".into(),
        step_index: 0,
        subject: crate::journey_gate::GateSubject {
            kind: "release_derivation_batch".into(),
            id: format!("batch-{subject_hash}"),
            hash: subject_hash,
        },
        prompt_hash: prompt.digest()?,
    };
    let authority_root = authority_store_root(&root)?;
    let gate_store = crate::journey_gate::CapsuleStore::new(authority_root.join("human-gate"))?;
    let issued = gate_store.issue(binding.clone(), prompt)?;
    let claimed = gate_store.claim(
        &issued.pending.resume_token,
        &binding,
        crate::journey_gate::ResumeAnswer {
            choice_id: "approve".into(),
            human_decision: match &human_decision {
                crate::ratification::HumanDecision::Mediated { response } => response.clone(),
                crate::ratification::HumanDecision::Direct { .. } => {
                    bail!("release derivation authorization must be host-mediated")
                }
            },
            free_form: None,
        },
        executor,
    )?;
    let receipt = claimed.receipt;
    if receipt.authority != "human"
        || receipt.executor != executor
        || receipt.choice_id != "approve"
        || receipt.binding != binding
        || receipt.human_decision != human_decision
    {
        bail!("host-mediated derivation authority receipt is inconsistent");
    }
    let batch_hash = derivation_batch_hash(&derivations, &human_decision)?;
    let pending = PendingDerivationAuthority {
        schema: DERIVATION_AUTHORITY_SCHEMA.into(),
        batch_hash: batch_hash.clone(),
        authority: "human".into(),
        executor: executor.to_string(),
        human_decision,
        gate_token_digest: receipt.token_digest,
        gate_binding: receipt.binding,
        derivations: derivations.clone(),
    };
    let token = issue_derivation_authority(&root, &pending)?;
    let next_command = derivation_authority_next_command(&token, &root)?;
    Ok(DerivationAuthorizationGrant {
        schema: DERIVATION_AUTHORITY_SCHEMA.into(),
        status: "authorized_pending_outer_runtime".into(),
        token,
        next_command,
        batch_hash,
        authority: "human".into(),
        executor: executor.to_string(),
        derivations: derivations
            .into_iter()
            .map(|subject| DerivationAuthoritySubject {
                journey_id: subject.journey_id,
                journey_hash: subject.journey_hash,
                proposal_id: subject.proposal_id,
                manifest_hash: subject.manifest_hash,
            })
            .collect(),
        continuation_environment: DERIVATION_AUTHORITY_TOKEN_ENV.into(),
    })
}

fn derivation_authority_next_command(token: &str, root: &Path) -> Result<String> {
    let root = root
        .to_str()
        .ok_or_else(|| anyhow!("release graph path is not valid UTF-8"))?;
    Ok(format!(
        "{DERIVATION_AUTHORITY_TOKEN_ENV}={token} loom journey run {RELEASE_JOURNEY_ID} --profile {RELEASE_PROFILE} --graph {}",
        crate::workitem::q(root)
    ))
}

fn derivation_batch_hash(
    derivations: &[AuthorizedDerivation],
    decision: &crate::ratification::HumanDecision,
) -> Result<String> {
    let value = canonical_json(serde_json::json!({
        "authority": "human",
        "human_decision": decision,
        "derivations": derivations,
    }));
    Ok(crate::journey_gate::sha256_digest(&serde_json::to_vec(
        &value,
    )?))
}

fn derivation_subject_hash(derivations: &[AuthorizedDerivation]) -> Result<String> {
    let value = canonical_json(serde_json::json!({"derivations": derivations}));
    Ok(crate::journey_gate::sha256_digest(&serde_json::to_vec(
        &value,
    )?))
}

fn authority_store_root(live_root: &Path) -> Result<PathBuf> {
    let configured = std::env::var_os(DERIVATION_AUTHORITY_STORE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("loom-release-derivation-authorities-v1"));
    fs::create_dir_all(configured.join("pending"))?;
    fs::create_dir_all(configured.join("claimed"))?;
    fs::create_dir_all(configured.join("batches"))?;
    let root = configured.canonicalize()?;
    let temp = std::env::temp_dir().canonicalize()?;
    if !root.starts_with(&temp) || root.starts_with(live_root.canonicalize()?) {
        bail!("release derivation authority store must be under the runtime temp root and outside the live graph");
    }
    Ok(root)
}

fn issue_derivation_authority(root: &Path, pending: &PendingDerivationAuthority) -> Result<String> {
    let store = authority_store_root(root)?;
    let marker = store.join("batches").join(&pending.batch_hash);
    let mut marker_options = fs::OpenOptions::new();
    marker_options.write(true).create_new(true);
    if marker_options.open(&marker).is_err() {
        bail!("this exact derivation authority batch has already been issued");
    }
    let result = (|| {
        let random: String = rusqlite::Connection::open_in_memory()?.query_row(
            "SELECT lower(hex(randomblob(32)))",
            [],
            |row| row.get(0),
        )?;
        let token = format!("{DERIVATION_AUTHORITY_TOKEN_PREFIX}{random}");
        validate_derivation_authority_token(&token)?;
        let path = store.join("pending").join(&token);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        serde_json::to_writer(&mut file, pending)?;
        use std::io::Write;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(token)
    })();
    if result.is_err() {
        let _ = fs::remove_file(marker);
    }
    result
}

fn validate_derivation_authority_token(token: &str) -> Result<()> {
    let random = token
        .strip_prefix(DERIVATION_AUTHORITY_TOKEN_PREFIX)
        .ok_or_else(|| anyhow!("invalid release derivation authority token"))?;
    if random.len() != 64
        || !random
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("invalid release derivation authority token");
    }
    Ok(())
}

pub fn write_outer_context_capsule(
    live_root: &Path,
    directory: &Path,
    spec: &crate::journey::JourneySpec,
    proof: &crate::journey_runtime::CompiledJourneyProof,
    run_id: &str,
) -> Result<(PathBuf, OuterJourneyContextCapsule)> {
    let proof_bytes = crate::journey_runtime::canonical_bytes(proof)?;
    let proof_text = std::str::from_utf8(&proof_bytes).context("compiled proof is not UTF-8")?;
    let journey_hash = spec.semantic_hash()?;
    let proof_hash = crate::artifact::fingerprint(proof_text);
    let derivation_authority = claim_derivation_authority(
        live_root,
        OuterClaim {
            journey_id: &spec.id,
            profile: &proof.profile,
            run_id,
            journey_hash: &journey_hash,
            surface_hash: &proof.surface_hash,
            compiler_version: &proof.compiler_version,
            proof_hash: &proof_hash,
        },
    )?;
    let capsule = OuterJourneyContextCapsule {
        schema: "loom.release-outer-context/v1".into(),
        journey_id: spec.id.clone(),
        profile: proof.profile.clone(),
        run_id: run_id.into(),
        journey_hash,
        surface_hash: proof.surface_hash.clone(),
        compiler_version: proof.compiler_version.clone(),
        proof_hash,
        derivation_authority,
    };
    let path = directory.join("release-outer-context.json");
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("creating release context capsule {}", path.display()))?;
    serde_json::to_writer(&mut file, &capsule)?;
    use std::io::Write;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok((path, capsule))
}

struct OuterClaim<'a> {
    journey_id: &'a str,
    profile: &'a str,
    run_id: &'a str,
    journey_hash: &'a str,
    surface_hash: &'a str,
    compiler_version: &'a str,
    proof_hash: &'a str,
}

fn claim_derivation_authority(
    live_root: &Path,
    outer: OuterClaim<'_>,
) -> Result<BoundDerivationAuthority> {
    let token = std::env::var(DERIVATION_AUTHORITY_TOKEN_ENV)
        .context("release-workflow requires a one-shot derivation authority token")?;
    validate_derivation_authority_token(&token)?;
    let store = authority_store_root(live_root)?;
    let pending_path = store.join("pending").join(&token);
    let claimed_path = store.join("claimed").join(&token);
    let metadata = match fs::symlink_metadata(&pending_path) {
        Ok(metadata) => metadata,
        Err(_error) if claimed_path.exists() => {
            bail!("release derivation authority token has already been consumed")
        }
        Err(error) => return Err(error).context("unknown release derivation authority token"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("release derivation authority capsule must be a regular non-symlink file");
    }
    let pending: PendingDerivationAuthority = serde_json::from_slice(&fs::read(&pending_path)?)
        .context("release derivation authority capsule is malformed")?;
    if pending.schema != DERIVATION_AUTHORITY_SCHEMA
        || pending.authority != "human"
        || pending.derivations.is_empty()
        || derivation_batch_hash(&pending.derivations, &pending.human_decision)?
            != pending.batch_hash
        || pending.gate_binding.subject.hash != derivation_subject_hash(&pending.derivations)?
        || pending.gate_binding.journey_id != RELEASE_JOURNEY_ID
        || pending.gate_binding.profile != RELEASE_PROFILE
    {
        bail!("release derivation authority capsule failed its exact binding");
    }
    fs::rename(&pending_path, &claimed_path)
        .map_err(anyhow::Error::from)
        .context("atomically claiming release derivation authority")?;
    let candidate_permits =
        expected_candidate_permits(&pending.batch_hash, outer.run_id, outer.proof_hash);
    Ok(BoundDerivationAuthority {
        schema: BOUND_DERIVATION_AUTHORITY_SCHEMA.into(),
        batch_hash: pending.batch_hash,
        authority: pending.authority,
        executor: pending.executor,
        human_decision: pending.human_decision,
        gate_token_digest: pending.gate_token_digest,
        gate_binding: pending.gate_binding,
        outer_journey_id: outer.journey_id.into(),
        outer_profile: outer.profile.into(),
        outer_run_id: outer.run_id.into(),
        outer_journey_hash: outer.journey_hash.into(),
        outer_surface_hash: outer.surface_hash.into(),
        outer_compiler_version: outer.compiler_version.into(),
        outer_proof_hash: outer.proof_hash.into(),
        derivations: pending.derivations,
        candidate_permits,
    })
}
