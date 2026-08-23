fn validate_reserved_roots(root: &Path) -> Result<()> {
    for name in INVENTORY_RESERVED_COMPONENTS {
        let path = root.join(name);
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            bail!("release snapshot refuses symlinked reserved root '{name}'");
        }
        if name != ".git" && !metadata.is_dir() {
            bail!("release snapshot reserved root '{name}' must be a directory");
        }
    }
    Ok(())
}

fn validate_inventory_manifest(manifest: &SourceInventoryManifest) -> Result<()> {
    if manifest.schema != "loom.release-inventory/v3" {
        bail!(
            "release source inventory has unsupported schema '{}'",
            manifest.schema
        );
    }
    if manifest.code_gates.is_empty() || manifest.cache_root_environment.is_empty() {
        bail!("release source inventory must declare code gates and cache roots");
    }
    if manifest.code_gates.len() != RELEASE_CODE_GATES.len()
        || !manifest
            .code_gates
            .iter()
            .zip(RELEASE_CODE_GATES)
            .all(|(actual, expected)| {
                actual
                    .iter()
                    .map(String::as_str)
                    .eq(expected.iter().copied())
            })
    {
        bail!("release source inventory v3 must declare the exact ordered 0.30 code gates");
    }
    if !manifest
        .cache_root_environment
        .iter()
        .map(String::as_str)
        .eq(RELEASE_CACHE_ROOT_ENVIRONMENT.iter().copied())
    {
        bail!("release source inventory v3 must attest the exact 0.30 cache roots");
    }
    let mut unique_gates = BTreeSet::new();
    for gate in &manifest.code_gates {
        if gate.is_empty()
            || gate
                .iter()
                .any(|token| token.is_empty() || token.contains('\0'))
        {
            bail!("release source inventory code gates contain an empty or NUL token");
        }
        if gate[0] != "cargo" {
            bail!("release source inventory v3 code gates must use exact bare argv0 'cargo'");
        }
        inspect_process_argv(Path::new("cargo"), &gate[1..])?;
        if !unique_gates.insert(gate.clone()) {
            bail!("release source inventory code gates must not repeat exact argv arrays");
        }
    }
    let mut prior_env: Option<&str> = None;
    for name in &manifest.cache_root_environment {
        if name.is_empty()
            || name.contains('\0')
            || !name
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
            || name.as_bytes()[0].is_ascii_digit()
            || prior_env.is_some_and(|prior| prior >= name.as_str())
        {
            bail!("release cache root environment names must be valid, unique, and sorted");
        }
        prior_env = Some(name);
    }
    let expected_reserved: Vec<String> = INVENTORY_RESERVED_COMPONENTS
        .iter()
        .map(ToString::to_string)
        .collect();
    let expected_secrets: Vec<String> = INVENTORY_SECRET_PATTERNS
        .iter()
        .map(ToString::to_string)
        .collect();
    if manifest.reserved_components != expected_reserved
        || manifest.secret_name_patterns != expected_secrets
    {
        bail!("release source inventory weakens the canonical reserved/secret policy");
    }
    if manifest.files.is_empty() {
        bail!("release source inventory must declare files");
    }
    let mut prior: Option<&str> = None;
    let mut has_self = false;
    for entry in &manifest.files {
        validate_candidate_path(Path::new(&entry.path))?;
        if !matches!(entry.mode.as_str(), "regular" | "executable" | "absent") {
            bail!(
                "release source inventory path '{}' has invalid mode",
                entry.path
            );
        }
        if prior.is_some_and(|previous| previous >= entry.path.as_str()) {
            bail!("release source inventory files must be unique and sorted");
        }
        has_self |= entry.path == RELEASE_INVENTORY_PATH;
        prior = Some(&entry.path);
    }
    if !has_self {
        bail!("release source inventory must include its own manifest entry");
    }
    Ok(())
}

fn expand_inventory(root: &Path, manifest: &SourceInventoryManifest) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for entry in &manifest.files {
        let relative = Path::new(&entry.path);
        if entry.mode == "absent" {
            validate_candidate_path(relative)?;
            match fs::symlink_metadata(root.join(relative)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
                Ok(_) => bail!(
                    "release inventory tombstone '{}' is present",
                    relative.display()
                ),
            }
        } else {
            insert_inventory_file(root, relative, &entry.mode, &mut paths)?;
        }
    }
    if paths.is_empty() {
        bail!("release source inventory expands to no files");
    }
    Ok(paths.into_iter().collect())
}

