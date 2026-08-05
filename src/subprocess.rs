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
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use process_control::{ChildExt, Control};

/// Bytes kept from EACH END of EACH stream. Anything between the first and last
/// `KEEP_BYTES` is dropped (and counted), so peak buffered memory per stream is
/// about twice this.
pub const KEEP_BYTES: usize = 512 * 1024;

/// Per-observation channel inherited by child loom processes. The descriptor is
/// passed with [`Command::env`], never by mutating this process's environment.
const CONTENTION_FD_ENV: &str = "LOOM_CONTENTION_FD";

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
    pub status: process_control::ExitStatus,
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
fn simple_shell_tokens(command: &str) -> Option<Vec<String>> {
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
            let quote = chars.next().expect("peeked quote");
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

/// Return a direct-spawn plan only for the exact simple form loom generates:
/// this executable, `--graph`, one graph path, then `sync`. Narrowing the
/// accepted command shape keeps future shell-facing arguments from silently
/// expanding the capability boundary. Candidate argv0 is evidence used only
/// for validation; execution always uses the trusted `current_exe` captured
/// here, closing replacement races on aliases and symlinks.
fn direct_current_exe_tokens(command: &str) -> Option<DirectCurrentExe> {
    let current_exe = std::env::current_exe().ok()?;
    let tokens = simple_shell_tokens(command)?;
    if tokens.len() != 4 || tokens[1] != "--graph" || tokens[2].is_empty() || tokens[3] != "sync" {
        return None;
    }
    let candidate = Path::new(&tokens[0]);
    let is_current = candidate == current_exe
        || (candidate.is_absolute()
            && candidate.canonicalize().ok().as_deref() == Some(current_exe.as_path()));
    is_current.then(|| DirectCurrentExe {
        executable: current_exe,
        args: tokens.into_iter().skip(1).collect(),
    })
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

/// Run `command` at `cwd`, directly for the strict current-executable form and
/// otherwise through `sh -c`, bounding captured output and killing the whole
/// process group on timeout. Compatibility wrapper for callers that only need
/// to distinguish completion from timeout.
pub fn run(command: &str, cwd: &Path, timeout: Duration) -> io::Result<Option<Captured>> {
    Ok(match run_observed(command, cwd, timeout)? {
        Observed::Exited(captured) => Some(captured),
        Observed::Killed { .. } => None,
    })
}

/// Run a bounded command while retaining stdout/stderr even when timeout
/// enforcement kills the process group. A strict direct invocation of the
/// current executable is spawned from parsed argv without a shell; every other
/// command retains the historical `sh -c` behavior.
pub fn run_observed(command: &str, cwd: &Path, timeout: Duration) -> io::Result<Observed> {
    let direct_plan = direct_current_exe_tokens(command);
    let direct_mode = direct_plan.is_some();
    let mut cmd = if let Some(plan) = direct_plan {
        let mut cmd = Command::new(plan.executable);
        cmd.args(plan.args);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    };
    cmd.current_dir(cwd)
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
    let child = cmd.spawn()?;
    // Only the child side may attest. Keeping a parent writer would also make
    // EOF unavailable, though reads are nonblocking as defence in depth.
    drop(attestation.writer.take());
    #[cfg(unix)]
    let pgid = child.id() as libc::pid_t;

    let out_win = Arc::new(Mutex::new(Window::default()));
    let err_win = Arc::new(Mutex::new(Window::default()));
    let ow = Arc::clone(&out_win);
    let ew = Arc::clone(&err_win);

    // Return `false` from the filters so process_control keeps nothing itself;
    // the bounded window is the only buffer, which is what caps memory.
    let result = child
        .controlled_with_output()
        .time_limit(timeout)
        .terminate_for_timeout()
        .stdout_filter(move |chunk: &[u8]| {
            ow.lock().expect("stdout window").push(chunk);
            io::Result::Ok(false)
        })
        .stderr_filter(move |chunk: &[u8]| {
            ew.lock().expect("stderr window").push(chunk);
            io::Result::Ok(false)
        })
        .wait()?;

    match result {
        Some(o) => {
            let contention_attested = read_attestation(attestation.reader.as_ref());
            let out = out_win.lock().expect("stdout window");
            let err = err_win.lock().expect("stderr window");
            Ok(Observed::Exited(Captured {
                status: o.status,
                stdout: out.render(),
                stderr: err.render(),
                stdout_total: out.total,
                stderr_total: err.total,
                contention_attested,
            }))
        }
        None => {
            // process_control has already terminated the group leader; sweep
            // the group so no descendant behind the capture pipes can outlive
            // the timeout as an orphan.
            #[cfg(unix)]
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
            let out = out_win.lock().expect("stdout window");
            let err = err_win.lock().expect("stderr window");
            Ok(Observed::Killed {
                stdout: out.render(),
                stderr: err.render(),
                stdout_total: out.total,
                stderr_total: err.total,
            })
        }
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
    fn only_literal_direct_current_exe_commands_are_attestable() {
        let exe = std::env::current_exe().expect("current exe");
        let plain = format!("{} --graph /tmp/graph sync", exe.display());
        let quoted = format!("'{}' --graph '/tmp/a graph' sync", exe.display());
        let double_quoted = format!("\"{}\" --graph \"/tmp/a graph\" sync", exe.display());
        assert!(is_direct_current_exe_invocation(&plain));
        assert!(is_direct_current_exe_invocation(&quoted));
        assert!(is_direct_current_exe_invocation(&double_quoted));
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
            format!("{} --graph /tmp/g status", exe.display()),
            format!("{} --graph /tmp/g sync --json", exe.display()),
            format!("{} --graph /tmp/g sync\nexit 75", exe.display()),
        ] {
            assert!(
                !is_direct_current_exe_invocation(&rejected),
                "shell syntax must disable attestation: {rejected}"
            );
        }
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
