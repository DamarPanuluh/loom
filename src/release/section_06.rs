fn cache_root_hash(root: &Path) -> Result<String> {
    // Cargo updates these coordination/access-index files even for an offline
    // read-only build. They contain no dependency artifacts; registry/index,
    // registry/cache, registry/src, git, and toolchain bytes remain attested.
    hash_tree(
        root,
        &[".global-cache", ".package-cache", ".package-cache-mut"],
    )
}

/// Bounded production-adapter smoke: check this checkout's host library target
/// against its lockfile in offline mode with the same isolated process
/// environment used by a rehearsal. Build output stays in the detached temp;
/// it does not run a Journey or write a release artifact.
#[doc(hidden)]
pub fn dependency_cache_smoke(root: &Path) -> Result<DependencyCacheAttestation> {
    let root = root.canonicalize()?;
    let (inventory, _) = load_source_inventory(&root)?;
    let dependency_cache = DependencyCacheGuard::open(&root, &inventory.cache_root_environment)?;
    let temp = DetachedCandidate::allocate(&root, "dependency-cache-smoke")?;
    dependency_cache.path_for("CARGO_HOME")?;
    let sandbox = ProcessSandbox::create(temp.path(), &dependency_cache)?;
    let mut executor = SystemReleaseExecutor;
    let mut ledger = Vec::new();
    let argv = ["check", "--locked", "--offline", "--lib"]
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    execute_checked(
        &mut executor,
        &sandbox,
        CheckedInvocation {
            cwd: &root,
            executable: Path::new("cargo"),
            argv: &argv,
            source: "dependency_cache_smoke",
        },
        &mut ledger,
    )?;
    // The smoke runs inside ProcessSandbox; host CARGO_HOME hash drift is
    // idle coordination, not a failed gate. Report the attestation honestly
    // and let the sandbox's checked execution be the proof.
    dependency_cache.attest()
}

#[derive(Clone)]
struct ProcessSandbox {
    environment: BTreeMap<String, String>,
    _external_temp: Arc<ExternalProcessTemp>,
}

impl ProcessSandbox {
    fn create(candidate: &Path, dependency_cache: &DependencyCacheGuard) -> Result<Self> {
        let root = candidate.join(".release-sandbox");
        let home = root.join("home");
        let external_temp = Arc::new(ExternalProcessTemp::allocate(candidate)?);
        let temp = external_temp.path();
        fs::create_dir_all(&home)?;
        let git_config = root.join("gitconfig");
        fs::write(
            &git_config,
            b"[credential]\n\thelper =\n[commit]\n\tgpgsign = false\n",
        )?;
        let mut environment = BTreeMap::new();
        for key in [
            "PATH",
            "SYSTEMROOT",
            "WINDIR",
            "PATHEXT",
            "COMSPEC",
            "CARGO_BUILD_JOBS",
        ] {
            if let Ok(value) = std::env::var(key) {
                environment.insert(key.into(), value);
            }
        }
        environment.insert("HOME".into(), home.to_string_lossy().into_owned());
        for (name, path, _) in &dependency_cache.roots {
            environment.insert(name.clone(), path.to_string_lossy().into_owned());
        }
        environment.insert("TMPDIR".into(), temp.to_string_lossy().into_owned());
        environment.insert("TEMP".into(), temp.to_string_lossy().into_owned());
        environment.insert("TMP".into(), temp.to_string_lossy().into_owned());
        environment.insert("CARGO_NET_OFFLINE".into(), "true".into());
        environment.insert(
            "CARGO_TARGET_DIR".into(),
            candidate.join("target").to_string_lossy().into_owned(),
        );
        environment.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        environment.insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        environment.insert(
            "GIT_CONFIG_GLOBAL".into(),
            git_config.to_string_lossy().into_owned(),
        );
        environment.insert("GIT_CONFIG_SYSTEM".into(), "/dev/null".into());
        Ok(Self {
            environment,
            _external_temp: external_temp,
        })
    }
}

struct ExternalProcessTemp(PathBuf);

