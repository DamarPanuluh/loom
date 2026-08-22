use crate::journey::OperationArgument;
use crate::Result;
use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::observation::{ExecutableBoundary, ExecutionAnchors};
use super::types::{
    CompiledJourneyProof, EXECUTOR_PLATFORM_ENVIRONMENT, FAILURE_DIAGNOSTIC_BYTES,
    STREAM_EXCERPT_BYTES,
};
use super::values::{redact_json_secrets, redact_text, resolve_argv};

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_json_operation(
    repository_root: &Path,
    cwd: &Path,
    graph_root: &Path,
    policy: &crate::candidate_surface_policy::SurfacePlan,
    operation_id: &str,
    confinement: crate::candidate_surface_policy::ActualConfinement,
    env: &BTreeMap<String, String>,
    resolved_host_env: &BTreeMap<String, String>,
    declared_environment: &[String],
    base_argv: &[String],
    arguments: &[OperationArgument],
    timeout_seconds: u64,
    expected_exit: u32,
    inputs: &BTreeMap<String, Value>,
    captures: &BTreeMap<String, Value>,
    run_id: &str,
    secrets: &mut Vec<String>,
    label: &str,
    boundary: Option<&mut Vec<ExecutableBoundary>>,
) -> Result<(Vec<String>, i64, Value)> {
    let operation_env = operation_environment(env, resolved_host_env, declared_environment)?;
    let (argv, mut display_argv) =
        resolve_argv(base_argv, arguments, inputs, captures, run_id, secrets)?;
    let argv0 = argv[0].clone();
    let authorized = policy.authorize(operation_id, argv, confinement)?;
    if authorized.injects_graph() {
        display_argv.insert(1, graph_root.display().to_string());
        display_argv.insert(1, "--graph".into());
    }
    // Store-owned guarded runs resolve argv0 ONCE under the trusted execution
    // policy, hash the resolved executable before the spawn, and recheck the
    // same path and hash after execution while the harness guard is held.
    // Public API runs (which can never settle) keep the legacy resolver.
    let guarded = boundary.is_some();
    let resolved = if guarded {
        Some(
            resolve_trusted_executable(repository_root, &argv0).map_err(|error| {
                anyhow!(
                "{label} executable boundary: refusing to run an unapproved executable: {error:#}"
            )
            })?,
        )
    } else {
        None
    };
    let observed = run_direct(
        repository_root,
        cwd,
        graph_root,
        &operation_env,
        authorized,
        Duration::from_secs(timeout_seconds),
        resolved.as_ref(),
    )
    .with_context(|| format!("{label} could not start"))?;
    if let Some(resolved) = &resolved {
        // The same path, rechecked while the guard remains held: a missing,
        // replaced, or self-modifying executable must not be treated as the
        // Store-approved binary that ran.
        let current = std::fs::read(&resolved.path)
            .map(|bytes| crate::artifact::fingerprint_bytes(&bytes))
            .unwrap_or_default();
        if current != resolved.hash {
            bail!(
                "{label} executable '{}' was missing, replaced, or modified while it was \
                 running; refusing the result",
                resolved.path.display()
            );
        }
    }
    if observed.timed_out {
        bail!("{label} exceeded the execution timeout");
    }
    // Exit status is liveness, not an assertion: the observed exit must equal
    // the operation's expected_exit (zero by default), and a killed/signaled
    // process (no exit code at all) never satisfies it. A matching non-zero
    // exit proceeds exactly like exit 0: stdout must still be one UTF-8 JSON
    // value and the compiled captures/assertions still run against it.
    let observed_code = observed.status.code().map(i64::from);
    if observed_code != Some(i64::from(expected_exit)) {
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            observed.status.signal()
        };
        #[cfg(not(unix))]
        let signal = None;
        bail!(
            "{}",
            failed_operation_detail(
                label,
                observed_code.unwrap_or(-1),
                signal,
                &observed.stdout,
                &observed.stderr,
                secrets
            )
        );
    }
    let exit_code = i64::from(expected_exit);
    let stdout = std::str::from_utf8(&observed.stdout)
        .with_context(|| format!("{label} stdout is not UTF-8 JSON"))?;
    let output = serde_json::from_str(stdout)
        .with_context(|| format!("{label} stdout is not one JSON value"))?;
    if let Some(record) = boundary {
        // Pin only operations that produced an observed result. Recording
        // before the exit/JSON checks left a dangling boundary when the child
        // failed, so settlement compared compiled steps against a longer
        // recorded list and hid the real error.
        record.push(match &resolved {
            Some(resolved) => ExecutableBoundary {
                operation_id: operation_id.to_string(),
                declared: base_argv[0].clone(),
                argv0: argv0.clone(),
                resolved: resolved.path.to_string_lossy().into_owned(),
                hash: resolved.hash.clone(),
            },
            None => {
                resolve_executable_boundary(operation_id, &base_argv[0], &argv0, repository_root)
            }
        });
    }
    Ok((display_argv, exit_code, output))
}

