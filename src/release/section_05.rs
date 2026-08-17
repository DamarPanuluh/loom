fn current_journey_bindings(root: &Path) -> Result<BTreeMap<String, (String, String)>> {
    let store = Store::open(root)?;
    let mut bindings = BTreeMap::new();
    for path in journey_artifacts(root)? {
        let spec = crate::journey::parse(&path)?;
        let journey = store.resolve_node(&spec.id, Some(NodeType::Journey))?;
        let surface_hash = crate::journey::surface_projection_hash(&store, &journey)?
            .ok_or_else(|| anyhow!("Journey '{}' has no current accepted surface", spec.id))?;
        let journey_hash = spec.semantic_hash()?;
        bindings.insert(spec.id, (journey_hash, surface_hash));
    }
    Ok(bindings)
}

fn require_outer_compile_report(bytes: &[u8], outer: &OuterJourneyAttestation) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("outer Journey compile output is not JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("outer Journey compile output is not an object"))?;
    let exact = object.get("compiled").and_then(serde_json::Value::as_bool) == Some(true)
        && object.get("journey_id").and_then(serde_json::Value::as_str)
            == Some(outer.journey_id.as_str())
        && object.get("profile").and_then(serde_json::Value::as_str)
            == Some(outer.profile.as_str())
        && object
            .get("journey_hash")
            .and_then(serde_json::Value::as_str)
            == Some(outer.journey_hash.as_str())
        && object
            .get("surface_hash")
            .and_then(serde_json::Value::as_str)
            == Some(outer.surface_hash.as_str())
        && object
            .get("compiler_version")
            .and_then(serde_json::Value::as_str)
            == Some(outer.compiler_version.as_str());
    if !exact {
        bail!("outer release-workflow compile attestation is incomplete or stale");
    }
    Ok(())
}

#[doc(hidden)]
pub fn require_passed_journey_report(
    bytes: &[u8],
    journey_id: &str,
    profile: &str,
    expected_steps: usize,
) -> Result<()> {
    require_passed_journey_report_with_sandbox(bytes, journey_id, profile, expected_steps, None)
}

/// A headless candidate cannot answer a host-mediated gate: missing human
/// authority is a pause, not a failure. A suspended run is acceptable in the
/// isolated dogfood only when the journey's canonical candidate manifest
/// genuinely declares that gate.
fn pending_human_gate(bytes: &[u8]) -> Result<Option<crate::journey_gate::PendingHuman>> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Ok(None);
    };
    if value.get("schema").and_then(serde_json::Value::as_str)
        == Some(crate::journey_gate::PENDING_HUMAN_SCHEMA)
        && value.get("status").and_then(serde_json::Value::as_str) == Some("pending_human")
    {
        let pending: crate::journey_gate::PendingHuman = serde_json::from_value(value)
            .context("pending Journey output is not one canonical human-gate report")?;
        pending.validate()?;
        Ok(Some(pending))
    } else {
        Ok(None)
    }
}

fn require_declared_human_gate(
    root: &Path,
    journey_id: &str,
    profile: &str,
    pending: &crate::journey_gate::PendingHuman,
) -> Result<()> {
    if pending.binding.journey_id != journey_id || pending.binding.profile != profile {
        bail!(
            "pending Journey gate binding '{}':'{}' does not match the dogfood run '{journey_id}':'{profile}'",
            pending.binding.journey_id,
            pending.binding.profile
        );
    }
    let manifest_path = root
        .join(SURFACE_MANIFEST_ROOT)
        .join(format!("{journey_id}.surface.json"));
    let manifest = crate::journey::SurfaceManifest::parse_json(&manifest_path)?;
    if !manifest.bindings.iter().any(|binding| {
        matches!(binding, crate::journey::SurfaceBinding::HumanDecision(_))
            && binding.step_id() == pending.binding.step_id
    }) {
        bail!(
            "Journey '{journey_id}' suspended at a human gate step its canonical manifest never declares"
        );
    }
    Ok(())
}