/// Allocate a fresh, collision-safe directory under `parent`, named
/// `<prefix>-<pid>-<nanos>-<sequence>`. Both detached owners (the process
/// temp and the release candidate) share this one allocator so their retry
/// bounds, naming, and exhaustion behavior cannot drift apart.
fn allocate_detached_dir(parent: &Path, prefix: &str, exhausted: &str) -> Result<PathBuf> {
    for sequence in 0..1000_u32 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let path = parent.join(format!(
            "{prefix}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    bail!("{exhausted}")
}

impl ExternalProcessTemp {
    fn allocate(candidate: &Path) -> Result<Self> {
        let candidate = candidate.canonicalize()?;
        let parent = candidate
            .parent()
            .ok_or_else(|| anyhow!("detached release candidate has no parent"))?
            .canonicalize()?;
        if parent.starts_with(&candidate) {
            bail!("release process temp parent must be outside the candidate");
        }
        Ok(Self(allocate_detached_dir(
            &parent,
            "loom-release-process-temp",
            "could not allocate detached release process temp root",
        )?))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

struct CheckedInvocation<'a> {
    cwd: &'a Path,
    executable: &'a Path,
    argv: &'a [String],
    source: &'a str,
}

fn execute_checked(
    executor: &mut dyn ReleaseExecutor,
    sandbox: &ProcessSandbox,
    invocation: CheckedInvocation<'_>,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<CommandObservation> {
    let policy = inspect_process_argv(invocation.executable, invocation.argv)?;
    let index = ledger.len();
    ledger.push(ArgvLedgerEntry {
        source: invocation.source.into(),
        executable: invocation.executable.to_string_lossy().into_owned(),
        argv: invocation.argv.to_vec(),
        policy: policy.into(),
        attempted: true,
        outcome: "started".into(),
    });
    let observed = executor.execute(
        invocation.cwd,
        invocation.executable,
        invocation.argv,
        &sandbox.environment,
    )?;
    ledger[index].outcome = if observed.success {
        "passed".into()
    } else {
        format!("failed_exit_{}", observed.exit_code)
    };
    if !observed.success {
        let stdout = release_diagnostic_stream(&observed.stdout, sandbox);
        let stderr = release_diagnostic_stream(&observed.stderr, sandbox);
        bail!(
            "{} {} exited {}\nstdout:\n{}\nstderr:\n{}",
            invocation.executable.display(),
            invocation.argv.join(" "),
            observed.exit_code,
            stdout,
            stderr,
        );
    }
    Ok(observed)
}

fn release_diagnostic_stream(bytes: &[u8], sandbox: &ProcessSandbox) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    for name in [
        "PATH",
        "HOME",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "CARGO_TARGET_DIR",
        "GIT_CONFIG_GLOBAL",
    ] {
        if let Some(secret) = sandbox
            .environment
            .get(name)
            .filter(|value| value.len() >= 4)
        {
            text = text.replace(secret, "[REDACTED]");
        }
    }
    bounded_diagnostic_text(&text, RELEASE_DIAGNOSTIC_BYTES)
}

fn bounded_diagnostic_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.trim().to_string();
    }
    let half = limit / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!(
        "{}\n...[diagnostic output omitted]...\n{}",
        text[..head].trim_end(),
        text[tail..].trim_start()
    )
}

fn inspect_process_argv(executable: &Path, argv: &[String]) -> Result<&'static str> {
    let name = executable
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "sh" | "bash" | "zsh" | "fish" | "env" | "python" | "python3" | "perl" | "ruby" | "node"
    ) {
        bail!("release verifier refuses shell/interpreter executable '{name}'");
    }
    if argv.iter().any(|part| part.contains('\0')) {
        bail!("release verifier refuses argv containing NUL");
    }
    if name == "cargo" {
        let subcommand = argv.first().map(String::as_str).unwrap_or("");
        if matches!(
            subcommand,
            "install" | "publish" | "package" | "login" | "owner"
        ) {
            bail!("release rehearsal refuses cargo mutation '{subcommand}'");
        }
        if !matches!(subcommand, "fmt" | "clippy" | "test" | "build" | "check") {
            bail!("release rehearsal refuses unapproved cargo command '{subcommand}'");
        }
        return Ok("candidate_code_gate");
    }
    if name == "git" {
        let subcommand = argv.first().map(String::as_str).unwrap_or("");
        if !matches!(subcommand, "-C" | "ls-files") {
            bail!("release rehearsal refuses Git command '{subcommand}'");
        }
        return Ok("candidate_file_plan");
    }
    if name == "loom" || name.ends_with("loom.exe") || name == "target/debug/loom" {
        if argv
            .windows(2)
            .any(|parts| parts == ["release", "rehearse"])
        {
            bail!("release verifier refuses nested release rehearsal execution");
        }
        return Ok("candidate_loom_gate");
    }
    if executable.to_string_lossy().ends_with("target/debug/loom") {
        return Ok("candidate_loom_gate");
    }
    bail!(
        "release verifier refuses unapproved executable '{}'",
        executable.display()
    )
}

struct DetachedCandidate(PathBuf);

impl DetachedCandidate {
    fn allocate(live_root: &Path, label: &str) -> Result<Self> {
        let parent = std::env::temp_dir().canonicalize()?;
        let live_root = live_root.canonicalize()?;
        if parent == live_root || parent.starts_with(&live_root) {
            bail!("release rehearsal temp root must be outside the caller repository");
        }
        Ok(Self(allocate_detached_dir(
            &parent,
            &format!("loom-release-{label}"),
            "could not allocate detached release candidate",
        )?))
    }

