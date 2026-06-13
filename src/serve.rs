//! `loom serve` — the OPT-IN daemon (slice ⑥c).
//!
//! A persistent process holds the graph open ONCE (the sole lock-holder) and
//! serves many client requests against it over a Unix domain socket, amortizing
//! the ~36–100 ms per-call DB-open floor to ~0 and unlocking safe concurrent
//! multi-agent access (grafeo in-process MVCC — proven by
//! tests/grafeo_probe.rs::daemon_contract_concurrent_sessions_persistent).
//!
//! NON-NEGOTIABLE: the daemon is an OPTIONAL PERFORMANCE LAYER, never a
//! correctness dependency. The client only attempts it when `LOOM_DAEMON=1`,
//! and ANY failure or uncertainty (can't connect, parse fails, command not
//! servable, human mode, unresolvable version skew, a panic) makes the client
//! FALL BACK to today's direct `commands::dispatch`. With `LOOM_DAEMON` unset
//! the daemon code path is never taken and behaviour is byte-identical to today.
//!
//! Transport: length-prefixed (u32 big-endian) JSON frames. The protocol is
//! deliberately tiny — see [`Request`] / [`Reply`].

use crate::commands::{dispatch_with_db, DispatchOutcome};
use crate::db::GrafeoDb;
use crate::output::Printer;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Wire protocol
// ---------------------------------------------------------------------------

/// Client → daemon. `argv` is the raw post-binary argument vector (what
/// `std::env::args().skip(1)` yields), re-parsed daemon-side via clap so the
/// daemon and client never disagree about argument semantics.
#[derive(Serialize, Deserialize)]
struct Request {
    /// The client's binary identity — the skew handshake key.
    build_id: String,
    /// The client's `--json` flag. The daemon only serves JSON requests
    /// (human-mode stdout can't be captured per-connection without racing);
    /// `false` ⇒ the daemon replies `fallback`.
    json: bool,
    /// The command's argument vector (excluding the binary name).
    argv: Vec<String>,
}

/// Daemon → client. Three terminal shapes, all of which the client handles
/// without ever panicking.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Reply {
    /// The command ran against the held handle. `stdout` is replayed verbatim;
    /// the process exits with `exit_code`.
    Output { stdout: String, exit_code: i32 },
    /// The command is not daemon-servable (graph-releasing, lifecycle, human
    /// mode, unknown token) — the client runs it via direct dispatch.
    Fallback,
    /// The daemon's build id differs from the client's. The client drains this
    /// daemon and respawns a fresh one on its own binary.
    VersionMismatch,
}

// ---------------------------------------------------------------------------
// Identity & paths
// ---------------------------------------------------------------------------

/// This binary's skew identity: an FNV-1a hash of the running executable's
/// bytes. Stricter than the git build stamp — a dirty rebuild at the same
/// commit produces different bytes, so a stale in-memory daemon is caught even
/// when `loom --version` would read identical. Computed once (the file read is
/// not free) and memoized. Falls back to the compile-time build stamp if the
/// executable can't be read (the handshake still functions, just coarser).
pub fn binary_identity() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        std::env::current_exe()
            .and_then(std::fs::read)
            .map(|bytes| crate::repo::content_hash(&bytes))
            .unwrap_or_else(|_| format!("stamp:{}", env!("LOOM_BUILD")))
    })
    .clone()
}

/// The per-graph socket path — one daemon per resolved graph root, keyed by
/// that root (so different repos never cross-talk). Lives under `.loom/`
/// alongside the graph it serves.
pub fn socket_path(root: &Path) -> PathBuf {
    root.join(".loom").join("daemon.sock")
}

// ---------------------------------------------------------------------------
// Frame I/O — length-prefixed (u32 BE) JSON
// ---------------------------------------------------------------------------

