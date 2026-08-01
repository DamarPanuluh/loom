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
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use process_control::{ChildExt, Control};

/// Bytes kept from EACH END of EACH stream. Anything between the first and last
/// `KEEP_BYTES` is dropped (and counted), so peak buffered memory per stream is
/// about twice this.
pub const KEEP_BYTES: usize = 512 * 1024;

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

/// Run `sh -c command` at `cwd`, bounding captured output and killing the whole
/// process group on timeout. Returns `Ok(None)` when the time limit elapsed
/// (after the group was signalled), or `Ok(Some(_))` with the observed result.
pub fn run(command: &str, cwd: &Path, timeout: Duration) -> io::Result<Option<Captured>> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The child leads its own process group (pgid == its pid), so a timeout can
    // signal the group and reap whatever the shell forked, not just the shell.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn()?;
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
            let out = out_win.lock().expect("stdout window");
            let err = err_win.lock().expect("stderr window");
            Ok(Some(Captured {
                status: o.status,
                stdout: out.render(),
                stderr: err.render(),
                stdout_total: out.total,
                stderr_total: err.total,
            }))
        }
        None => {
            // process_control has already terminated the group leader; sweep the
            // group so any child the shell forked behind the pipe cannot outlive
            // the timeout as an orphan.
            #[cfg(unix)]
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
            Ok(None)
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
}