fn require_passed_journey_report_with_sandbox(
    bytes: &[u8],
    journey_id: &str,
    profile: &str,
    expected_steps: usize,
    sandbox: Option<&ProcessSandbox>,
) -> Result<()> {
    let report: crate::journey_runtime::RuntimeReport = serde_json::from_slice(bytes)
        .context("Journey run output is not one complete runtime report")?;
    if report.journey_id != journey_id
        || report.profile != profile
        || report.status != crate::journey_runtime::RuntimeStatus::Passed
        || report.assertions_failed != 0
        || report.steps.len() != expected_steps
        || report
            .steps
            .iter()
            .any(|step| step.exit_code != 0 || step.assertions_failed != 0)
    {
        let failing_steps: Vec<_> = report
            .steps
            .iter()
            .filter(|step| step.exit_code != 0 || step.assertions_failed != 0)
            .take(8)
            .map(|step| {
                serde_json::json!({
                    "step_id": bounded_diagnostic_text(&step.step_id, 256),
                    "operation_id": bounded_diagnostic_text(&step.operation_id, 256),
                    "exit_code": step.exit_code,
                    "assertions_passed": step.assertions_passed,
                    "assertions_failed": step.assertions_failed,
                })
            })
            .collect();
        // Name the checks, not just how many. A candidate Journey that refuses
        // costs a sealed one-shot authority token to observe; reporting
        // "assertions_failed: 1" and no identity makes the next token the only
        // way to learn which one, and that is a diagnostic the runtime already
        // held.
        let failed_assertions: Vec<_> = report
            .failed_assertions
            .iter()
            .take(8)
            .map(|failed| {
                serde_json::json!({
                    "operation_id": bounded_diagnostic_text(&failed.operation_id, 256),
                    "assertion_id": bounded_diagnostic_text(&failed.assertion_id, 256),
                    "pointer": bounded_diagnostic_text(&failed.pointer, 256),
                    "kind": failed.kind,
                })
            })
            .collect();
        let failed_assertions_omitted = report.failed_assertions.len().saturating_sub(8);
        let detail = report.detail.as_deref().map(|detail| match sandbox {
            Some(sandbox) => release_diagnostic_stream(detail.as_bytes(), sandbox),
            None => bounded_diagnostic_text(detail, RELEASE_DIAGNOSTIC_BYTES),
        });
        let diagnostic = serde_json::json!({
            "expected": {
                "journey_id": journey_id,
                "profile": profile,
                "status": "passed",
                "assertions_failed": 0,
                "step_count": expected_steps,
            },
            "observed": {
                "journey_id": bounded_diagnostic_text(&report.journey_id, 256),
                "profile": bounded_diagnostic_text(&report.profile, 256),
                "status": report.status.as_str(),
                "assertions_passed": report.assertions_passed,
                "assertions_failed": report.assertions_failed,
                "step_count": report.steps.len(),
                "detail": detail,
                "failing_steps": failing_steps,
                "failing_steps_omitted": report.steps.iter()
                    .filter(|step| step.exit_code != 0 || step.assertions_failed != 0)
                    .count().saturating_sub(8),
                "failed_assertions": failed_assertions,
                "failed_assertions_omitted": failed_assertions_omitted,
            }
        });
        bail!(
            "Journey '{journey_id}:{profile}' did not return one complete passing report: {}",
            bounded_diagnostic_text(
                &serde_json::to_string(&diagnostic)?,
                RELEASE_DIAGNOSTIC_BYTES
            )
        );
    }
    Ok(())
}

#[doc(hidden)]
pub fn require_clean_coverage(bytes: &[u8]) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("release candidate coverage output is not JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("release candidate coverage output is not an object"))?;
    let planned = object
        .get("intents")
        .and_then(|value| value.get("planned_or_needs_change"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing intents.planned_or_needs_change"))?;
    let ungrounded = object
        .get("grounding")
        .and_then(|value| value.get("ungrounded"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing grounding.ungrounded"))?;
    let codefiles = object
        .get("codefiles")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!("coverage is missing codefiles"))?;
    let registered = codefiles
        .get("registered")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.registered"))?;
    let owned = codefiles
        .get("owned")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.owned"))?;
    let _observed = codefiles
        .get("observed")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.observed"))?;
    let unowned = codefiles
        .get("unowned")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("coverage is missing codefiles.unowned"))?;
    if planned != 0
        || ungrounded != 0
        || unowned != 0
        || registered != owned.saturating_add(unowned)
    {
        bail!(
            "release candidate coverage is blocking: planned={planned}, ungrounded={ungrounded}, unowned={unowned}"
        );
    }
    Ok(())
}

#[doc(hidden)]
pub fn require_clean_drift(bytes: &[u8]) -> Result<()> {
    require_clean_drift_excusing_gates(bytes, &BTreeSet::new())
}

/// A journey suspended at a declared host-mediated gate never settles its
/// compiled projection inside a headless candidate — that pause is authority
/// hygiene, not drift. Every other stale projection still blocks, and the
/// reported stale count must be exactly the excused pauses.
#[doc(hidden)]
pub fn require_clean_drift_excusing_gates(
    bytes: &[u8],
    human_gated: &BTreeSet<String>,
) -> Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .context("release candidate Journey drift output is not JSON")?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("release candidate Journey drift output is not an object"))?;
    let rows = object
        .get("journeys")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("release candidate Journey drift journeys is not an array"))?;
    let stale = object
        .get("stale")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("release candidate Journey drift is missing stale count"))?;
    let mut excused = 0u64;
    for (index, row) in rows.iter().enumerate() {
        let current = row
            .as_object()
            .and_then(|object| object.get("current"))
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| anyhow!("Journey drift row {index} is missing boolean current"))?;
        if current {
            continue;
        }
        let journey_id = row
            .as_object()
            .and_then(|object| object.get("journey_id"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("Journey drift row {index} is missing journey_id"))?;
        if human_gated.contains(journey_id) {
            excused += 1;
            continue;
        }
        bail!("release candidate retains stale Journey projections");
    }
    if stale != excused {
        bail!(
            "release candidate reports {stale} stale Journey projection(s) beyond {excused} declared human-gate pause(s)"
        );
    }
    Ok(())
}

