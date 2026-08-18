//! The one hardened way loom runs an external command and watches it.
//!
//! Plane: observation support. Every runner (`runner`, `proof`, `scan`,
//! `journey`) spawns a shell command and waits with a timeout. The traps are
//! the same each time, so they are solved once here:
//!
//!   1. **The whole process group dies on timeout.** `sh -c` forks its children
//!      (a pipeline, a `foo && bar`), and killing the shell alone orphans the
//!      real linter still holding the read end of the pipe — the parent then
//!      blocks forever draining a pipe no one will close. The child is placed in
//!      its OWN process group and, on timeout, the group is signalled, so the
//!      command and everything it spawned go together.
//!
//!   2. **Capture is bounded.** A chatty adapter can print gigabytes; buffering
//!      it unbounded would let a subprocess decide loom's memory ceiling. Each
//!      stream keeps only its first and last [`KEEP_BYTES`], with the true byte
//!      total preserved so truncation stays visible.
//!
//!   3. **The tail survives.** A test runner prints its verdict LAST
//!      ("test result: ok. N passed"), so a head-only clip would drop exactly
//!      the line proof grading reads. Keeping both ends preserves it.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// Bytes kept from EACH END of EACH stream. Anything between the first and last
/// `KEEP_BYTES` is dropped (and counted), so peak buffered memory per stream is
/// about twice this.
pub const KEEP_BYTES: usize = 512 * 1024;

/// Per-observation channel inherited by child loom processes. The descriptor is
/// passed with [`Command::env`], never by mutating this process's environment.
const CONTENTION_FD_ENV: &str = "LOOM_CONTENTION_FD";

/// Absolute executable bound into shell observations. A shell function named
/// `loom` dispatches here so pipelines and compound journey steps exercise the
/// same binary as their recorder instead of an arbitrary older PATH install.
const CURRENT_LOOM_ENV: &str = "LOOM_CURRENT_EXE";

/// Exact, versioned frame written out-of-band when a child loom exits because
/// its graph or proof harness is contended. Keep this comfortably below
/// `PIPE_BUF` so one `write(2)` is atomic.
const CONTENTION_FRAME: &[u8] = b"LOOM-CONTENTION/1\n";

/// The inherited writer belongs only to this loom process. Startup moves it out
/// of the environment and marks it close-on-exec before any command can spawn.
/// `-1` means this process was not directly attested by a parent loom.
#[cfg(unix)]
static INHERITED_CONTENTION_FD: AtomicI32 = AtomicI32::new(-1);

/// What a bounded, group-reaped run observed.
pub struct Captured {
    pub status: ExitStatus,
    /// stdout bounded to head+tail of [`KEEP_BYTES`], with an omission marker
    /// spliced in when bytes were dropped.
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// The TRUE byte count seen on the stream, including bytes dropped from the
    /// middle — so a caller can report real size and flag truncation.
    pub stdout_total: usize,
    pub stderr_total: usize,
    contention_attested: bool,
}

impl Captured {
    /// Whether a child loom attested infrastructure contention over this run's
    /// private pipe. Crate-internal by design: callers classify only through
    /// the runner, not by manufacturing observations.
    pub(crate) fn is_loom_contention(&self) -> bool {
        self.contention_attested
    }
}

/// A bounded command observation, including the streams emitted before a
/// timeout killed the process group.
pub enum Observed {
    Exited(Captured),
    Killed {
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        stdout_total: usize,
        stderr_total: usize,
    },
}

/// Child identity policy selected by the subsystem that owns the execution.
/// Generic observation inherits. Validation may explicitly create a hermetic
/// fixture world for repository-native Cargo tests while retaining validator
/// authority for the outer verdict write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildEnvironment {
    Inherit,
    SoloTestFixture,
}

#[cfg(unix)]
struct AttestationReader(OwnedFd);

#[cfg(unix)]
type AttestationWriter = OwnedFd;

#[cfg(not(unix))]
struct AttestationReader;

