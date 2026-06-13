# `loom serve` — daemon design

Status: **greenlit (opt-in), being built in slices.** This is the spec the slices
implement against. Motivation: every `loom` invocation pays a ~36–100 ms DB-open
floor (grafeo loading the store); a multi-agent run makes thousands of calls, so
that floor dominates. A persistent process that holds the graph open amortizes
it to ~0 *and* unlocks safe concurrent multi-agent access (grafeo's in-process
MVCC — pinned by `tests/grafeo_probe.rs::daemon_contract_concurrent_sessions_persistent`:
4 writer + 4 reader sessions on one persistent handle, zero errors, snapshot
isolation holds, concurrent writes all commit).

## Principles

- **Opt-in, never default.** The orchestrator/agent starts `loom serve` for a
  heavy multi-agent session. One-shot interactive use stays direct-open. No
  daemon running ⇒ no version-skew surface ⇒ casual use is unaffected.
- **Optional performance layer, never a correctness dependency.** If anything is
  off (can't connect, version mismatch unresolved, crash, disabled), the CLI
  falls back to *exactly today's direct-open behavior*. Worst case = lose the
  speedup, never correctness.
- **Per-graph, lock-enforced singleton.** One daemon per graph, keyed by the
  resolved graph root. grafeo's exclusive file lock (both `rw+rw` and `rw+ro`
  refused — probe 7) guarantees at most one opener, so the daemon *is* the
  singleton by construction. Different repos = different graphs = different
  daemons = fully independent (no cross-talk; they never contended even today).

## Concurrency model

The daemon opens the graph ONCE (holds the lock + one `GrafeoDB` handle) and
serves many client requests against it. grafeo's MVCC snapshot isolation makes
concurrent reads + writes from separate sessions safe in-process (proven). So a
single daemon handles a whole agent fleet on one graph.

## Version-skew safety (the #1 risk)

Replacing the global binary while a daemon runs is the dangerous case: the stale
daemon holds the lock AND serves old logic, and the new binary can't open the
graph directly (lock held) — so it'd be forced to talk to stale logic.

Mitigation:
1. **Build-id handshake — NOT the version string.** The crate version is
   permanently "0.1.0"; identity comes from the git build stamp (`build.rs` →
   `LOOM_BUILD`, shown in `loom --version`). The daemon records its build id at
   startup; every client carries its own. *(Slice ⑥a — done.)*
2. **Auto-replace on mismatch.** Build-id differs ⇒ the client sends the daemon
   a drain-and-exit (finish in-flight, commit, **release the lock**, exit),
   waits for the lock to free, then spawns a fresh daemon on the new binary. The
   first command after a binary swap transparently retires the stale daemon.
3. **Fallback.** If replacement can't complete, the client opens the graph
   directly (once the lock frees) — never blocks on the daemon.

## Lifecycle

- **Lazy spawn**: first daemon-mode command with no live daemon spawns one.
- **Idle timeout**: a daemon idle for N minutes drains + exits, releasing the
  lock — never squats on a repo's graph.
- **Stale cleanup**: a crashed daemon leaves a dead socket; the client detects
  connect-refused + a stale lock, cleans up, spawns fresh.

## Transport & protocol (slice ⑥c)

- Unix domain socket, path derived from the graph root (e.g. `.loom/daemon.sock`).
- Client → daemon: `{build_id, argv}`. Daemon runs the command against its held
  handle, captures stdout/stderr/exit, returns them; client replays verbatim.
- Connect → handshake (build-id) → route | spawn | fallback. State machine lives
  in the CLI entry path.

## The prerequisite refactor (slice ⑥b) — DONE

Every command's `run()` used to call `GrafeoDb::open` itself — which would fail
inside the daemon (the graph is already open/locked). So 28 single-handle
commands were split into `run_with_db(db: &GrafeoDb, root, …)` (the body) + a
thin `run(…)` wrapper that opens-and-delegates. `run()` signatures are
unchanged, so the dispatcher is untouched. Behavior-identical (verified: build,
186 + 178 tests, every read command, a scratch write-persist test).