fn insert_inventory_file(
    root: &Path,
    relative: &Path,
    expected_mode: &str,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    validate_candidate_path(relative)?;
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("release inventory file '{}' is missing", relative.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "release inventory file '{}' must be regular",
            relative.display()
        );
    }
    if normalized_mode(&metadata) != expected_mode {
        bail!(
            "release inventory file '{}' mode is stale",
            relative.display()
        );
    }
    paths.insert(relative.to_path_buf());
    Ok(())
}

fn normalized_mode(metadata: &fs::Metadata) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            return "executable";
        }
    }
    "regular"
}

fn open_inventory_file(path: &Path) -> Result<(fs::File, fs::Metadata)> {
    let before = fs::symlink_metadata(path)?;
    if before.file_type().is_symlink() || !before.is_file() {
        bail!(
            "release inventory refuses non-regular source '{}'",
            path.display()
        );
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("opening no-follow inventory source '{}'", path.display()))?;
    let opened = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            bail!(
                "release inventory source changed while opening '{}'",
                path.display()
            );
        }
    }
    Ok((file, opened))
}

fn set_inventory_permissions(path: &Path, mode: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if mode == "executable" { 0o755 } else { 0o644 }),
        )?;
    }
    Ok(())
}

fn inventory_content_hash(root: &Path, entries: &[SourceInventoryEntry]) -> Result<String> {
    let mut bytes = Vec::new();
    for entry in entries {
        let mut content = Vec::new();
        if entry.mode != "absent" {
            let (mut file, _) = open_inventory_file(&root.join(&entry.path))?;
            file.read_to_end(&mut content)?;
        }
        bytes.extend_from_slice(entry.path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(entry.mode.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(content.len().to_string().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&content);
        bytes.push(0xff);
    }
    Ok(crate::artifact::fingerprint_bytes(&bytes))
}

fn git_sandboxed_output(
    sandbox: &DetachedCandidate,
    argv: &[String],
) -> Result<std::process::Output> {
    let mut command = Command::new("git");
    command.args(argv).env_clear();
    for key in ["PATH", "SYSTEMROOT", "WINDIR", "PATHEXT", "COMSPEC"] {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    command
        .env("HOME", sandbox.path())
        .env("XDG_CONFIG_HOME", sandbox.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("running sandboxed git inventory command")
}

fn record_git_inventory(
    ledger: &mut Vec<ArgvLedgerEntry>,
    source: &str,
    argv: Vec<String>,
    output: &std::process::Output,
) {
    ledger.push(ArgvLedgerEntry {
        source: source.into(),
        executable: "git".into(),
        argv,
        policy: "read_only_git_inventory".into(),
        attempted: true,
        outcome: if output.status.success() {
            "passed".into()
        } else {
            format!("failed_exit_{}", output.status.code().unwrap_or(-1))
        },
    });
}

fn git_candidate_paths(
    root: &Path,
    ledger: &mut Vec<ArgvLedgerEntry>,
) -> Result<Option<BTreeMap<PathBuf, String>>> {
    if !root.join(".git").exists() {
        return Ok(None);
    }
    let sandbox = DetachedCandidate::allocate(root, "git-inventory-sandbox")?;
    let argv = vec![
        "-C".into(),
        root.to_string_lossy().into_owned(),
        "ls-files".into(),
        "-z".into(),
        "--cached".into(),
        "--others".into(),
        "--exclude-standard".into(),
    ];
    let output = git_sandboxed_output(&sandbox, &argv)?;
    record_git_inventory(ledger, "candidate_file_plan", argv, &output);
    if !output.status.success() {
        bail!(
            "release candidate requires a readable Git worktree inventory: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let flags_argv = vec![
        "-C".into(),
        root.to_string_lossy().into_owned(),
        "ls-files".into(),
        "-z".into(),
        "-v".into(),
        "--cached".into(),
    ];
    let flags_output = git_sandboxed_output(&sandbox, &flags_argv)?;
    record_git_inventory(
        ledger,
        "candidate_file_plan:index_flags",
        flags_argv,
        &flags_output,
    );
    if !flags_output.status.success() {
        bail!("release candidate cannot verify Git index flags");
    }
    for raw in flags_output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        if raw.len() < 3 || raw[0] != b'H' || raw[1] != b' ' {
            let path =
                std::str::from_utf8(raw.get(2..).unwrap_or_default()).unwrap_or("<non-utf8>");
            bail!(
                "release candidate refuses sparse, assume-unchanged, or exceptional Git index path '{}'",
                path
            );
        }
    }
    let mut candidate_paths = BTreeSet::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let text =
            std::str::from_utf8(raw).context("Git file inventory contains a non-UTF-8 path")?;
        let relative = PathBuf::from(text);
        validate_candidate_path(&relative)?;
        candidate_paths.insert(relative);
    }

    // The index no longer names staged deletions or the old side of staged
    // renames. Union the committed tree so those paths must appear as explicit
    // `absent` tombstones instead of disappearing from the candidate identity.
    let head_argv = vec![
        "-C".into(),
        root.to_string_lossy().into_owned(),
        "ls-tree".into(),
        "-r".into(),
        "-z".into(),
        "--name-only".into(),
        "HEAD".into(),
    ];
    let head_output = git_sandboxed_output(&sandbox, &head_argv)?;
    ledger.push(ArgvLedgerEntry {
        source: "candidate_file_plan:committed_tree".into(),
        executable: "git".into(),
        argv: head_argv,
        policy: "read_only_git_inventory".into(),
        attempted: true,
        outcome: if head_output.status.success() {
            "passed".into()
        } else {
            "unborn_head".into()
        },
    });
    if head_output.status.success() {
        for raw in head_output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|raw| !raw.is_empty())
        {
            let text = std::str::from_utf8(raw)
                .context("committed Git inventory contains a non-UTF-8 path")?;
            let relative = PathBuf::from(text);
            validate_candidate_path(&relative)?;
            candidate_paths.insert(relative);
        }
    }

    let mut paths = BTreeMap::new();
    for relative in candidate_paths {
        let text = relative.to_string_lossy();
        let absolute = root.join(&relative);
        match fs::symlink_metadata(&absolute) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    bail!("release candidate refuses non-regular path '{}'", text);
                }
                paths.insert(relative, normalized_mode(&metadata).into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                paths.insert(relative, "absent".into());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting candidate path '{}'", text));
            }
        }
    }
    if paths.is_empty() {
        bail!("release candidate Git file plan is empty");
    }
    Ok(Some(paths))
}