/// Journey ids whose canonical candidate manifests declare a host-mediated
/// human-decision binding.
fn human_gated_journeys(root: &Path) -> Result<BTreeSet<String>> {
    let mut gated = BTreeSet::new();
    let manifest_root = root.join(SURFACE_MANIFEST_ROOT);
    if !manifest_root.is_dir() {
        return Ok(gated);
    }
    for entry in fs::read_dir(&manifest_root)? {
        let path = entry?.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(manifest) = SurfaceManifest::parse_json(&path) else {
            continue;
        };
        if manifest
            .bindings
            .iter()
            .any(|binding| matches!(binding, crate::journey::SurfaceBinding::HumanDecision(_)))
        {
            gated.insert(manifest.journey_id.clone());
        }
    }
    Ok(gated)
}

fn semantic_result_hash(
    root: &Path,
    manifests: &[ManifestAttestation],
    outer: &OuterJourneyAttestation,
    journey_summaries: &[JourneyResultSummary],
) -> Result<String> {
    let value = serde_json::json!({
        "candidate_hash": hash_tree(root, &RESULT_EXCLUDES)?,
        "manifests": manifests,
        "outer_journey": outer.journey_id,
        "outer_profile": outer.profile,
        "journey_summaries": journey_summaries,
        "schema_version": crate::SCHEMA_VERSION,
    });
    Ok(crate::artifact::fingerprint(&serde_json::to_string(
        &value,
    )?))
}

fn journey_artifacts(root: &Path) -> Result<Vec<PathBuf>> {
    let journey_root = root.join("journeys");
    let mut paths = Vec::new();
    collect_regular_files(&journey_root, &mut paths, &|path| {
        matches!(
            path.extension().and_then(OsStr::to_str),
            Some("yaml" | "yml" | "json")
        ) && !path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(".surface.json"))
    })?;
    paths.sort();
    Ok(paths)
}

fn regular_files_with_suffix(root: &Path, suffix: &str) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    collect_regular_files(root, &mut paths, &|path| {
        path.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(suffix))
    })?;
    Ok(paths)
}

