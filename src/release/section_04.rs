fn imported_executable_surfaces(export: &crate::travel::Export) -> Result<Vec<Node>> {
    let mut snapshot = export.clone().into_snapshot();
    crate::travel::quarantine_imported_execution(&mut snapshot)?;
    let mut surfaces: Vec<Node> = snapshot
        .nodes
        .into_iter()
        .filter(|node| node.node_type == NodeType::InterfaceSurface && node.status == "quarantined")
        .collect();
    surfaces.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(surfaces)
}

fn candidate_manifest_attestations(
    root: &Path,
    imported_surfaces: &[Node],
    outer: &OuterJourneyAttestation,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<Vec<ManifestAttestation>> {
    if imported_surfaces.is_empty() {
        return Ok(Vec::new());
    }
    let manifest_root = root.join(SURFACE_MANIFEST_ROOT);
    if !manifest_root.is_dir() {
        bail!(
            "imported executable surfaces remain quarantined: candidate-owned canonical manifests are missing at '{SURFACE_MANIFEST_ROOT}'"
        );
    }
    let mut paths = regular_files_with_suffix(&manifest_root, ".surface.json")?;
    paths.sort();
    let imported: BTreeMap<&str, &Node> = imported_surfaces
        .iter()
        .map(|surface| (surface.name.as_str(), surface))
        .collect();
    let mut matched = BTreeSet::new();
    let mut attestations = Vec::new();
    for path in paths {
        let manifest = SurfaceManifest::parse_json(&path)?;
        let Some(surface) = imported.get(manifest.surface.id.as_str()) else {
            bail!(
                "candidate manifest '{}' does not reauthorize an imported quarantined surface",
                path.display()
            );
        };
        if surface.body != manifest.surface.node_body()? {
            bail!(
                "candidate manifest '{}' does not exactly match imported surface '{}'",
                path.display(),
                surface.name
            );
        }
        if !matched.insert(surface.id.clone()) {
            bail!("candidate repeats manifest for surface '{}'", surface.name);
        }
        let spec_path = journey_artifacts(root)?
            .into_iter()
            .find_map(|candidate| {
                crate::journey::parse(&candidate)
                    .ok()
                    .filter(|spec| spec.id == manifest.journey_id)
                    .map(|spec| (candidate, spec))
            })
            .ok_or_else(|| {
                anyhow!(
                    "candidate manifest '{}' has no authored Journey source for '{}'",
                    path.display(),
                    manifest.journey_id
                )
            })?;
        let (_, spec) = spec_path;
        let semantic_hash = spec.semantic_hash()?;
        manifest.validate_for(&spec, &semantic_hash)?;
        inspect_candidate_manifest_operations(&spec, &manifest, outer, ledger)?;
        let codefile_path = root.join(&manifest.surface.codefile);
        let metadata = fs::symlink_metadata(&codefile_path).with_context(|| {
            format!(
                "candidate manifest '{}' exposes missing CodeFile '{}'",
                path.display(),
                manifest.surface.codefile
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!(
                "candidate manifest '{}' exposes non-regular CodeFile '{}'",
                path.display(),
                manifest.surface.codefile
            );
        }
        let relative = path
            .strip_prefix(root)
            .context("candidate manifest escaped detached root")?;
        let canonical = canonical_json(serde_json::to_value(&manifest)?);
        attestations.push(ManifestAttestation {
            journey_id: manifest.journey_id,
            surface_id: manifest.surface.id,
            path: relative.to_string_lossy().into_owned(),
            manifest_hash: crate::artifact::fingerprint(&serde_json::to_string(&canonical)?),
            codefile: manifest.surface.codefile,
            locator: manifest.surface.locator,
        });
    }
    let missing: Vec<&str> = imported_surfaces
        .iter()
        .filter(|surface| !matched.contains(&surface.id))
        .map(|surface| surface.name.as_str())
        .collect();
    if !missing.is_empty() {
        bail!(
            "imported executable surfaces remain quarantined without candidate manifests: {}",
            missing.join(", ")
        );
    }
    attestations.sort_by(|left, right| left.surface_id.cmp(&right.surface_id));
    Ok(attestations)
}

#[doc(hidden)]
pub fn inspect_candidate_manifest_operations(
    spec: &crate::journey::JourneySpec,
    manifest: &SurfaceManifest,
    outer: &OuterJourneyAttestation,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    let plan = match crate::candidate_surface_policy::inspect_manifest(
        spec,
        manifest,
        crate::candidate_surface_policy::PolicyMode::DetachedReleaseInspection {
            outer_journey_id: &outer.journey_id,
            outer_surface_id: "loom-release-rehearsal",
        },
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let operation = manifest.surface.operations.first();
            ledger.push(ArgvLedgerEntry {
                source: format!("candidate_manifest:{}:policy", manifest.surface.id),
                executable: operation
                    .and_then(|operation| operation.argv.first())
                    .cloned()
                    .unwrap_or_default(),
                argv: operation
                    .and_then(|operation| operation.argv.get(1..))
                    .unwrap_or_default()
                    .to_vec(),
                policy: "candidate-surface/v1".into(),
                attempted: false,
                outcome: "rejected_candidate_surface_policy".into(),
            });
            return Err(error);
        }
    };
    let operations: BTreeMap<&str, &crate::journey::CliOperation> = manifest
        .surface
        .operations
        .iter()
        .map(|operation| (operation.id.as_str(), operation))
        .collect();
    for inspection in plan.inspections() {
        let operation = operations
            .get(inspection.operation_id.as_str())
            .ok_or_else(|| {
                // A plan referencing an operation the manifest no longer
                // declares is an inconsistent trust artifact — a release
                // gate refuses it instead of panicking mid-ledger.
                anyhow!(
                    "candidate surface plan references operation '{}' that the manifest does not declare",
                    inspection.operation_id
                )
            })?;
        ledger.push(ArgvLedgerEntry {
            source: format!(
                "candidate_manifest:{}:{}",
                manifest.surface.id, inspection.operation_id
            ),
            executable: operation.argv.first().cloned().unwrap_or_default(),
            argv: operation.argv.get(1..).unwrap_or_default().to_vec(),
            policy: if inspection.outcome == "suppressed_exact_outer" {
                "outer_profile_compile_only".into()
            } else {
                plan.policy_version().into()
            },
            attempted: false,
            outcome: inspection.outcome.clone(),
        });
        for nested in &inspection.nested {
            ledger.push(ArgvLedgerEntry {
                source: format!(
                    "candidate_manifest:{}:{}/{}",
                    manifest.surface.id, inspection.operation_id, nested.source
                ),
                executable: String::new(),
                argv: Vec::new(),
                policy: plan.policy_version().into(),
                attempted: false,
                outcome: nested.outcome.clone(),
            });
        }
    }
    Ok(())
}

fn run_code_gates(
    root: &Path,
    gates: &[Vec<String>],
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    for gate in gates {
        let executable = Path::new(&gate[0]);
        let args = gate[1..].to_vec();
        execute_checked(
            executor,
            sandbox,
            CheckedInvocation {
                cwd: root,
                executable,
                argv: &args,
                source: "code_gate",
            },
            ledger,
        )
        .with_context(|| format!("release code gate `{}` failed", gate.join(" ")))?;
    }
    Ok(())
}

fn seed_candidate_graph(
    root: &Path,
    binary: &Path,
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<()> {
    const TEST_IGNORE_REASON: &str = "Tests are Validation/proof artifacts, not implementation ownership; literal test paths may be re-registered when an Exercises edge needs source-drift tracking.";
    let ignores = run_loom(
        root,
        binary,
        &["ignore", "list", "--json"],
        executor,
        sandbox,
        ledger,
    )?;
    let ignore_rows: serde_json::Value = serde_json::from_slice(&ignores.stdout)
        .context("release candidate ignore list is not JSON")?;
    let matching: Vec<&serde_json::Value> = ignore_rows
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["glob"] == "tests/**")
        .collect();
    match matching.as_slice() {
        [] => {
            run_loom(
                root,
                binary,
                &[
                    "ignore",
                    "add",
                    "tests/**",
                    "--reason",
                    TEST_IGNORE_REASON,
                    "--json",
                ],
                executor,
                sandbox,
                ledger,
            )?;
        }
        [row] if row["reason"] == TEST_IGNORE_REASON => {}
        _ => bail!("release candidate has conflicting tests/** coverage policy"),
    }
    run_loom(
        root,
        binary,
        &["codefile", "add", "src/**/*.rs", "--json"],
        executor,
        sandbox,
        ledger,
    )?;
    run_loom(
        root,
        binary,
        &["codefile", "add", "scripts/*.sh", "--json"],
        executor,
        sandbox,
        ledger,
    )?;
    run_loom(root, binary, &["sync", "--json"], executor, sandbox, ledger)?;
    for path in journey_artifacts(root)? {
        let relative = path
            .strip_prefix(root)
            .context("Journey artifact escaped detached candidate")?;
        run_loom(
            root,
            binary,
            &["journey", "add", &relative.to_string_lossy(), "--json"],
            executor,
            sandbox,
            ledger,
        )?;
    }
    Ok(())
}

struct CandidateExec<'a> {
    binary: &'a Path,
    executor: &'a mut dyn ReleaseExecutor,
    sandbox: &'a ProcessSandbox,
    ledger: &'a mut Vec<ArgvLedgerEntry>,
}

fn replay_derivation_authority(
    root: &Path,
    authority: &BoundDerivationAuthority,
    permit: &DerivationCandidatePermit,
    exec: &mut CandidateExec<'_>,
) -> Result<()> {
    if !authority.candidate_permits.contains(permit) {
        bail!("detached candidate received an unbound derivation authority permit");
    }
    let manifest_root = root
        .join(".release-sandbox")
        .join("derivation-authority")
        .join(&permit.permit_hash);
    fs::create_dir_all(&manifest_root)?;
    let manifest_root = manifest_root.canonicalize()?;
    if !manifest_root.starts_with(root.join(".release-sandbox").canonicalize()?) {
        bail!("candidate derivation manifests escaped the detached sandbox");
    }
    let decision = match &authority.human_decision {
        crate::ratification::HumanDecision::Mediated { response } => response.as_str(),
        crate::ratification::HumanDecision::Direct { .. } => {
            bail!("release derivation replay requires host-mediated human authority")
        }
    };
    let specs: BTreeMap<String, crate::journey::JourneySpec> = journey_artifacts(root)?
        .into_iter()
        .map(|path| {
            let spec = crate::journey::parse(&path)?;
            Ok((spec.id.clone(), spec))
        })
        .collect::<Result<_>>()?;
    let mut builder_sandbox = exec.sandbox.clone();
    builder_sandbox
        .environment
        .insert(crate::identity::AGENT_ENV.into(), "llm:builder".into());
    builder_sandbox.environment.insert(
        crate::identity::PROFILE_ENV.into(),
        "release-rehearsal".into(),
    );
    for subject in &authority.derivations {
        let spec = specs.get(&subject.journey_id).ok_or_else(|| {
            anyhow!(
                "approved derivation Journey '{}' is missing from the candidate",
                subject.journey_id
            )
        })?;
        let semantic_hash = spec.semantic_hash()?;
        subject.manifest.validate_for(spec, &semantic_hash)?;
        if semantic_hash != subject.journey_hash
            || subject.manifest.proposal_id != subject.proposal_id
        {
            bail!(
                "approved derivation for '{}' is stale in the candidate",
                subject.journey_id
            );
        }
        let canonical = canonical_json(serde_json::to_value(&subject.manifest)?);
        let canonical_text = serde_json::to_string(&canonical)?;
        if crate::artifact::fingerprint(&canonical_text) != subject.manifest_hash {
            bail!(
                "approved derivation manifest for '{}' changed before replay",
                subject.journey_id
            );
        }
        let path = manifest_root.join(format!("{}.json", subject.journey_id));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options.open(&path)?;
        use std::io::Write;
        file.write_all(canonical_text.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        let relative = path
            .strip_prefix(root)
            .context("candidate derivation manifest escaped detached root")?
            .to_string_lossy()
            .into_owned();
        run_loom(
            root,
            exec.binary,
            &[
                "journey",
                "derive-accept",
                &subject.journey_id,
                "--manifest",
                &relative,
                "--human-decision",
                decision,
                "--json",
            ],
            exec.executor,
            &builder_sandbox,
            exec.ledger,
        )?;
    }
    // The import boundary deliberately voids ratification authority, and the
    // derive-accepts above re-establish it only for derivation subjects. The
    // remaining domain intents would read needs_reconfirmation forever inside
    // the candidate, so the audit-clean discrimination journeys could never
    // measure the state they claim. Re-ratify them under the same sealed
    // human decision that authorized this rehearsal; `intent ratify --all`
    // seals its own batch envelope before writing.
    run_loom(
        root,
        exec.binary,
        &[
            "intent",
            "ratify",
            "--all",
            "--evidence",
            "release rehearsal: re-establish domain-intent authority voided by candidate import, under the sealed rehearsal decision",
            "--human-decision",
            decision,
            "--json",
        ],
        exec.executor,
        &builder_sandbox,
        exec.ledger,
    )?;
    Ok(())
}

fn run_candidate_journeys(
    root: &Path,
    outer: &OuterJourneyAttestation,
    expected_bindings: &BTreeMap<String, (String, String)>,
    exec: &mut CandidateExec<'_>,
) -> Result<Vec<JourneyResultSummary>> {
    let mut excluded = 0usize;
    let mut summaries = Vec::new();
    let mut identities = BTreeSet::new();
    for path in journey_artifacts(root)? {
        let spec = crate::journey::parse(&path)?;
        let (expected_journey_hash, expected_surface_hash) =
            expected_bindings.get(&spec.id).ok_or_else(|| {
                anyhow!(
                    "Journey '{}' has no current accepted source binding",
                    spec.id
                )
            })?;
        for profile in spec.profiles.keys() {
            if spec.id == outer.journey_id && profile == &outer.profile {
                excluded += 1;
                let observed = run_loom(
                    root,
                    exec.binary,
                    &[
                        "journey",
                        "compile",
                        &spec.id,
                        "--profile",
                        profile,
                        "--json",
                    ],
                    exec.executor,
                    exec.sandbox,
                    exec.ledger,
                )?;
                require_outer_compile_report(&observed.stdout, outer)?;
                summaries.push(JourneyResultSummary {
                    journey_id: spec.id.clone(),
                    profile: profile.clone(),
                    journey_hash: outer.journey_hash.clone(),
                    surface_hash: outer.surface_hash.clone(),
                    verdict: "compiled_exact_outer".into(),
                });
                continue;
            }
            let observed = run_loom(
                root,
                exec.binary,
                &["journey", "run", &spec.id, "--profile", profile, "--json"],
                exec.executor,
                exec.sandbox,
                exec.ledger,
            )?;
            if let Some(pending) = pending_human_gate(&observed.stdout)? {
                require_declared_human_gate(root, &spec.id, profile, &pending)?;
                let journey_hash = &pending.binding.journey_hash;
                let surface_hash = &pending.binding.surface_hash;
                if journey_hash != expected_journey_hash || surface_hash != expected_surface_hash {
                    bail!("pending Journey report has a stale Journey or surface hash");
                }
                summaries.push(JourneyResultSummary {
                    journey_id: spec.id.clone(),
                    profile: profile.clone(),
                    journey_hash: journey_hash.into(),
                    surface_hash: surface_hash.into(),
                    verdict: "pending_human".into(),
                });
                continue;
            }
            require_passed_journey_report_with_sandbox(
                &observed.stdout,
                &spec.id,
                profile,
                spec.steps.len(),
                Some(exec.sandbox),
            )?;
            let report: crate::journey_runtime::RuntimeReport =
                serde_json::from_slice(&observed.stdout)?;
            if &report.journey_hash != expected_journey_hash
                || &report.surface_hash != expected_surface_hash
            {
                bail!("Journey report has a stale Journey or surface hash");
            }
            summaries.push(JourneyResultSummary {
                journey_id: spec.id.clone(),
                profile: profile.clone(),
                journey_hash: report.journey_hash,
                surface_hash: report.surface_hash,
                verdict: "passed".into(),
            });
        }
    }
    if excluded != 1 {
        bail!(
            "nested verifier must exclude exactly one outer release-workflow/proof profile (found {excluded})"
        );
    }
    summaries.sort_by(|left, right| {
        (&left.journey_id, &left.profile).cmp(&(&right.journey_id, &right.profile))
    });
    for summary in &summaries {
        if !identities.insert((summary.journey_id.clone(), summary.profile.clone())) {
            bail!("candidate Journey execution produced duplicate journey/profile summaries");
        }
    }
    Ok(summaries)
}
