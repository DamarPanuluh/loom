fn validate_candidate_path(path: &Path) -> Result<()> {
    use std::path::Component;

    if path.as_os_str().is_empty() || path.is_absolute() {
        bail!("release candidate path must be a confined relative path");
    }
    for component in path.components() {
        let Component::Normal(component) = component else {
            bail!(
                "release candidate path '{}' is not confined",
                path.display()
            );
        };
        let component = component
            .to_str()
            .ok_or_else(|| anyhow!("release candidate path is not UTF-8"))?;
        if INVENTORY_RESERVED_COMPONENTS.contains(&component) {
            bail!(
                "release candidate path '{}' enters reserved component '{}'",
                path.display(),
                component
            );
        }
    }
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let secret = name == ".env"
        || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || matches!(
            name.as_str(),
            "id_rsa"
                | "id_ed25519"
                | "credentials"
                | "credentials.json"
                | "secrets"
                | "secrets.json"
                | ".npmrc"
                | ".pypirc"
                | ".netrc"
        );
    if secret {
        bail!(
            "release candidate refuses secret-bearing path '{}'",
            path.display()
        );
    }
    Ok(())
}

impl LiveState {
    fn capture(root: &Path) -> Result<Self> {
        let git_dir = git_directory(root)?;
        Ok(Self {
            source: hash_tree(root, &SOURCE_EXCLUDES)?,
            // The cooperative holder record is ephemeral coordination, not
            // graph truth. Read-only Store access refreshes it by design; the
            // rehearsal compares every durable graph byte around it.
            graph: hash_optional_path_excluding(&root.join(crate::LOOM_DIR), &["lock"])?,
            target: hash_optional_path(&root.join("target"))?,
            git: hash_optional_path(&root.join(".git"))?,
            git_head: git_head_hash(git_dir.as_deref())?,
            git_index: git_dir
                .as_deref()
                .map(|directory| hash_optional_path(&directory.join("index")))
                .transpose()?
                .unwrap_or_else(|| "absent".into()),
            git_remotes: git_dir
                .as_deref()
                .map(|directory| hash_optional_path(&directory.join("config")))
                .transpose()?
                .unwrap_or_else(|| "absent".into()),
            installed_binary: installed_loom_hash()?,
        })
    }

    fn compare(&self, after: &Self, ledger: &[ArgvLedgerEntry]) -> EffectAttestation {
        let live_source_changed = self.source != after.source;
        let live_graph_changed = self.graph != after.graph;
        let live_target_changed = self.target != after.target;
        let live_git_changed = self.git != after.git;
        let live_git_head_changed = self.git_head != after.git_head;
        let live_git_index_changed = self.git_index != after.git_index;
        let live_git_remotes_changed = self.git_remotes != after.git_remotes;
        let installed_binary_changed = self.installed_binary != after.installed_binary;
        let mut release_paths_changed = Vec::new();
        for (changed, label) in [
            (live_source_changed, "caller_source"),
            (live_graph_changed, "caller_graph"),
            (live_target_changed, "caller_target"),
            (live_git_changed, "caller_git"),
            (live_git_head_changed, "caller_git_head"),
            (live_git_index_changed, "caller_git_index"),
            (live_git_remotes_changed, "caller_git_remotes"),
            (installed_binary_changed, "installed_loom"),
        ] {
            if changed {
                release_paths_changed.push(label.into());
            }
        }
        let attempted = |executable: &str, command: &str| {
            ledger.iter().any(|entry| {
                entry.attempted
                    && Path::new(&entry.executable)
                        .file_name()
                        .and_then(OsStr::to_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(executable))
                    && entry.argv.first().is_some_and(|arg| arg == command)
            })
        };
        EffectAttestation {
            live_source_changed,
            live_graph_changed,
            live_target_changed,
            live_git_changed,
            live_git_head_changed,
            live_git_index_changed,
            live_git_remotes_changed,
            installed_binary_changed,
            release_paths_changed,
            argv_attempt_scope:
                "direct top-level argv ledger only; descendant process containment is not claimed"
                    .into(),
            top_level_install_argv_attempted: attempted("cargo", "install"),
            top_level_commit_argv_attempted: attempted("git", "commit"),
            top_level_push_argv_attempted: attempted("git", "push"),
        }
    }
}