**NOT daemon-servable — `validate` and `saga execute` are excluded** (left
unsplit, on their original single-handle open/drop/reopen flow). These commands
DELIBERATELY `drop` the graph mid-run to **release the lock** while running an
external validation/saga (which may itself invoke `loom`); a lent, held-open
handle would defeat that release and deadlock. They open at most one handle at a
time and manage their own lifecycle. The daemon routes only the hot read /
single-handle commands; graph-releasing commands run direct. (Lesson: a fan-out
that mechanically split these introduced two simultaneous handles — caught by
independent runtime review, not by the test suite, since the command layer is
barely unit-tested. See [[proven-axis-overstatement]].)

## Slice plan

- **⑥a build-identity** — `build.rs` git stamp in `loom --version`. *(done)*
- **⑥b DB-acquisition refactor** — 28 commands lend a held handle. *(done)*
- **⑥c–e `loom serve` + routing + tests** — `src/serve.rs`: Printer capture,
  threaded socket serve loop dispatching `run_with_db` against an `Arc<GrafeoDB>`
  handle (per-connection sessions — the proven MVCC pattern), exe-content-hash
  skew handshake + drain/respawn, lazy-spawn/idle-timeout lifecycle, and the
  client connect→route|spawn|fallback path. *(done — built via workflow, then
  hardened on two review-found defects, see below)*

## ⑥c verification + the defects independent review caught

Built via an adversarial workflow (build → review on fallback/skew/concurrency
lenses + live smoke). Parity is **byte-identical** (servable `--json` via daemon
== direct, same SHA256); per-request capture buffers and per-thread sessions are
race-free (review-confirmed). The review + smoke caught **two correctness
defects the 191-test suite missed** (the command layer is barely unit-tested):

1. **Client had no read timeout** → a wedged daemon (accepts, never replies)
   hung the client forever instead of falling back. Fixed: `connect_client`
   applies a 30s read/write timeout to every client stream → timeout → Err →
   fallback. The "never a correctness dependency" guarantee rests on this.
2. **Human-mode / non-served under a live daemon broke** (`loom status` with no
   `--json` → "locked by another process") — the direct path bailed without
   freeing the daemon's held lock. Fixed: `try_client` now, on ANY direct path
   (human, non-json, `LOOM_DAEMON` unset), drains a *live* daemon first (a
   crashed one already released the lock via OS flock-on-death). Verified live:
   human `status` under a daemon drains it and runs direct, rc=0.

## Known limitations (acceptable for an OPT-IN layer; the direct default is unaffected)

- **Durability on hard kill**: writes via the daemon flush to disk on CLEAN exit
  (drain / idle-timeout / normal shutdown) — and a clean drain fires on ANY
  direct access, so writes flush frequently. A hard SIGKILL with unflushed
  writes loses them (same risk class as killing the single-process CLI
  mid-write). Future hardening: flush after each mutating request. Verified:
  write-via-daemon → drain → persisted on disk.
- **Skew boot-window**: `binary_identity` is frozen once at daemon startup; if
  the binary is swapped in the sub-second window *between exec and that read*,
  identity binds to the new bytes while running old logic. Narrow; fallback
  covers correctness.
- **Deep repo roots**: `.loom/daemon.sock` can exceed the unix `sockaddr_un`
  length (~104 bytes on macOS) under a very deep root → bind fails → client
  falls back to direct.

## Test plan (⑥e)

- **Parity**: for a sample of commands, `loom <cmd>` via daemon == via direct,
  byte-for-byte (stdout + exit).
- **Latency**: median round-trip via daemon vs direct-open, on the live graph.
- **Skew**: start daemon at build A; swap binary to build B; next command must
  detect the mismatch, drain A, and serve from B (assert the served build id).
- **Concurrency**: extend the pinned in-process contract to the real IPC path.