#[cfg(not(unix))]
struct AttestationWriter;

#[cfg(unix)]
struct AttestationPipe {
    reader: Option<AttestationReader>,
    writer: Option<AttestationWriter>,
}

#[cfg(not(unix))]
struct AttestationPipe {
    reader: Option<AttestationReader>,
    writer: Option<AttestationWriter>,
}

/// Install a fresh anonymous attestation pipe only for a direct invocation of
/// this executable. Shell commands receive neither the descriptor nor its env
/// name; the caller has already made the direct-mode decision from parsed
/// tokens, so this capability boundary is not reparsed here.
#[cfg(unix)]
fn configure_attestation(cmd: &mut Command, direct_mode: bool) -> io::Result<AttestationPipe> {
    if !direct_mode {
        cmd.env_remove(CONTENTION_FD_ENV);
        return Ok(AttestationPipe {
            reader: None,
            writer: None,
        });
    }

    let mut fds = [-1; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `pipe` returned two newly-owned descriptors.
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    set_fd_flag(read.as_raw_fd(), libc::FD_CLOEXEC, true)?;
    set_fd_flag(write.as_raw_fd(), libc::FD_CLOEXEC, true)?;
    set_status_flag(read.as_raw_fd(), libc::O_NONBLOCK, true)?;

    let child_write_fd = write.as_raw_fd();
    cmd.env(CONTENTION_FD_ENV, child_write_fd.to_string());
    // SAFETY: the pre-exec closure itself performs only raw `fcntl` calls.
    // Avoid formatting or allocation in this post-fork context.
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(move || {
            let current = libc::fcntl(child_write_fd, libc::F_GETFD);
            if current == -1
                || libc::fcntl(child_write_fd, libc::F_SETFD, current & !libc::FD_CLOEXEC) == -1
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(AttestationPipe {
        reader: Some(AttestationReader(read)),
        writer: Some(write),
    })
}

#[cfg(not(unix))]
fn configure_attestation(cmd: &mut Command, _: bool) -> io::Result<AttestationPipe> {
    cmd.env_remove(CONTENTION_FD_ENV);
    Ok(AttestationPipe {
        reader: None,
        writer: None,
    })
}

#[cfg(unix)]
fn set_fd_flag(fd: libc::c_int, flag: libc::c_int, enabled: bool) -> io::Result<()> {
    let current = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if current == -1 {
        return Err(io::Error::last_os_error());
    }
    let wanted = if enabled {
        current | flag
    } else {
        current & !flag
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, wanted) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn set_status_flag(fd: libc::c_int, flag: libc::c_int, enabled: bool) -> io::Result<()> {
    let current = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if current == -1 {
        return Err(io::Error::last_os_error());
    }
    let wanted = if enabled {
        current | flag
    } else {
        current & !flag
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, wanted) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Read at most one frame plus one byte. The read end is nonblocking, so a
/// daemonized descendant retaining the inherited write descriptor cannot make
/// observation completion hang.
#[cfg(unix)]
fn read_attestation(reader: Option<&AttestationReader>) -> bool {
    let Some(reader) = reader else {
        return false;
    };
    let mut bytes = vec![0_u8; CONTENTION_FRAME.len() + 1];
    let mut used = 0;
    while used < bytes.len() {
        let read = unsafe {
            libc::read(
                reader.0.as_raw_fd(),
                bytes[used..].as_mut_ptr().cast(),
                bytes.len() - used,
            )
        };
        if read > 0 {
            used += read as usize;
            continue;
        }
        if read == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() == io::ErrorKind::WouldBlock {
            break;
        }
        return false;
    }
    &bytes[..used] == CONTENTION_FRAME
}

#[cfg(not(unix))]
fn read_attestation(_: Option<&AttestationReader>) -> bool {
    false
}

/// Parse a deliberately small shell subset. Quotes may surround one whole
/// token, but shell operators, substitutions, escapes, comments, and newlines
/// are rejected rather than interpreted. This is enough for the command form
/// loom generates while keeping the capability decision independent of `sh`.
pub(crate) fn strict_simple_tokens(command: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut chars = command.chars().peekable();

    while chars.peek().is_some() {
        while matches!(chars.peek(), Some(c) if c.is_whitespace() && *c != '\n' && *c != '\r') {
            chars.next();
        }
        let Some(&first) = chars.peek() else {
            break;
        };
        if first == '\n' || first == '\r' {
            return None;
        }

        let mut token = String::new();
        if first == '\'' || first == '"' {
            // `first` came from peek(); consuming it cannot miss.
            let quote = chars.next()?;
            let mut closed = false;
            for c in chars.by_ref() {
                if c == quote {
                    closed = true;
                    break;
                }
                if c == '\n' || c == '\r' || c == '\0' {
                    return None;
                }
                // Double quotes still perform substitution and escaping. Keep
                // their accepted form literal-only; single quotes already are.
                if quote == '"' && matches!(c, '$' | '`' | '\\') {
                    return None;
                }
                token.push(c);
            }
            if !closed || matches!(chars.peek(), Some(c) if !c.is_whitespace()) {
                return None;
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                if matches!(
                    c,
                    ';' | '|'
                        | '&'
                        | '<'
                        | '>'
                        | '$'
                        | '`'
                        | '\\'
                        | '\''
                        | '"'
                        | '#'
                        | '('
                        | ')'
                        | '{'
                        | '}'
                        | '['
                        | ']'
                        | '*'
                        | '?'
                        | '~'
                        | '\0'
                ) {
                    return None;
                }
                token.push(c);
                chars.next();
            }
        }
        if token.is_empty() {
            return None;
        }
        tokens.push(token);
    }
    Some(tokens)
}

/// Trusted direct-spawn plan. `executable` is captured from `current_exe`,
/// never from the candidate command token; `args` excludes argv0.
#[derive(Debug, PartialEq, Eq)]
struct DirectCurrentExe {
    executable: PathBuf,
    args: Vec<String>,
}

/// Return a direct-spawn plan for a simple invocation of this Loom executable.
/// A stored proof may name bare `loom`; resolving that through PATH can execute
/// an older installed schema, so a production Loom process binds it to its own
/// executable. Shell syntax still fails closed into ordinary shell mode.
/// Candidate argv0 is evidence used only for validation; execution always uses
/// the trusted `current_exe` captured here, closing alias replacement races.
fn direct_current_exe_tokens(command: &str) -> Option<DirectCurrentExe> {
    let current_exe = std::env::current_exe().ok()?;
    let tokens = strict_simple_tokens(command)?;
    let argv0 = tokens.first()?;
    if !names_current_loom(argv0, &current_exe) {
        return None;
    }
    Some(DirectCurrentExe {
        executable: current_exe,
        args: tokens.into_iter().skip(1).collect(),
    })
}

fn names_current_loom(argv0: &str, current_exe: &Path) -> bool {
    let current_is_loom = is_loom_executable(current_exe);
    let is_bare_loom = argv0 == "loom" && current_is_loom;
    let candidate = Path::new(argv0);
    let is_current_path = candidate == current_exe
        || (candidate.is_absolute()
            && candidate.canonicalize().ok().as_deref() == Some(current_exe));
    is_bare_loom || is_current_path
}

fn is_loom_executable(current_exe: &Path) -> bool {
    current_exe
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "loom")
}

fn shell_script(command: &str, current_exe: Option<&Path>) -> String {
    if current_exe.is_some_and(is_loom_executable) {
        // POSIX shell functions participate in pipelines and compound command
        // lists without changing PATH for unrelated tools. The absolute target
        // arrives through Command::env below, never through command text.
        format!("loom() {{ \"${CURRENT_LOOM_ENV}\" \"$@\"; }}\n{command}")
    } else {
        command.to_owned()
    }
}

#[cfg(test)]
fn is_direct_current_exe_invocation(command: &str) -> bool {
    direct_current_exe_tokens(command).is_some()
}

/// Consume an inherited contention capability at process startup. The
/// descriptor becomes process-local and close-on-exec immediately, and the env
/// name is removed before argument parsing or any command execution.
#[doc(hidden)]
pub fn initialize_contention_capability() {
    #[cfg(unix)]
    {
        let raw = std::env::var(CONTENTION_FD_ENV).ok();
        std::env::remove_var(CONTENTION_FD_ENV);
        let Some(fd) = raw.and_then(|value| value.parse::<libc::c_int>().ok()) else {
            return;
        };
        if fd < 0 {
            return;
        }
        if set_fd_flag(fd, libc::FD_CLOEXEC, true).is_err() {
            unsafe {
                libc::close(fd);
            }
            return;
        }
        let status = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if status == -1 || status & libc::O_ACCMODE == libc::O_RDONLY {
            unsafe {
                libc::close(fd);
            }
            return;
        }
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == -1 {
            unsafe {
                libc::close(fd);
            }
            return;
        }
        // SAFETY: `fstat` initialized `stat` on success.
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
            unsafe {
                libc::close(fd);
            }
            return;
        }
        if INHERITED_CONTENTION_FD
            .compare_exchange(-1, fd, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            unsafe {
                libc::close(fd);
            }
        }
    }
    #[cfg(not(unix))]
    std::env::remove_var(CONTENTION_FD_ENV);
}

/// Best-effort attestation used by the CLI error path before it exits 75. A
/// normal direct CLI has no inherited descriptor, so its visible behavior is
/// unchanged.
#[doc(hidden)]
pub fn attest_contention_from_env() {
    #[cfg(unix)]
    {
        let fd = INHERITED_CONTENTION_FD.load(Ordering::SeqCst);
        if fd < 0 {
            return;
        }
        loop {
            let written = unsafe {
                libc::write(fd, CONTENTION_FRAME.as_ptr().cast(), CONTENTION_FRAME.len())
            };
            if written == CONTENTION_FRAME.len() as isize {
                return;
            }
            if written == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
    }
}

/// A head+tail capture window: the first `KEEP_BYTES` verbatim, then a ring of
/// the last `KEEP_BYTES`, so both ends survive an overrun.
#[derive(Default)]
struct Window {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
}

impl Window {
    fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len();
        for &b in chunk {
            if self.head.len() < KEEP_BYTES {
                self.head.push(b);
            } else {
                self.tail.push_back(b);
                if self.tail.len() > KEEP_BYTES {
                    self.tail.pop_front();
                }
            }
        }
    }

    fn render(&self) -> Vec<u8> {
        let dropped = self.total.saturating_sub(self.head.len() + self.tail.len());
        let mut out = self.head.clone();
        if dropped > 0 {
            out.extend_from_slice(format!("\n…[{dropped} bytes omitted]…\n").as_bytes());
        }
        out.extend(self.tail.iter().copied());
        out
    }
}

fn capture_stream<R>(mut stream: R) -> thread::JoinHandle<io::Result<Window>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut window = Window::default();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => return Ok(window),
                Ok(n) => window.push(&buffer[..n]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    })
}

fn join_capture(handle: thread::JoinHandle<io::Result<Window>>) -> io::Result<Window> {
    handle
        .join()
        .map_err(|_| io::Error::other("subprocess capture thread panicked"))?
}

#[cfg(unix)]
fn terminate_process_group(child: &mut Child, pgid: libc::pid_t) -> io::Result<()> {
    // SIGTERM gives well-behaved children the same termination signal the old
    // runner used. SIGKILL immediately follows as a bounded sweep: descendants
    // may otherwise keep the capture pipes open forever after the leader exits.
    unsafe {
        libc::killpg(pgid, libc::SIGTERM);
        libc::killpg(pgid, libc::SIGKILL);
    }
    child.wait().map(|_| ())
}

#[cfg(not(unix))]
fn terminate_process_group(child: &mut Child, _pgid: i32) -> io::Result<()> {
    child.kill()?;
    child.wait().map(|_| ())
}

/// Run `command` at `cwd`, directly for the strict current-executable form and
/// otherwise through `sh -c`, bounding captured output and killing the whole
/// process group on timeout. Compatibility wrapper for callers that only need
/// to distinguish completion from timeout.
pub fn run(command: &str, cwd: &Path, timeout: Duration) -> io::Result<Option<Captured>> {
    run_with_environment(command, cwd, timeout, ChildEnvironment::Inherit)
}

pub(crate) fn run_with_environment(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    environment: ChildEnvironment,
) -> io::Result<Option<Captured>> {
    Ok(
        match run_observed_with_environment(command, cwd, timeout, environment)? {
            Observed::Exited(captured) => Some(captured),
            Observed::Killed { .. } => None,
        },
    )
}

/// Run a bounded command while retaining stdout/stderr even when timeout
/// enforcement kills the process group. A strict direct invocation of the
/// current executable is spawned from parsed argv without a shell; every other
/// command retains the historical `sh -c` behavior.
pub fn run_observed(command: &str, cwd: &Path, timeout: Duration) -> io::Result<Observed> {
    run_observed_with_environment(command, cwd, timeout, ChildEnvironment::Inherit)
}

pub(crate) fn run_observed_with_environment(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    environment: ChildEnvironment,
) -> io::Result<Observed> {
    let current_exe = std::env::current_exe().ok();
    let direct_plan = direct_current_exe_tokens(command);
    let direct_mode = direct_plan.is_some();
    let mut cmd = if let Some(plan) = direct_plan {
        let mut cmd = Command::new(plan.executable);
        cmd.args(plan.args);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(shell_script(command, current_exe.as_deref()));
        cmd
    };
    if let Some(current) = current_exe.as_deref().filter(|exe| is_loom_executable(exe)) {
        cmd.env(CURRENT_LOOM_ENV, current);
    } else {
        cmd.env_remove(CURRENT_LOOM_ENV);
    }
    if environment == ChildEnvironment::SoloTestFixture {
        cmd.env(crate::identity::AGENT_ENV, "solo");
        cmd.env_remove(crate::identity::PROFILE_ENV);
    }
    cmd.current_dir(cwd)
        // Observations are deliberately non-interactive. Once the child leads
        // its own process group it is a background group relative to Loom's
        // terminal; inheriting stdin lets an accidental read stop it with
        // SIGTTIN. process_control reported that stop as an exit race and
        // panicked, while a plain poller correctly waited forever. EOF makes
        // the boundary explicit and lets callers classify the refusal.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The child leads its own process group (pgid == its pid), so a timeout can
    // signal the group and reap whatever the command forked, not just its group
    // leader.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut attestation = configure_attestation(&mut cmd, direct_mode)?;
    let mut child = cmd.spawn()?;
    // Only the child side may attest. Keeping a parent writer would also make
    // EOF unavailable, though reads are nonblocking as defence in depth.
    drop(attestation.writer.take());
    #[cfg(unix)]
    let pgid = child.id() as libc::pid_t;
    #[cfg(not(unix))]
    let pgid = 0;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("subprocess stdout was not piped"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("subprocess stderr was not piped"))?;
    let stdout_handle = capture_stream(stdout);
    let stderr_handle = capture_stream(stderr);

    // `process_control` used a second platform wait handle and then expected
    // `std::process::Child` to report the same exit synchronously. On macOS a
    // fast child can be visible to the first waiter before `try_wait`, causing
    // the dependency to panic with "missing exit status". One Child owns both
    // polling and reaping here, so an exit-by-code or exit-by-signal is always
    // represented by the real std ExitStatus.
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            terminate_process_group(&mut child, pgid)?;
            break None;
        }
        thread::sleep(
            timeout
                .saturating_sub(elapsed)
                .min(Duration::from_millis(10)),
        );
    };

    let out = join_capture(stdout_handle)?;
    let err = join_capture(stderr_handle)?;
    if let Some(status) = status {
        let contention_attested = read_attestation(attestation.reader.as_ref());
        Ok(Observed::Exited(Captured {
            status,
            stdout: out.render(),
            stderr: err.render(),
            stdout_total: out.total,
            stderr_total: err.total,
            contention_attested,
        }))
    } else {
        Ok(Observed::Killed {
            stdout: out.render(),
            stderr: err.render(),
            stdout_total: out.total,
            stderr_total: err.total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_passes_through_whole_and_untruncated() {
        let dir = std::env::temp_dir();
        let cap = run(
            "printf 'hello\\n'; printf 'oops\\n' 1>&2",
            &dir,
            Duration::from_secs(5),
        )
        .expect("io")
        .expect("did not time out");
        assert_eq!(cap.stdout, b"hello\n");
        assert_eq!(cap.stderr, b"oops\n");
        assert_eq!(cap.stdout_total, 6);
        assert_eq!(cap.status.code(), Some(0));
    }

    /// A command may terminate by signal instead of returning a numeric exit
    /// code. The observer must preserve that real outcome rather than panic
    /// while trying to manufacture an exit status.
    #[cfg(unix)]
    #[test]
    fn a_signaled_command_is_observed_without_panicking() {
        use std::os::unix::process::ExitStatusExt;

        let dir = std::env::temp_dir();
        let cap = run("kill -TERM $$", &dir, Duration::from_secs(5))
            .expect("io")
            .expect("command completed before the timeout");
        assert!(!cap.status.success());
        assert_eq!(cap.status.code(), None);
        assert_eq!(cap.status.signal(), Some(libc::SIGTERM));
    }

    #[test]
    fn observed_commands_receive_eof_instead_of_inheriting_the_terminal() {
        let dir = std::env::temp_dir();
        let cap = run("read value; test $? -ne 0", &dir, Duration::from_secs(5))
            .expect("io")
            .expect("stdin EOF must not wait for the timeout");
        assert!(
            cap.status.success(),
            "stdin should be closed for observations"
        );
    }

    /// The verdict a runner prints LAST must survive an overrun: keep the tail,
    /// not just the head, and mark the gap. The middle must actually overflow
    /// the tail ring for anything to be dropped.
    #[test]
    fn a_huge_stream_keeps_both_ends_and_reports_the_true_total() {
        let mut w = Window::default();
        w.push(&vec![b'a'; KEEP_BYTES]); // fills the head
        w.push(&vec![b'X'; 2 * KEEP_BYTES]); // overflows the tail ring: drops
        w.push(b"VERDICT-AT-THE-END");
        let rendered = w.render();
        let text = String::from_utf8_lossy(&rendered);
        assert!(text.starts_with("aaaa"), "head kept");
        assert!(
            text.ends_with("VERDICT-AT-THE-END"),
            "tail kept: {}",
            &text[text.len() - 40..]
        );
        assert!(text.contains("bytes omitted"), "gap marked");
        assert_eq!(
            w.total,
            KEEP_BYTES + 2 * KEEP_BYTES + 18,
            "true total counts dropped bytes"
        );
    }

    /// A command that outruns its time limit is reported as a timeout (None),
    /// and the process group is reaped rather than orphaned.
    #[test]
    fn a_command_over_its_limit_times_out() {
        let dir = std::env::temp_dir();
        let out = run("sleep 5", &dir, Duration::from_millis(200)).expect("io");
        assert!(
            out.is_none(),
            "a command past its deadline reports a timeout"
        );
    }

    #[test]
    fn simple_current_exe_commands_are_direct_and_shell_forms_are_not() {
        let exe = std::env::current_exe().expect("current exe");
        let plain = format!("{} --graph /tmp/graph sync", exe.display());
        let quoted = format!("'{}' --graph '/tmp/a graph' sync", exe.display());
        let double_quoted = format!("\"{}\" --graph \"/tmp/a graph\" sync", exe.display());
        assert!(is_direct_current_exe_invocation(&plain));
        assert!(is_direct_current_exe_invocation(&quoted));
        assert!(is_direct_current_exe_invocation(&double_quoted));
        assert!(is_direct_current_exe_invocation(&format!(
            "{} --graph /tmp/g status",
            exe.display()
        )));
        assert!(is_direct_current_exe_invocation(&format!(
            "{} --graph /tmp/g sync --json",
            exe.display()
        )));
        let plan = direct_current_exe_tokens(&quoted).unwrap();
        assert_eq!(plan.executable, exe);
        assert_eq!(
            plan.args,
            vec![
                "--graph".to_string(),
                "/tmp/a graph".to_string(),
                "sync".to_string()
            ]
        );

        for rejected in [
            format!("{} --graph /tmp/g sync; exit 75", exe.display()),
            format!("{} --graph /tmp/g sync | cat", exe.display()),
            format!("{} --graph /tmp/g sync > /tmp/out", exe.display()),
            format!("X=1 {} --graph /tmp/g sync", exe.display()),
            format!("{} --graph $(printf /tmp/g) sync", exe.display()),
            format!("{} --graph /tmp/g sync\nexit 75", exe.display()),
        ] {
            assert!(
                !is_direct_current_exe_invocation(&rejected),
                "shell syntax must disable attestation: {rejected}"
            );
        }
    }

    #[test]
    fn bare_loom_binds_only_to_a_running_loom_binary() {
        assert!(names_current_loom("loom", Path::new("/tmp/loom")));
        assert!(!names_current_loom(
            "loom",
            Path::new("/tmp/loom-unit-test-hash")
        ));
        assert!(!names_current_loom("other", Path::new("/tmp/loom")));
    }

    #[test]
    fn shell_contained_loom_is_bound_without_rewriting_other_commands() {
        let command = "printf x | loom mcp serve && cargo test";
        assert_eq!(
            shell_script(command, Some(Path::new("/tmp/loom"))),
            format!("loom() {{ \"${CURRENT_LOOM_ENV}\" \"$@\"; }}\n{command}")
        );
        assert_eq!(
            shell_script(command, Some(Path::new("/tmp/loom-test-hash"))),
            command
        );
        assert_eq!(shell_script(command, None), command);
    }

    #[cfg(unix)]
    #[test]
    fn validated_symlink_replacement_cannot_change_direct_spawn_executable() {
        use std::os::unix::fs::symlink;

        let exe = std::env::current_exe().expect("current exe");
        let dir =
            std::env::temp_dir().join(format!("loom-direct-plan-toctou-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let alias = dir.join("loom-alias");
        symlink(&exe, &alias).unwrap();
        let command = format!("{} --graph '/tmp/a graph' sync", alias.display());

        let plan = direct_current_exe_tokens(&command).expect("validated alias");
        std::fs::remove_file(&alias).unwrap();
        symlink("/bin/false", &alias).unwrap();

        assert_eq!(plan.executable, exe);
        assert_eq!(
            plan.args,
            vec![
                "--graph".to_string(),
                "/tmp/a graph".to_string(),
                "sync".to_string()
            ]
        );
        assert_ne!(plan.executable, alias);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An arbitrary descendant receives no attestation descriptor, so its
    /// lifetime cannot delay completion of an unrelated observed shell.
    #[cfg(unix)]
    #[test]
    fn a_descendant_retaining_the_attestation_fd_does_not_delay_completion() {
        let dir = std::env::temp_dir();
        let started = std::time::Instant::now();
        let cap = run(
            "(sleep 3 >/dev/null 2>&1) & exit 0",
            &dir,
            Duration::from_secs(2),
        )
        .expect("io")
        .expect("shell completed");
        assert_eq!(cap.status.code(), Some(0));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "attestation read waited for descendant EOF"
        );
    }
}
