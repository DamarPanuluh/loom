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
- **⑥b DB-acquisition refactor** — commands lend a held handle; direct path
  injects a freshly-opened one (behavior-identical today).
- **⑥c `loom serve`** — socket loop + transport + build-id handshake + drain.
- **⑥d routing + lifecycle** — CLI connect→route|spawn|fallback; idle timeout;
  stale cleanup.
- **⑥e tests** — parity (daemon result == direct result, per command), latency
  (vs the open floor), version-skew (binary swap retires the stale daemon).

## Test plan (⑥e)

- **Parity**: for a sample of commands, `loom <cmd>` via daemon == via direct,
  byte-for-byte (stdout + exit).
- **Latency**: median round-trip via daemon vs direct-open, on the live graph.
- **Skew**: start daemon at build A; swap binary to build B; next command must
  detect the mismatch, drain A, and serve from B (assert the served build id).
- **Concurrency**: extend the pinned in-process contract to the real IPC path.