fn verify_snapshot_inventory(root: &Path, manifest: &SourceInventoryManifest) -> Result<()> {
    let planned: BTreeMap<PathBuf, String> = manifest
        .files
        .iter()
        .filter(|entry| entry.mode != "absent")
        .map(|entry| (PathBuf::from(&entry.path), entry.mode.clone()))
        .collect();
    let observed = scan_snapshot_files(root)?;
    if planned != observed {
        let missing: Vec<String> = observed
            .keys()
            .filter(|path| !planned.contains_key(*path))
            .take(5)
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let extra: Vec<String> = planned
            .keys()
            .filter(|path| !observed.contains_key(*path))
            .take(5)
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        bail!(
            "release source tree differs from its explicit inventory (undeclared: [{}], missing: [{}])",
            missing.join(", "),
            extra.join(", ")
        );
    }
    Ok(())
}

fn scan_snapshot_files(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    fn walk(
        root: &Path,
        directory: &Path,
        depth: usize,
        out: &mut BTreeMap<PathBuf, String>,
    ) -> Result<()> {
        let mut entries: Vec<_> = fs::read_dir(directory)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(root)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| anyhow!("release snapshot contains non-UTF-8 path"))?;
            let metadata = fs::symlink_metadata(&path)?;
            if INVENTORY_RESERVED_COMPONENTS.contains(&name.as_str()) {
                if metadata.file_type().is_symlink() || !metadata.is_dir() || depth > 0 {
                    bail!(
                        "release snapshot contains unsafe reserved path '{}'",
                        relative.display()
                    );
                }
                continue;
            }
            if metadata.file_type().is_symlink() {
                bail!("release snapshot refuses symlink '{}'", relative.display());
            }
            if metadata.is_dir() {
                walk(root, &path, depth + 1, out)?;
            } else if metadata.is_file() {
                validate_candidate_path(relative)?;
                out.insert(relative.to_path_buf(), normalized_mode(&metadata).into());
            } else {
                bail!(
                    "release snapshot refuses non-regular path '{}'",
                    relative.display()
                );
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    walk(root, root, 0, &mut files)?;
    Ok(files)
}