fn failed_operation_detail(
    label: &str,
    exit_code: i64,
    signal: Option<i32>,
    stdout: &[u8],
    stderr: &[u8],
    secrets: &[String],
) -> String {
    let stdout = match serde_json::from_slice::<Value>(stdout) {
        Ok(mut structured) => {
            redact_json_secrets(&mut structured, secrets);
            serde_json::to_string_pretty(&structured)
                .unwrap_or_else(|_| redact_text(&String::from_utf8_lossy(stdout), secrets))
        }
        Err(_) => redact_text(&String::from_utf8_lossy(stdout), secrets),
    };
    let stderr = redact_text(&String::from_utf8_lossy(stderr), secrets);
    let status = match signal {
        Some(signal) => format!("{exit_code}, signal {signal}"),
        None => exit_code.to_string(),
    };
    format!(
        "{label} exited {status}\nstdout:\n{}\nstderr:\n{}",
        bounded_runtime_diagnostic(&stdout),
        bounded_runtime_diagnostic(&stderr),
    )
}

fn bounded_runtime_diagnostic(text: &str) -> String {
    if text.len() <= FAILURE_DIAGNOSTIC_BYTES {
        return text.trim().to_string();
    }
    let half = FAILURE_DIAGNOSTIC_BYTES / 2;
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

fn operation_environment(
    explicit: &BTreeMap<String, String>,
    resolved_host: &BTreeMap<String, String>,
    declared: &[String],
) -> Result<BTreeMap<String, String>> {
    let mut environment = explicit.clone();
    for name in declared {
        if environment.contains_key(name) {
            continue;
        }
        let value = resolved_host.get(name).ok_or_else(|| {
            anyhow!("declared operation environment variable '{name}' was not preflighted")
        })?;
        environment.insert(name.clone(), value.clone());
    }
    Ok(environment)
}

pub(crate) fn preflight_operation_environment(
    proof: &CompiledJourneyProof,
    explicit: &BTreeMap<String, String>,
    secrets: &mut Vec<String>,
) -> Result<BTreeMap<String, String>> {
    let mut declared = BTreeSet::new();
    if let Some(setup) = &proof.setup {
        for operation in &setup.operations {
            declared.extend(operation.environment.iter().cloned());
        }
    }
    for step in &proof.steps {
        declared.extend(step.environment.iter().cloned());
    }
    let mut resolved = BTreeMap::new();
    for name in declared {
        if explicit.contains_key(&name) {
            continue;
        }
        let value = std::env::var(&name).map_err(|error| match error {
            std::env::VarError::NotPresent => {
                anyhow!("declared operation environment variable '{name}' is missing")
            }
            std::env::VarError::NotUnicode(_) => {
                anyhow!("declared operation environment variable '{name}' is not valid UTF-8")
            }
        })?;
        secrets.push(value.clone());
        resolved.insert(name, value);
    }
    Ok(resolved)
}

pub(crate) struct TemporaryRoot(PathBuf);

impl TemporaryRoot {
    pub(crate) fn create(root: &Path) -> Result<Self> {
        let parent = root.join(".loom").join("tmp");
        std::fs::create_dir_all(&parent)?;
        for sequence in 0..1000_u32 {
            let path = parent.join(format!("journey-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not allocate a unique temporary Journey root")
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn persist_to(mut self, destination: &Path) -> Result<()> {
        if destination.exists() {
            bail!(
                "Journey continuation workspace '{}' already exists",
                destination.display()
            );
        }
        std::fs::rename(&self.0, destination).with_context(|| {
            format!(
                "persisting Journey workspace {} as {}",
                self.0.display(),
                destination.display()
            )
        })?;
        self.0 = PathBuf::new();
        Ok(())
    }

    pub(crate) fn adopt(path: PathBuf) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(&path).with_context(|| {
            format!("opening Journey continuation workspace {}", path.display())
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("Journey continuation workspace is not a confined directory");
        }
        Ok(Self(path))
    }

    pub(crate) fn create_detached(live_root: &Path) -> Result<Self> {
        Self::create_detached_with_prefix(live_root, "loom-journey-git")
    }

    pub(crate) fn create_gate_detached(live_root: &Path) -> Result<Self> {
        Self::create_detached_with_prefix(live_root, "loom-journey-gate")
    }

    fn create_detached_with_prefix(live_root: &Path, prefix: &str) -> Result<Self> {
        let parent = std::env::temp_dir();
        std::fs::create_dir_all(&parent)?;
        let canonical_parent = parent
            .canonicalize()
            .with_context(|| format!("canonicalizing system temp root {}", parent.display()))?;
        let canonical_live = live_root
            .canonicalize()
            .with_context(|| format!("canonicalizing live graph root {}", live_root.display()))?;
        if canonical_parent == canonical_live || canonical_parent.starts_with(&canonical_live) {
            bail!("system temp root must be outside the live repository");
        }
        for sequence in 0..1000_u32 {
            let path = canonical_parent.join(format!("{prefix}-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        bail!("could not allocate a detached temporary Journey root")
    }
}

impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

struct DirectObservation {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

fn run_direct(
    repository_root: &Path,
    cwd: &Path,
    graph_root: &Path,
    env: &BTreeMap<String, String>,
    invocation: crate::candidate_surface_policy::AuthorizedInvocation,
    timeout: Duration,
    resolved: Option<&ResolvedExecutable>,
) -> std::io::Result<DirectObservation> {
    let argv = invocation.into_graph_argv(graph_root);
    // On the Store-owned guarded path the executable was resolved once under
    // the trusted execution policy before this call; spawn that exact path,
    // never the unresolved token. The unguarded public API replicates the
    // legacy resolver for diagnostics only.
    let executable = match resolved {
        Some(resolved) => resolved.path.clone(),
        None => resolve_executable(repository_root, &argv[0]),
    };
    let mut command = Command::new(executable);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .env("LOOM_NON_INTERACTIVE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Preserve only the canonical executor-infrastructure allowlist needed to
    // resolve/spawn child tools. These names are distinct from authored
    // operation.environment declarations. CI, cloud credentials, tokens, and
    // arbitrary host variables remain absent.
    for &key in EXECUTOR_PLATFORM_ENVIRONMENT {
        if !env.contains_key(key) {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().map(read_stream);
    let stderr = child.stderr.take().map(read_stream);
    let started = Instant::now();
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if started.elapsed() >= timeout {
            kill_process_group(&mut child);
            break (child.wait()?, true);
        }
        thread::sleep(Duration::from_millis(10));
    };
    Ok(DirectObservation {
        status,
        stdout: stdout
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default(),
        stderr: stderr
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default(),
        timed_out,
    })
}

fn resolve_executable(root: &Path, executable: &str) -> PathBuf {
    let candidate = Path::new(executable);
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }
    if executable.contains('/') {
        return root.join(candidate);
    }
    if executable == "loom" {
        if let Ok(current) = std::env::current_exe() {
            if current
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "loom")
            {
                return current;
            }
        }
    }
    candidate.to_path_buf()
}

/// One operation's executable as resolved by the Store-derived trusted
/// execution policy: a canonical absolute path plus the content fingerprint
/// read immediately before the spawn. Both are pinned in the executed
/// boundary and re-derived at settlement; the runtime spawns exactly this
/// path and rechecks the same path's bytes after execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedExecutable {
    pub path: PathBuf,
    pub hash: String,
}

/// Approved toolchain directories a bare executable name may resolve in, in
/// probe order. This is the explicit approved toolchain/environment boundary:
/// a bare name is never resolved through the caller-mutated PATH, so a PATH
/// shim cannot redirect the spawn. Only the directories the toolchain itself
/// ships in are trusted; anything else must be declared as a confined relative
/// path under the repository or an explicit absolute identity.
#[cfg(unix)]
const APPROVED_TOOLCHAIN_DIRECTORIES: &[&str] = &[
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/opt/homebrew/bin",
    "/opt/local/bin",
];

/// Windows: the Loom toolchain may live beside the running binary, and the
/// platform shell essentials live under the system root.
#[cfg(windows)]
fn approved_toolchain_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        directories.push(PathBuf::from(system_root).join("System32"));
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(parent) = current.parent() {
            directories.push(parent.to_path_buf());
        }
    }
    directories
}

/// Resolve one declared executable token under the Store-derived trusted
/// execution policy, hashing its bytes immediately before execution.
///
/// - Literal paths: canonicalized once; a RELATIVE literal path must resolve
///   to a real file beneath the canonical repository root — a symlink escape
///   is refused even when the link itself sits inside the root. Absolute
///   literals are the caller's explicit identity and are used as-is.
/// - Bare names: `loom` binds to the currently running Loom binary (the
///   allowlisted absolute identity of the toolchain itself); any other bare
///   name must exist in an approved toolchain directory. Caller-mutated PATH
///   is never consulted.
///
/// Missing, unreadable, or — for bare names — unapproved executables are
/// refused before anything spawns.
pub(crate) fn resolve_trusted_executable(
    root: &Path,
    declared: &str,
) -> Result<ResolvedExecutable> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey repository root {}", root.display()))?;
    let token = Path::new(declared);
    let candidate: PathBuf = if token.is_absolute() {
        token.to_path_buf()
    } else if declared.contains('/') {
        canonical_root.join(token)
    } else {
        resolve_approved_bare(declared)?
    };
    let canonical = candidate.canonicalize().with_context(|| {
        format!(
            "resolving executable '{declared}': the declared executable does not resolve to a \
             real file under the trusted execution policy"
        )
    })?;
    if !token.is_absolute() && declared.contains('/') && !canonical.starts_with(&canonical_root) {
        bail!(
            "executable '{declared}' resolves through a symlink to '{}' outside the repository \
             root; refusing to execute it (symlink escapes are not approved)",
            canonical.display()
        );
    }
    let bytes = std::fs::read(&canonical)
        .with_context(|| format!("reading resolved executable '{}'", canonical.display()))?;
    let hash = crate::artifact::fingerprint_bytes(&bytes);
    Ok(ResolvedExecutable {
        path: canonical,
        hash,
    })
}

/// Resolve a bare executable name (no path separator) through the approved
/// toolchain/environment boundary. `loom` binds to the currently running Loom
/// binary — the allowlisted absolute identity of the toolchain. Any other
/// bare name must already exist in an approved toolchain directory; the
/// caller's PATH is never consulted, so a PATH shim cannot redirect the
/// spawn.
fn resolve_approved_bare(declared: &str) -> Result<PathBuf> {
    if declared == "loom" {
        if let Ok(current) = std::env::current_exe() {
            if current
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "loom")
            {
                return Ok(current);
            }
        }
        bail!(
            "bare executable 'loom' cannot be bound to the approved Loom toolchain identity: the \
             running process is not a Loom binary"
        );
    }
    #[cfg(unix)]
    let directories = APPROVED_TOOLCHAIN_DIRECTORIES
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    #[cfg(windows)]
    let directories = approved_toolchain_directories();
    for directory in directories {
        let probe = directory.join(declared);
        if probe.is_file() {
            return Ok(probe);
        }
    }
    bail!(
        "bare executable '{declared}' is not approved: it is absent from the approved toolchain \
         directories and is not the Loom toolchain binary itself; declare it as a confined \
         relative path under the repository (e.g. tools/{declared}) or an explicit absolute \
         identity"
    )
}

/// Capture the execution-time anchors before anything may execute: the
/// canonical execution root and the covered-file hashes in force now.
/// A covered file that cannot be read before execution refuses the run —
/// settlement evidence must bind real files, not absent ones.
pub(crate) fn capture_execution_anchors(
    root: &Path,
    covered_files: &[String],
) -> Result<ExecutionAnchors> {
    let execution_root = root
        .canonicalize()
        .with_context(|| format!("canonicalizing Journey repository root {}", root.display()))?;
    let mut covered_hashes = BTreeMap::new();
    for file in covered_files {
        let content = std::fs::read_to_string(root.join(file)).with_context(|| {
            format!(
                "covered CodeFile '{}' is not readable before Journey execution",
                file
            )
        })?;
        covered_hashes.insert(file.clone(), crate::artifact::fingerprint(&content));
    }
    Ok(ExecutionAnchors {
        covered_hashes,
        execution_root,
        executed_boundary: Vec::new(),
    })
}

/// Pin the executable boundary of one spawned operation: the resolved
/// executable path and its content fingerprint at execution time. Bare
/// names replicate the executor's PATH search so the record names the
/// actual binary; root-relative paths resolve against the repository root
/// exactly like the spawn itself.
fn resolve_executable_boundary(
    operation_id: &str,
    declared: &str,
    argv0: &str,
    root: &Path,
) -> ExecutableBoundary {
    let candidate = resolve_executable(root, argv0);
    let located: PathBuf = if candidate.is_absolute() || argv0.contains('/') {
        candidate
    } else {
        let mut found = None;
        if let Some(path) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&path) {
                let probe = directory.join(&candidate);
                if probe.is_file() {
                    found = Some(probe);
                    break;
                }
            }
        }
        found.unwrap_or(candidate)
    };
    let resolved = located
        .canonicalize()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| located.to_string_lossy().into_owned());
    let hash = std::fs::read(&located)
        .map(|bytes| crate::artifact::fingerprint_bytes(&bytes))
        .unwrap_or_default();
    ExecutableBoundary {
        operation_id: operation_id.to_string(),
        declared: declared.to_string(),
        argv0: argv0.to_string(),
        resolved,
        hash,
    }
}

fn read_stream(mut stream: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut retained = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let room = STREAM_EXCERPT_BYTES.saturating_sub(retained.len());
                    retained.extend_from_slice(&buffer[..read.min(room)]);
                }
            }
        }
        retained
    })
}

fn kill_process_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}