fn write_frame<W: Write, T: Serialize>(w: &mut W, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(bytes.len())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

fn read_frame<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> std::io::Result<T> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    // A sane ceiling so a corrupt/hostile length can't make us allocate wildly;
    // loom requests/replies are tiny (argv + one rendered JSON payload).
    const MAX_FRAME: usize = 64 * 1024 * 1024;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame exceeds maximum size",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

/// Run the daemon: hold ONE open graph, serve requests until idle for
/// `idle_secs` (or a `__drain__` signal arrives), then drain in-flight work and
/// exit — releasing the lock. Never squats on a repo's graph.
pub fn serve(root: &Path, idle_secs: u64) -> Result<()> {
    // A lazily-spawned daemon is a child of a SHORT-LIVED client. Even detached
    // into its own session (`setsid` in `spawn_daemon`), it can still be handed
    // a SIGHUP when the controlling terminal goes away and a SIGPIPE if it ever
    // writes to a closed stdio — either would kill it right after its first
    // request, collapsing all amortization (the symptom: "daemon serves one
    // request, then GONE"). Ignore both so the daemon lives out its idle window.
    ignore_hangup_signals();

    let sock = socket_path(root);
    // Open the graph once — the sole lock-holder. If this fails (e.g. another
    // process — a stale daemon — already holds the lock), bail; the client's
    // connect attempt will have already failed/fallen back.
    let db_file = crate::db::ensure_initialized(root)?;
    let handle = GrafeoDb::open(&db_file)?.handle();
    let my_id = binary_identity();

    // A stale socket file from a crashed predecessor must not block the bind.
    let _ = std::fs::remove_file(&sock);
    if let Some(parent) = sock.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&sock)?;
    listener.set_nonblocking(true)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let mut last_activity = Instant::now();
    let idle = Duration::from_secs(idle_secs);

    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                last_activity = Instant::now();
                // The listener polls non-blocking, but the accepted stream
                // INHERITS that flag (notably on macOS) — a non-blocking
                // `read_exact` would return WouldBlock instantly and drop the
                // request unanswered. Put each connection back in blocking mode
                // (with read/write timeouts so a stalled client can't pin a
                // worker thread forever) before handing it to the worker.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
                let handle = Arc::clone(&handle);
                let my_id = my_id.clone();
                let root = root.to_path_buf();
                let shutdown = Arc::clone(&shutdown);
                let in_flight = Arc::clone(&in_flight);
                in_flight.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    // A per-connection panic must never take the daemon down —
                    // the worst case is one client falling back.
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handle_connection(stream, &handle, &my_id, &root, &shutdown);
                    }));
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connection. Honour an in-flight drain request, then
                // the idle timeout — but never exit while work is in flight.
                if shutdown.load(Ordering::SeqCst) || last_activity.elapsed() >= idle {
                    if in_flight.load(Ordering::SeqCst) == 0 {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => {
                // A transient accept error — back off briefly and keep serving.
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    // Drain: wait for any straggler thread to finish, then release the lock
    // (drop the held handle) BEFORE removing the socket. Ordering matters: a
    // client that drained us (to run a graph-releasing command direct) waits
    // for the socket to disappear and then opens the graph — so the socket must
    // vanish only AFTER the lock is actually free, or that client would still
    // hit "locked by another process".
    while in_flight.load(Ordering::SeqCst) > 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(handle);
    let _ = std::fs::remove_file(&sock);
    Ok(())
}

/// Set SIGHUP and SIGPIPE to SIG_IGN so a detached daemon isn't killed when its
/// spawning client exits / closes the pipe. Uses `signal(2)` directly (no extra
/// dep). Best-effort: a failure leaves the default disposition.
fn ignore_hangup_signals() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIG_IGN: usize = 1;
    const SIGHUP: i32 = 1;
    const SIGPIPE: i32 = 13;
    // SAFETY: installing SIG_IGN for HUP/PIPE has no memory effects and is a
    // standard daemonization step.
    unsafe {
        signal(SIGHUP, SIG_IGN);
        signal(SIGPIPE, SIG_IGN);
    }
}

/// Serve one connection: read the request, route it, write exactly one reply.
fn handle_connection(
    mut stream: UnixStream,
    handle: &Arc<grafeo::GrafeoDB>,
    my_id: &str,
    root: &Path,
    shutdown: &AtomicBool,
) {
    let req: Request = match read_frame(&mut stream) {
        Ok(r) => r,
        // A malformed request: nothing safe to reply with the right shape; the
        // client's own read will fail and it falls back. Just return.
        Err(_) => return,
    };

    // The drain signal: a stale daemon being retired by a newer client. Flag
    // shutdown (the accept loop exits once in-flight reaches zero) and ack so
    // the client knows it landed before it waits for the lock to free.
    if req.argv == ["__drain__"] {
        shutdown.store(true, Ordering::SeqCst);
        let _ = write_frame(&mut stream, &Reply::Output {
            stdout: String::new(),
            exit_code: 0,
        });
        return;
    }

    // Version-skew handshake — stricter than the git stamp (the executable's
    // bytes). A mismatch means the binary was replaced under a running daemon;
    // tell the client to drain + respawn rather than serve stale logic.
    if req.build_id != my_id {
        let _ = write_frame(&mut stream, &Reply::VersionMismatch);
        return;
    }

    // Human mode can't be captured per-request without racing concurrent
    // threads on process stdout — fall back to direct.
    if !req.json {
        let _ = write_frame(&mut stream, &Reply::Fallback);
        return;
    }

    // Parse the argv via the SAME clap entry point the direct path uses. A
    // parse failure (clap would print to stderr/exit in the direct path) →
    // fall back so the client surfaces the real teaching error.
    let cli = match crate::cli::Cli::try_parse_from_argv(&req.argv) {
        Ok(c) => c,
        Err(_) => {
            let _ = write_frame(&mut stream, &Reply::Fallback);
            return;
        }
    };

    // Each connection gets its OWN session over the shared open handle (grafeo
    // MVCC: concurrent sessions on one persistent handle are proven safe).
    let db = GrafeoDb::from_handle(Arc::clone(handle));
    let printer = Printer::capturing(true);

    let reply = match dispatch_with_db(&db, root, cli, &printer) {
        DispatchOutcome::NotServable => Reply::Fallback,
        DispatchOutcome::Ran(result) => {
            let stdout = printer.captured().unwrap_or_default();
            // The command's own Result is its exit verdict: Ok → 0, Err → the
            // error text on stderr… but the daemon path only carries stdout, so
            // an Err becomes a non-zero exit and its message is appended to
            // stdout (the direct path would print it via the `Result` bubbling
            // to `main`, which writes to stderr — see the client note below).
            match result {
                Ok(()) => Reply::Output { stdout, exit_code: 0 },
                Err(e) => Reply::Output {
                    // Mirror anyhow's `main` rendering: "Error: <msg>" on a fresh
                    // line. The client prints it to stderr to match direct mode.
                    stdout: format!("{stdout}\x1e{e:?}"),
                    exit_code: 1,
                },
            }
        }
    };
    let _ = write_frame(&mut stream, &reply);
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// The client side of the daemon path. Returns:
/// - `None` ⇒ "fall back to direct dispatch" (daemon off, not servable, any
///   I/O / parse error, unresolved skew). The caller runs `commands::dispatch`.
/// - `Some(Ok(()))` / `Some(Err(..))` ⇒ the daemon ran the command; this is the
///   terminal result for the process (already printed to stdout/stderr here).
///
/// Only proceeds when `LOOM_DAEMON=1` AND the request is `--json`. NEVER panics.
pub fn try_client(cli_json: bool, argv: &[String]) -> Option<Result<()>> {
    let want_daemon =
        std::env::var("LOOM_DAEMON").ok().as_deref() == Some("1") && cli_json;
    // Any failure resolving the root means we can't know which graph to talk to
    // — fall back (direct dispatch resolves it the same way and errors cleanly).
    let root = crate::db::resolve_root().ok()?;
    let sock = socket_path(&root);

    if !want_daemon {
        // The DIRECT path (human mode, non-json, or LOOM_DAEMON unset). A LIVE
        // daemon holds the graph's EXCLUSIVE lock, so a direct GrafeoDb::open
        // would fail "locked by another process" — breaking the invariant that
        // fallback == today's behaviour. If a daemon is live (the socket
        // accepts a connection), retire it first (drain → it commits, frees the
        // lock, exits), then run direct; the next servable call re-spawns one. A
        // CRASHED daemon already released the lock (the OS drops the flock on
        // death), so only a live one needs draining. No socket ⇒ pure-direct
        // session ⇒ the connect fails instantly and we add nothing.
        if UnixStream::connect(&sock).is_ok() {
            drain(&sock);
            wait_socket_gone(&sock, Duration::from_secs(5));
        }
        return None;
    }

    let mut stream = connect_or_spawn(&sock, &root)?;

    match round_trip(&mut stream, argv) {
        Some(Reply::Output { stdout, exit_code }) => Some(replay(stdout, exit_code)),
        // We could not even reach a live daemon — direct dispatch resolves the
        // graph itself and there's no lock to free; just fall back.
        None => None,
        Some(Reply::Fallback) => {
            // The command is NOT daemon-servable (graph-releasing: validate /
            // saga; or lifecycle: init / etc.). The daemon HOLDS THE LOCK, so a
            // naive direct open would fail with "locked by another process" —
            // which would break the invariant that fallback == today's direct
            // behaviour. DRAIN the daemon first (it finishes in-flight work,
            // commits, releases the lock, exits), wait for the lock to free,
            // THEN fall back to direct. The next servable call lazily re-spawns
            // a fresh daemon. Graph-releasing commands thus run direct exactly
            // as documented, and correctness never depends on the daemon.
            drop(stream);
            drain(&sock);
            wait_socket_gone(&sock, Duration::from_secs(5));
            None
        }
        Some(Reply::VersionMismatch) => {
            // The running daemon is stale. Drain it, wait for the lock to free,
            // spawn a fresh one on THIS binary, retry exactly once.
            drop(stream);
            drain(&sock);
            wait_socket_gone(&sock, Duration::from_secs(5));
            let mut fresh = connect_or_spawn(&sock, &root)?;
            match round_trip(&mut fresh, argv) {
                Some(Reply::Output { stdout, exit_code }) => Some(replay(stdout, exit_code)),
                // Still not resolved (or skewed again) — fall back to direct.
                _ => None,
            }
        }
    }
}

/// One request/reply exchange. Any I/O or parse error ⇒ `None` (fall back).
fn round_trip(stream: &mut UnixStream, argv: &[String]) -> Option<Reply> {
    let req = Request {
        build_id: binary_identity(),
        json: true,
        argv: argv.to_vec(),
    };
    write_frame(stream, &req).ok()?;
    read_frame(stream).ok()
}

/// Replay the daemon's captured output and become its exit verdict. stdout is
/// printed verbatim; a non-zero exit carries the error text (split on the RS
/// `\x1e` sentinel) to stderr, mirroring direct mode's `Error: …` on stderr.
fn replay(stdout: String, exit_code: i32) -> Result<()> {
    if exit_code == 0 {
        print!("{stdout}");
        let _ = std::io::stdout().flush();
        Ok(())
    } else {
        // Split captured stdout from the appended error rendering.
        let (out, err) = match stdout.split_once('\x1e') {
            Some((o, e)) => (o.to_string(), e.to_string()),
            None => (stdout, String::new()),
        };
        print!("{out}");
        let _ = std::io::stdout().flush();
        // Returning Err lets `main` render it to stderr exactly as the direct
        // path would (anyhow's `Error: <msg>`), so behaviour stays identical.
        if err.is_empty() {
            anyhow::bail!("command failed (exit {exit_code})")
        } else {
            Err(anyhow::anyhow!("{err}"))
        }
    }
}

/// Bounded so a wedged daemon (accepts a connection but never replies) can
/// NEVER hang the client: on timeout `read_frame` returns Err and the caller
/// falls back to direct dispatch. Generous (matches the server's own 30s) so a
/// legitimately slow servable command (e.g. `sync` over many files) is not cut
/// off. The whole-slice guarantee — correctness never depends on the daemon —
/// rests on this: a broken daemon degrades to a bounded wait then fallback,
/// never an infinite stall.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Connect to the daemon socket WITH the client timeout applied. The single
/// place client streams are born, so none can block forever on a stalled peer.
fn connect_client(sock: &Path) -> std::io::Result<UnixStream> {
    let s = UnixStream::connect(sock)?;
    let _ = s.set_read_timeout(Some(CLIENT_TIMEOUT));
    let _ = s.set_write_timeout(Some(CLIENT_TIMEOUT));
    Ok(s)
}

/// Send a drain request on a FRESH connection (the daemon serves one request
/// per connection and closes it, so the reply we already read on the prior
/// stream came on a now-closed socket — reusing it would write into the void).
/// Best-effort: ignore the ack's content; the caller then waits for the socket
/// to vanish as the real signal that the daemon retired and freed the lock.
fn drain(sock: &Path) {
    if let Ok(mut stream) = connect_client(sock) {
        let req = Request {
            build_id: binary_identity(),
            json: true,
            argv: vec!["__drain__".to_string()],
        };
        let _ = write_frame(&mut stream, &req);
        let _: Option<Reply> = read_frame(&mut stream).ok();
    }
}

/// Connect to an existing daemon, or lazily spawn one and wait briefly for its
/// socket. Returns `None` on any failure (⇒ fall back to direct).
fn connect_or_spawn(sock: &Path, root: &Path) -> Option<UnixStream> {
    if let Ok(s) = connect_client(sock) {
        return Some(s);
    }
    // No live daemon. A leftover socket from a crash refuses connections — drop
    // it so the fresh daemon can bind.
    let _ = std::fs::remove_file(sock);
    spawn_daemon(root)?;
    // Poll for the socket to appear and accept a connection.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(s) = connect_client(sock) {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

/// Spawn a detached `loom serve` on the current binary, pinned to `root` via
/// `--graph` so it serves the same graph the client resolved. Detached: its
/// stdio is discarded, and it gets its OWN session/process group (`setsid`) so
/// it OUTLIVES this short-lived client — without that, the daemon shares the
/// client's process group and dies the moment the client exits (it then squats
/// only as a stale socket and the next request re-spawns, killing all
/// amortization).
fn spawn_daemon(root: &Path) -> Option<()> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().ok()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--graph")
        .arg(root)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Don't let the daemon inherit LOOM_DAEMON (it must not try to be its own
    // client) — though `serve` is routed before the client check anyway.
    cmd.env_remove("LOOM_DAEMON");
    // Detach into a new session (`setsid`) so the daemon survives the client's
    // exit. SAFETY: in the forked child between fork and exec we call only the
    // async-signal-safe `setsid` and touch no shared state; a failure (e.g.
    // already a session leader) is non-fatal — the daemon still runs.
    unsafe {
        cmd.pre_exec(|| {
            extern "C" {
                fn setsid() -> i32;
            }
            // The libc symbol is linked by every Rust binary's C runtime — no
            // extra dependency needed for this one call.
            setsid();
            Ok(())
        });
    }
    cmd.spawn().ok().map(|_child| ())
}

/// Wait (briefly) for a socket file to disappear — used after a drain so we
/// don't race the retiring daemon for the lock.
fn wait_socket_gone(sock: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !sock.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::LoomDb;

    /// Round-trip a request through the actual frame I/O over an in-memory pipe
    /// — pins the wire format without a socket or a process.
    #[test]
    fn frames_round_trip_request_and_reply() {
        let (mut a, mut b) = std::os::unix::net::UnixStream::pair().unwrap();
        let req = Request {
            build_id: "abc".into(),
            json: true,
            argv: vec!["status".into(), "--json".into()],
        };
        write_frame(&mut a, &req).unwrap();
        let got: Request = read_frame(&mut b).unwrap();
        assert_eq!(got.build_id, "abc");
        assert!(got.json);
        assert_eq!(got.argv, vec!["status".to_string(), "--json".to_string()]);

        let reply = Reply::Output {
            stdout: "{\"ok\":true}".into(),
            exit_code: 0,
        };
        write_frame(&mut b, &reply).unwrap();
        match read_frame::<_, Reply>(&mut a).unwrap() {
            Reply::Output { stdout, exit_code } => {
                assert_eq!(stdout, "{\"ok\":true}");
                assert_eq!(exit_code, 0);
            }
            _ => panic!("expected Output"),
        }
    }

    /// A frame whose declared length exceeds the ceiling is rejected, not
    /// allocated — a corrupt/hostile length can't OOM the daemon.
    #[test]
    fn oversized_frame_length_is_rejected() {
        let (mut a, mut b) = std::os::unix::net::UnixStream::pair().unwrap();
        // Write a bogus 4-byte length header far over MAX_FRAME, then close.
        let huge: u32 = u32::MAX;
        a.write_all(&huge.to_be_bytes()).unwrap();
        drop(a);
        let r: std::io::Result<Request> = read_frame(&mut b);
        assert!(r.is_err(), "oversized length must be refused");
    }

    /// `binary_identity` is stable within a process (memoized) and non-empty.
    #[test]
    fn binary_identity_is_stable() {
        let a = binary_identity();
        let b = binary_identity();
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    /// The socket path is derived under `.loom/` of the resolved root.
    #[test]
    fn socket_path_lives_under_loom() {
        let p = socket_path(Path::new("/tmp/some/repo"));
        assert!(p.ends_with(".loom/daemon.sock"), "{}", p.display());
    }

    /// Full IPC contract against a REAL daemon loop on a REAL graph file: a
    /// servable command's daemon output is byte-identical to a direct
    /// dispatch, a non-json request falls back, a build-id mismatch is
    /// reported, and `__drain__` stops the loop. This is the parity guarantee
    /// the whole slice stands on.
    #[test]
    fn daemon_ipc_parity_fallback_skew_and_drain() {
        // A real on-disk graph (the daemon holds the lock on it). Initialise it
        // the way `loom init` does: create `.loom/`, open the store, stamp the
        // meta sentinel + indexes — then we capture the direct status oracle and
        // close the handle (drop) so the daemon can acquire the lock.
        //
        // The dir name is kept SHORT: a Unix socket path must fit in
        // `sockaddr_un.sun_path` (~104 bytes on macOS), and `.loom/daemon.sock`
        // already eats ~16 of those — a long temp path overflows SUN_LEN.
        let dir = std::env::temp_dir().join(format!("lm{:x}", uuid::Uuid::new_v4().as_u128() & 0xffffff));
        std::fs::create_dir_all(crate::db::loom_dir(&dir)).unwrap();
        {
            let db = GrafeoDb::open(&crate::db::db_path(&dir)).unwrap();
            for stmt in crate::db::schema::index_statements() {
                db.execute(&stmt).unwrap();
            }
            db.execute(&crate::db::schema::insert_meta(
                crate::db::schema::SCHEMA_VERSION,
                "t",
                &uuid::Uuid::new_v4().to_string(),
                "serve-test",
                "owned",
            ))
            .unwrap();
        }
        let direct_status = direct_status_json(&dir);

        // The exclusive file lock is process-level and released on handle drop,
        // but the OS may free it a beat after `direct_status_json` returns. Poll
        // until a fresh open succeeds so the daemon thread (same process) is
        // guaranteed to acquire the lock rather than racing the just-dropped
        // handle.
        let free_by = Instant::now() + Duration::from_secs(5);
        loop {
            match GrafeoDb::open(&crate::db::db_path(&dir)) {
                Ok(db) => {
                    drop(db);
                    break;
                }
                Err(_) if Instant::now() < free_by => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => panic!("graph lock never freed: {e}"),
            }
        }

        let sock = socket_path(&dir);
        // Run the daemon loop in a thread (short idle; we stop it via __drain__).
        let daemon_root = dir.clone();
        let daemon = std::thread::spawn(move || serve(&daemon_root, 10));
        // Wait for the socket to come up.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && UnixStream::connect(&sock).is_err() {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !sock.exists() {
            // The daemon thread must have errored out of serve() — surface why.
            let err = daemon.join().unwrap();
            panic!("daemon failed to bind its socket: {err:?}");
        }

        let my_id = binary_identity();

        // 1) PARITY: a servable json command's daemon output == direct output.
        let reply = client_send(&sock, &my_id, true, &["status".into(), "--json".into()]);
        match reply {
            Some(Reply::Output { stdout, exit_code }) => {
                assert_eq!(exit_code, 0);
                assert_eq!(
                    stdout, direct_status,
                    "daemon status must be byte-identical to direct"
                );
            }
            other => panic!("expected Output, got {:?}", other.map(|_| "reply")),
        }

        // 2) NON-JSON: human mode falls back.
        let reply = client_send(&sock, &my_id, false, &["status".into()]);
        assert!(matches!(reply, Some(Reply::Fallback)), "non-json must fall back");

        // 3) NOT-SERVABLE: validate is graph-releasing → fallback.
        let reply = client_send(&sock, &my_id, true, &["validate".into(), "--all".into()]);
        assert!(
            matches!(reply, Some(Reply::Fallback)),
            "validate must fall back (graph-releasing)"
        );

        // 4) SKEW: a wrong build id → version_mismatch.
        let reply = client_send(&sock, "not-my-id", true, &["status".into(), "--json".into()]);
        assert!(
            matches!(reply, Some(Reply::VersionMismatch)),
            "a build-id mismatch must report version_mismatch"
        );

        // 5) DURABILITY: a write THROUGH the daemon (own session over the shared
        // handle) must survive a CLEAN exit. Add an intent, then drain.
        let reply = client_send(
            &sock,
            &my_id,
            true,
            &[
                "intent".into(),
                "add".into(),
                "--name".into(),
                "served-intent".into(),
                "--level".into(),
                "feature".into(),
                "--description".into(),
                "a behavior added through the daemon to prove writes persist".into(),
                "--json".into(),
            ],
        );
        assert!(
            matches!(reply, Some(Reply::Output { exit_code: 0, .. })),
            "a servable mutation must run and report exit 0"
        );

        // 6) DRAIN: stop the loop cleanly (Drop → close → flush) and confirm the
        // socket is removed.
        let _ = client_send(&sock, &my_id, true, &["__drain__".into()]);
        daemon.join().unwrap().unwrap();
        assert!(!sock.exists(), "drain must remove the socket on exit");

        // The daemon exited cleanly — its write must now be on disk (read direct).
        let db = GrafeoDb::open(&crate::db::db_path(&dir)).unwrap();
        let names = crate::db::queries::list_intents(&db, None, None).unwrap();
        drop(db);
        assert!(
            names.iter().any(|i| i.name == "served-intent"),
            "a write made through the daemon must persist after a clean exit"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Open the graph directly and capture `status --json` exactly as the CLI
    /// would — the parity oracle for the daemon path.
    fn direct_status_json(root: &Path) -> String {
        let db_file = crate::db::ensure_initialized(root).unwrap();
        let db = GrafeoDb::open(&db_file).unwrap();
        let printer = Printer::capturing(true);
        crate::commands::status::run_with_db(&db, root, &printer).unwrap();
        printer.captured().unwrap()
    }

    /// One raw client exchange used by the IPC test: connect, send a request
    /// with the given identity/json/argv, read one reply.
    fn client_send(sock: &Path, build_id: &str, json: bool, argv: &[String]) -> Option<Reply> {
        let mut stream = UnixStream::connect(sock).ok()?;
        let req = Request {
            build_id: build_id.to_string(),
            json,
            argv: argv.to_vec(),
        };
        write_frame(&mut stream, &req).ok()?;
        read_frame(&mut stream).ok()
    }
}