fn collect_regular_files(
    root: &Path,
    out: &mut Vec<PathBuf>,
    keep: &impl Fn(&Path) -> bool,
) -> Result<()> {
    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() {
        bail!(
            "release verifier refuses symlinked path '{}'",
            root.display()
        );
    }
    let mut entries: Vec<_> = fs::read_dir(root)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            bail!(
                "release verifier refuses symlinked path '{}'",
                path.display()
            );
        }
        if metadata.is_dir() {
            collect_regular_files(&path, out, keep)?;
        } else if metadata.is_file() && keep(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn run_loom(
    root: &Path,
    binary: &Path,
    args: &[&str],
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<CommandObservation> {
    let mut full = vec!["--graph".into(), ".".into()];
    full.extend(args.iter().map(|arg| (*arg).to_string()));
    execute_checked(
        executor,
        sandbox,
        CheckedInvocation {
            cwd: root,
            executable: binary,
            argv: &full,
            source: "candidate_loom",
        },
        ledger,
    )
}

struct SystemReleaseExecutor;

impl ReleaseExecutor for SystemReleaseExecutor {
    fn execute(
        &mut self,
        cwd: &Path,
        executable: &Path,
        argv: &[String],
        environment: &BTreeMap<String, String>,
    ) -> Result<CommandObservation> {
        let output = Command::new(executable)
            .args(argv)
            .current_dir(cwd)
            .env_clear()
            .envs(environment)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("starting {}", executable.display()))?;
        Ok(CommandObservation {
            success: output.status.success(),
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

struct DependencyCacheGuard {
    roots: Vec<(String, PathBuf, String)>,
}

/// The toolchain-documented default for a declared cache root when the
/// environment does not export one. Cargo reads `$CARGO_HOME` or falls back to
/// `$HOME/.cargo`; rustup reads `$RUSTUP_HOME` or falls back to
/// `$HOME/.rustup`. A stock install exports neither, so resolving only through
/// the environment refused the exact default layout the gate exists to verify.
/// Unknown names have no documented default and stay a hard error.
fn default_cache_root(name: &str) -> Option<PathBuf> {
    let relative = match name {
        "CARGO_HOME" => ".cargo",
        "RUSTUP_HOME" => ".rustup",
        _ => return None,
    };
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(relative))
}

impl DependencyCacheGuard {
    fn open(candidate_root: &Path, names: &[String]) -> Result<Self> {
        let candidate_root = candidate_root.canonicalize()?;
        let mut roots = Vec::new();
        for name in names {
            // Resolve exactly as the toolchain does: an explicit export wins,
            // otherwise the documented default. Every downstream check below
            // (absolute, existing, non-symlink directory, non-overlapping)
            // still applies to the resolved path, so the fallback widens where
            // the root may be found without weakening what it must be.
            let configured = match std::env::var(name) {
                Ok(value) => PathBuf::from(value),
                Err(std::env::VarError::NotPresent) => {
                    default_cache_root(name).ok_or_else(|| {
                        anyhow!(
                            "release cache root environment '{name}' is unset and has no documented default"
                        )
                    })?
                }
                Err(error) => {
                    return Err(anyhow::Error::new(error)).with_context(|| {
                        format!("release cache root environment '{name}' is not valid UTF-8")
                    })
                }
            };
            if !configured.is_absolute() {
                bail!("release cache root environment '{name}' must name an absolute path");
            }
            let metadata = fs::symlink_metadata(&configured).with_context(|| {
                format!("release cache root environment '{name}' does not exist")
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("release cache root environment '{name}' must name a non-symlink directory");
            }
            let path = configured.canonicalize()?;
            if path.starts_with(&candidate_root) || candidate_root.starts_with(&path) {
                bail!("release cache root environment '{name}' overlaps the repository");
            }
            if roots
                .iter()
                .any(|(_, prior, _): &(String, PathBuf, String)| {
                    path.starts_with(prior) || prior.starts_with(&path)
                })
            {
                bail!("release cache roots must be distinct and non-overlapping");
            }
            let before = cache_root_hash(&path)?;
            roots.push((name.clone(), path, before));
        }
        Ok(Self { roots })
    }

    fn path_for(&self, environment: &str) -> Result<&Path> {
        self.roots
            .iter()
            .find(|(name, _, _)| name == environment)
            .map(|(_, path, _)| path.as_path())
            .ok_or_else(|| anyhow!("release cache roots do not declare '{environment}'"))
    }

    fn attest(&self) -> Result<DependencyCacheAttestation> {
        let roots: Vec<CacheRootAttestation> = self
            .roots
            .iter()
            .map(|(environment, path, before_hash)| {
                let after_hash = cache_root_hash(path)?;
                Ok(CacheRootAttestation {
                    environment: environment.clone(),
                    path: path.to_string_lossy().into_owned(),
                    before_hash: before_hash.clone(),
                    unchanged: before_hash == &after_hash,
                    after_hash,
                })
            })
            .collect::<Result<_>>()?;
        let before_projection: Vec<_> = roots
            .iter()
            .map(|root| (&root.environment, &root.path, &root.before_hash))
            .collect();
        let after_projection: Vec<_> = roots
            .iter()
            .map(|root| (&root.environment, &root.path, &root.after_hash))
            .collect();
        let before_hash = fingerprint_bytes(&serde_json::to_vec(&before_projection)?);
        let after_hash = fingerprint_bytes(&serde_json::to_vec(&after_projection)?);
        Ok(DependencyCacheAttestation {
            strategy: "declared_cache_roots_before_after_verified".into(),
            unchanged: roots.iter().all(|root| root.unchanged),
            roots,
            before_hash: before_hash.clone(),
            after_hash,
            offline: true,
        })
    }
}