fn git_directory(root: &Path) -> Result<Option<PathBuf>> {
    let dot_git = root.join(".git");
    if !dot_git.exists() {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(&dot_git)?;
    if metadata.file_type().is_symlink() {
        bail!("release rehearsal refuses a symlinked caller Git directory");
    }
    if metadata.is_dir() {
        return Ok(Some(dot_git));
    }
    let pointer = fs::read_to_string(&dot_git)?;
    let relative = pointer
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| anyhow!("caller .git file has no gitdir pointer"))?;
    let path = PathBuf::from(relative);
    let resolved = if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
    .canonicalize()?;
    Ok(Some(resolved))
}

fn git_head_hash(git_dir: Option<&Path>) -> Result<String> {
    let Some(git_dir) = git_dir else {
        return Ok("absent".into());
    };
    let head_path = git_dir.join("HEAD");
    if !head_path.exists() {
        return Ok("absent".into());
    }
    let head = fs::read(&head_path)?;
    let mut projection = BTreeMap::new();
    projection.insert("HEAD", fingerprint_bytes(&head));
    if let Ok(text) = std::str::from_utf8(&head) {
        if let Some(reference) = text.trim().strip_prefix("ref: ") {
            projection.insert("referent", hash_optional_path(&git_dir.join(reference))?);
            projection.insert(
                "packed_refs",
                hash_optional_path(&git_dir.join("packed-refs"))?,
            );
        }
    }
    Ok(fingerprint_bytes(&serde_json::to_vec(&projection)?))
}

fn installed_loom_hash() -> Result<String> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    match cargo_home {
        Some(home) => hash_optional_path(&home.join("bin/loom")),
        None => Ok("unavailable".into()),
    }
}

fn hash_optional_path(path: &Path) -> Result<String> {
    hash_optional_path_excluding(path, &[])
}

fn hash_optional_path_excluding(path: &Path, top_level_excludes: &[&str]) -> Result<String> {
    if !path.exists() {
        return Ok("absent".into());
    }
    hash_tree(path, top_level_excludes)
}

/// Streaming FNV-1a with exactly the schedule [`fingerprint_bytes`] uses, so a
/// streamed tree hash equals hashing the same byte sequence in one buffer.
///
/// Buffering was not viable: a declared cache root is a real toolchain cache
/// (`~/.rustup` alone is gigabytes across six figures of files), and holding
/// every byte of it in one `Vec` to hash it at the end got the rehearsal
/// SIGKILLed before it could emit a single line of its report.
struct TreeHasher(u64);

impl TreeHasher {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(self) -> String {
        format!("{:016x}", self.0)
    }
}

fn hash_tree(root: &Path, top_level_excludes: &[&str]) -> Result<String> {
    let mut hasher = TreeHasher::new();
    hash_tree_into(root, root, top_level_excludes, &mut hasher)?;
    Ok(hasher.finish())
}

fn hash_tree_into(
    root: &Path,
    path: &Path,
    top_level_excludes: &[&str],
    out: &mut TreeHasher,
) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let relative = path.strip_prefix(root).unwrap_or(path);
    if metadata.file_type().is_symlink() {
        out.write(b"link\0");
        out.write(relative.to_string_lossy().as_bytes());
        out.write(&[0]);
        out.write(fs::read_link(path)?.to_string_lossy().as_bytes());
        return Ok(());
    }
    if metadata.is_file() {
        out.write(b"file\0");
        out.write(relative.to_string_lossy().as_bytes());
        out.write(&[0]);
        // Stream the contents: one cache root can hold gigabytes, and the
        // hash only needs the bytes in order, never all of them at once.
        let mut file = fs::File::open(path)?;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            out.write(&buffer[..read]);
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        out.write(b"special\0");
        out.write(relative.to_string_lossy().as_bytes());
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(path)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if path == root
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| top_level_excludes.contains(&name))
        {
            continue;
        }
        hash_tree_into(root, &entry.path(), top_level_excludes, out)?;
    }
    Ok(())
}

fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn canonical_json(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical_json).collect())
        }
        serde_json::Value::Object(object) => {
            let sorted: BTreeMap<String, serde_json::Value> = object
                .into_iter()
                .map(|(key, value)| (key, canonical_json(value)))
                .collect();
            serde_json::to_value(sorted).expect("canonical JSON map serializes")
        }
        scalar => scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_authority_handoff_contains_sealed_token_and_resolved_root() {
        let root = Path::new("/tmp/resolved-loom-root");
        let token = "rda1_actual-sealed-token";
        assert_eq!(
            derivation_authority_next_command(token, root).unwrap(),
            "LOOM_RELEASE_DERIVATION_AUTHORITY=rda1_actual-sealed-token loom journey run release-workflow --profile proof --graph '/tmp/resolved-loom-root'"
        );
        assert_eq!(
            derivation_authority_next_command(token, Path::new("/tmp/release candidate's graph"))
                .unwrap(),
            "LOOM_RELEASE_DERIVATION_AUTHORITY=rda1_actual-sealed-token loom journey run release-workflow --profile proof --graph '/tmp/release candidate'\\''s graph'"
        );
    }

    fn isolated_live_root(label: &str) -> DetachedCandidate {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        for sequence in 0..1000_u32 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default();
            let path = parent.join(format!(
                "loom-release-live-{label}-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return DetachedCandidate(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating isolated release fixture: {error}"),
            }
        }
        panic!("could not allocate isolated release fixture")
    }

    fn write_test_inventory(root: &Path, declared: &[(&str, &str)]) {
        let mut files = declared
            .iter()
            .map(|(path, mode)| SourceInventoryEntry {
                path: (*path).into(),
                mode: (*mode).into(),
            })
            .collect::<Vec<_>>();
        files.push(SourceInventoryEntry {
            path: RELEASE_INVENTORY_PATH.into(),
            mode: "regular".into(),
        });
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = SourceInventoryManifest {
            schema: "loom.release-inventory/v3".into(),
            code_gates: RELEASE_CODE_GATES
                .iter()
                .map(|gate| gate.iter().map(|token| (*token).into()).collect())
                .collect(),
            cache_root_environment: RELEASE_CACHE_ROOT_ENVIRONMENT
                .iter()
                .map(|name| (*name).into())
                .collect(),
            files,
            reserved_components: INVENTORY_RESERVED_COMPONENTS
                .iter()
                .map(ToString::to_string)
                .collect(),
            secret_name_patterns: INVENTORY_SECRET_PATTERNS
                .iter()
                .map(ToString::to_string)
                .collect(),
        };
        fs::create_dir_all(root.join("release")).unwrap();
        fs::write(
            root.join(RELEASE_INVENTORY_PATH),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn cold_rehearsal_files_are_exact_inventory_members_before_allocation() {
        let root = isolated_live_root("cold-paths");
        fs::create_dir_all(root.path().join("journeys/surfaces")).unwrap();
        fs::write(root.path().join("journeys/known.yaml"), b"known").unwrap();
        write_test_inventory(
            root.path(),
            &[
                ("journeys/known.yaml", "regular"),
                ("journeys/missing.yaml", "absent"),
            ],
        );
        let (inventory, _) = load_source_inventory(root.path()).unwrap();
        assert_eq!(
            confined_inventory_file(root.path(), &inventory, "journeys/known.yaml").unwrap(),
            root.path()
                .join("journeys/known.yaml")
                .canonicalize()
                .unwrap()
        );
        for rejected in [
            "journeys/foreign.yaml",
            "journeys/missing.yaml",
            "journeys/../journeys/known.yaml",
            "/tmp/foreign.yaml",
        ] {
            assert!(confined_inventory_file(root.path(), &inventory, rejected).is_err());
        }
    }

    #[test]
    fn release_inventory_v3_rejects_gate_or_cache_policy_drift() {
        let root = isolated_live_root("inventory-policy");
        fs::write(root.path().join("Cargo.toml"), b"[package]\nname='x'\n").unwrap();
        write_test_inventory(root.path(), &[("Cargo.toml", "regular")]);
        let (manifest, _) = load_source_inventory(root.path()).unwrap();

        let mut manifest_path = manifest.clone();
        manifest_path.code_gates[3]
            .extend(["--manifest-path".into(), "/tmp/foreign/Cargo.toml".into()]);
        assert!(validate_inventory_manifest(&manifest_path).is_err());

        let mut missing_gate = manifest.clone();
        missing_gate.code_gates.pop();
        assert!(validate_inventory_manifest(&missing_gate).is_err());

        let mut missing_cache = manifest;
        missing_cache.cache_root_environment.pop();
        assert!(validate_inventory_manifest(&missing_cache).is_err());
    }

    #[test]
    fn copy_refuses_nonempty_and_excludes_reserved_roots() {
        let root = isolated_live_root("source");
        fs::write(root.path().join("Cargo.toml"), b"[package]\nname='x'\n").unwrap();
        fs::write(root.path().join(".gitignore"), b".loom/\ntarget/\n").unwrap();
        write_test_inventory(
            root.path(),
            &[(".gitignore", "regular"), ("Cargo.toml", "regular")],
        );
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".git/config"), b"secret").unwrap();
        fs::create_dir(root.path().join(".loom")).unwrap();
        fs::write(root.path().join(".loom/graph.sqlite"), b"graph").unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        fs::write(root.path().join("target/output"), b"build").unwrap();
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .unwrap()
            .success());

        let destination = DetachedCandidate::allocate(root.path(), "destination").unwrap();
        let mut ledger = Vec::new();
        copy_candidate(root.path(), destination.path(), &mut ledger).unwrap();
        assert!(destination.path().join("Cargo.toml").is_file());
        for excluded in SOURCE_EXCLUDES {
            assert!(!destination.path().join(excluded).exists());
        }

        fs::write(destination.path().join("occupied"), b"x").unwrap();
        assert!(copy_candidate(root.path(), destination.path(), &mut ledger).is_err());
        assert!(ledger
            .iter()
            .any(|entry| entry.source == "candidate_file_plan" && entry.attempted));
    }

    #[test]
    fn imported_surface_requires_an_independent_candidate_manifest() {
        let root = isolated_live_root("manifest-gap");
        let surface = Node {
            id: "surface-id".into(),
            node_type: NodeType::InterfaceSurface,
            name: "candidate-cli".into(),
            description: String::new(),
            status: "quarantined".into(),
            truth_class: crate::model::TruthClass::Asserted,
            body: serde_json::json!({"operations": [{"argv": ["candidate"]}]}),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let mut ledger = Vec::new();
        let error = candidate_manifest_attestations(
            root.path(),
            &[surface],
            &missing_outer_attestation(),
            &mut ledger,
        )
        .unwrap_err();
        assert!(error.to_string().contains("remain quarantined"));
    }

    #[test]
    fn candidate_plan_refuses_secret_and_nested_reserved_paths() {
        for relative in ["nested/.env.production", "nested/target/payload"] {
            let root = isolated_live_root("secret-source");
            fs::create_dir_all(root.path().join(relative).parent().unwrap()).unwrap();
            fs::write(root.path().join(relative), b"secret").unwrap();
            write_test_inventory(root.path(), &[]);
            assert!(Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(root.path())
                .status()
                .unwrap()
                .success());
            let destination =
                DetachedCandidate::allocate(root.path(), "secret-destination").unwrap();
            let error = copy_candidate(root.path(), destination.path(), &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("secret-bearing") || error.contains("reserved component"),
                "{error}"
            );
        }
    }

    #[test]
    fn linked_worktree_head_change_blocks_even_when_git_pointer_bytes_do_not_change() {
        let root = isolated_live_root("linked-worktree");
        let git = DetachedCandidate::allocate(root.path(), "linked-gitdir").unwrap();
        fs::create_dir_all(git.path().join("refs/heads")).unwrap();
        fs::write(git.path().join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(git.path().join("refs/heads/main"), b"first\n").unwrap();
        fs::write(git.path().join("index"), b"index").unwrap();
        fs::write(git.path().join("config"), b"[remote \"origin\"]\n").unwrap();
        fs::write(
            root.path().join(".git"),
            format!("gitdir: {}\n", git.path().display()),
        )
        .unwrap();

        let before = LiveState::capture(root.path()).unwrap();
        fs::write(git.path().join("refs/heads/main"), b"second\n").unwrap();
        let after = LiveState::capture(root.path()).unwrap();
        let effects = before.compare(&after, &[]);
        assert!(!effects.live_git_changed, "the .git pointer file is stable");
        assert!(effects.live_git_head_changed);
        assert!(!effects.live_git_index_changed);
        assert!(!effects.live_git_remotes_changed);
        assert!(caller_state_changed(&effects));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_git_pointer_is_rejected_before_state_comparison() {
        use std::os::unix::fs::symlink;

        let root = isolated_live_root("symlink-worktree");
        let git = DetachedCandidate::allocate(root.path(), "symlink-gitdir").unwrap();
        symlink(git.path(), root.path().join(".git")).unwrap();
        assert!(git_directory(root.path())
            .unwrap_err()
            .to_string()
            .contains("symlinked caller Git directory"));
    }
}