    fn copy(
        live_root: &Path,
        label: &str,
        ledger: &mut Vec<ArgvLedgerEntry>,
    ) -> Result<(Self, SourceInventoryAttestation)> {
        let candidate = Self::allocate(live_root, label)?;
        let inventory = copy_candidate(live_root, candidate.path(), ledger)?;
        Ok((candidate, inventory))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for DetachedCandidate {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_candidate(
    source: &Path,
    destination: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<SourceInventoryAttestation> {
    if destination.join(crate::LOOM_DIR).exists() {
        bail!(
            "detached release destination '{}' contains preinitialized .loom state",
            destination.display()
        );
    }
    if fs::read_dir(destination)?.next().is_some() {
        bail!(
            "detached release destination '{}' must be empty",
            destination.display()
        );
    }
    let (plan, attestation) = candidate_file_plan(source, ledger)?;
    for relative in plan {
        let from = source.join(&relative);
        let to = destination.join(&relative);
        let (mut source_file, metadata) = open_inventory_file(&from)?;
        fs::create_dir_all(
            to.parent()
                .ok_or_else(|| anyhow!("candidate file has no parent"))?,
        )?;
        let mode = normalized_mode(&metadata);
        let mut target = fs::File::create(&to)?;
        std::io::copy(&mut source_file, &mut target)?;
        set_inventory_permissions(&to, mode)?;
    }
    let copied_manifest: SourceInventoryManifest =
        serde_json::from_slice(&fs::read(destination.join(RELEASE_INVENTORY_PATH))?)?;
    validate_inventory_manifest(&copied_manifest)?;
    expand_inventory(destination, &copied_manifest)?;
    let copied_hash = inventory_content_hash(destination, &copied_manifest.files)?;
    if copied_hash != attestation.inventory_hash {
        bail!("materialized release snapshot does not match its inventory hash");
    }
    Ok(attestation)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInventoryManifest {
    schema: String,
    code_gates: Vec<Vec<String>>,
    cache_root_environment: Vec<String>,
    files: Vec<SourceInventoryEntry>,
    reserved_components: Vec<String>,
    secret_name_patterns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInventoryEntry {
    path: String,
    mode: String,
}

fn candidate_file_plan(
    root: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<(Vec<PathBuf>, SourceInventoryAttestation)> {
    let (manifest, manifest_hash) = load_source_inventory(root)?;
    let paths = expand_inventory(root, &manifest)?;
    validate_reserved_roots(root)?;
    let inventory_hash = inventory_content_hash(root, &manifest.files)?;

    let git_inventory = git_candidate_paths(root, ledger)?;
    let provenance = match git_inventory {
        Some(git_paths) => {
            let planned: BTreeMap<PathBuf, String> = manifest
                .files
                .iter()
                .map(|entry| (PathBuf::from(&entry.path), entry.mode.clone()))
                .collect();
            if planned != git_paths {
                let missing: Vec<String> = git_paths
                    .keys()
                    .filter(|path| !planned.contains_key(*path))
                    .take(5)
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                let extra: Vec<String> = planned
                    .keys()
                    .filter(|path| !git_paths.contains_key(*path))
                    .take(5)
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                bail!(
                    "release source inventory is stale against tracked/nonignored Git files (missing: [{}], extra: [{}])",
                    missing.join(", "),
                    extra.join(", ")
                );
            }
            verify_snapshot_inventory(root, &manifest)?;
            "source_controlled_manifest_git_verified"
        }
        None => {
            verify_snapshot_inventory(root, &manifest)?;
            ledger.push(ArgvLedgerEntry {
                source: "candidate_file_plan".into(),
                executable: "internal-source-inventory".into(),
                argv: vec![RELEASE_INVENTORY_PATH.into()],
                policy: "source_controlled_manifest_non_git".into(),
                attempted: true,
                outcome: "passed".into(),
            });
            "source_controlled_manifest_non_git"
        }
    };
    let attestation = SourceInventoryAttestation {
        schema: "loom.release-source-inventory-attestation/v1".into(),
        path: RELEASE_INVENTORY_PATH.into(),
        manifest_hash,
        inventory_hash,
        provenance: provenance.into(),
        git_verification: if provenance.ends_with("git_verified") {
            "verified".into()
        } else {
            "not_applicable".into()
        },
        git_influenced_plan: false,
        entry_count: manifest.files.len(),
        file_count: paths.len(),
        tombstone_count: manifest
            .files
            .iter()
            .filter(|entry| entry.mode == "absent")
            .count(),
        materialized_matches: true,
        missing: 0,
        unexpected: 0,
        secret: 0,
        symlink: 0,
        non_regular: 0,
        reserved: 0,
    };
    Ok((paths, attestation))
}

fn load_source_inventory(root: &Path) -> Result<(SourceInventoryManifest, String)> {
    let manifest_path = root.join(RELEASE_INVENTORY_PATH);
    let metadata = fs::symlink_metadata(&manifest_path).with_context(|| {
        format!("release source inventory is missing at '{RELEASE_INVENTORY_PATH}'")
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("release source inventory must be a regular non-symlink file");
    }
    let manifest: SourceInventoryManifest = serde_json::from_slice(&fs::read(&manifest_path)?)
        .context("release source inventory is malformed")?;
    validate_inventory_manifest(&manifest)?;
    let canonical = canonical_json(serde_json::to_value(&manifest)?);
    let manifest_hash = crate::artifact::fingerprint(&serde_json::to_string(&canonical)?);
    Ok((manifest, manifest_hash))
}
