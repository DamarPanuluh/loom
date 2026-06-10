---
name: run-loom
description: Build, run, smoke-test, and drive the loom intent-graph CLI — use when asked to run loom, build loom, test loom, verify the loom binary, or drive loom against a codebase (map intents, sync, quality verdicts, export).
---

# Run loom

loom is a Rust CLI (embedded grafeo graph DB, statically linked) whose primary
user is an LLM agent: it maintains a falsifiable intent graph of a codebase.
There is no GUI and no server — the "interaction surface" is the CLI itself,
so the harness is a lifecycle smoke driver plus the driving protocol below.
All paths are relative to the repo root.

## Prerequisites

Rust toolchain (`cargo`). Nothing else — grafeo is a pure-Rust crate compiled
in. First build is slow (several minutes); incremental builds are seconds.

## Build + smoke (agent path — run this first)

```bash
.claude/skills/run-loom/driver.sh
```

Builds `target/debug/loom`, then drives one complete brownfield lifecycle in a
throwaway repo: detect → idempotent init → map intents **by name** → ground →
lane/evidence-gate rejections → content-hash sync ripple (touch-only flags
NOTHING; a real change flags + notes the cause on the edge) → 360° unmeasured
queue + one-command `rule verdict` (creates the edge) → ISO 5055 verdict →
ripple re-flags the verdict → a validation that *invokes loom itself*
(DB-lock regression) → a `blocked` proof (reason gated, queue silent) →
`next --all` closeout → positional export + `--check` commit guard (fresh
passes, drift fails) → import into a fresh graph → doctor + graph-pin (LOOM_GRAPH beats a foreign cwd). 18 checks; exits
non-zero on the first broken link. `LOOM_BIN=/path/to/loom` skips the build.

Unit/regression suite (71 tests, in-memory DB, fast):

```bash
cargo test
```

Install on PATH (release build):

```bash
cargo install --path .
```

## Drive loom against a real codebase (what loom is *for*)

The binary teaches the protocol itself — read these before driving:

```bash
loom guide      # the full driving loop for the detected mode
loom schema     # data model: nodes, edges, states, field owners
```

The loop, compressed: `loom init .` → seed intents → seed the quality packs
`loom detect` recommends (`loom rule seed
iso5055|mobile|web-ui|service|data|concurrency` — the 360° vantage points;
iso5055 always applies) → ground to files → then
repeat **`loom status` → do what the compass says → `loom sync` after ANY code
change**. Work queues per agent role:
`loom next --mode build|discovery|fix|validate|quality`; the cross-role
closeout is `loom next --all` (every queue + gaps + doctor in one list).
The pulse footer's second line is the **360° coverage vector** (grounded ·
realized · explored · measured · proven) — the compass routes to the weakest
axis; never-measured rule×intent pairs feed the quality queue and resolve in
ONE `loom rule verdict` (it creates the edge; component altitude covers
descendants; `independent` = measured, doesn't apply).
Derived problems: `loom smells`; per-file ownership: `loom codefile show`.
Proofs that can't run yet: `loom validation mark <id> --result blocked
--reason "…"` (honest, out of the queue, visible in `loom report`).
Bulk re-verification after a big sync: `loom batch <file>` with one JSONL
verdict per line (ground/issue/independent/rule_verdict) — same gates per line
(write the file first; never pipe loom-generated output into `loom batch -`,
the DB lock is exclusive and both pipe ends start concurrently).
Pin a session to one repo's graph: `export LOOM_GRAPH=<path>` (or `--graph`)
— every loom call then hits that graph regardless of cd mistakes.
Hostile-import fuzz: `.claude/skills/run-loom/fuzz_import.sh` (corrupted
exports rejected, no partial graphs). Porting: `loom guide --mode port`.
Ship the graph: `loom export` (commit `loom.graph.json`, gitignore `.loom/`);
guard freshness with `loom export --check` (non-zero on drift — pre-commit/CI).

Multi-agent: set `LOOM_AGENT=llm:builder|analyzer|fixer|validator|quality`
per agent — lanes are enforced. Unset (bare `llm`) = solo mode, all lanes pass.

## Gotchas (all hit for real)

- **One process at a time per `.loom/`.** The grafeo session is exclusive;
  a second concurrent loom process fails with `GRAFEO-X001 … locked`.
  Sequential commands are fine. Validation commands MAY invoke loom (the
  session is released while they run) — but don't parallelize loom itself.
- **Sync detects CONTENT changes, not timestamps.** `loom sync` hashes file
  bytes (`content_hash`): a `touch`/checkout that doesn't change bytes flags
  nothing, and a byte change is caught even within the same second. Only a
  never-hashed graph (pre-upgrade) falls back to mtime once.
- **Quote globs.** `loom codefile add 'src/**/*.rs'` — unquoted, the shell
  expands and only the first file is registered.
- **Evidence gates bite.** `--criterion "todo"` or anything <10 chars /
  placeholder-like is rejected on purpose. Write the real falsifiable claim.
- **Lane violations are errors, not warnings.** With `LOOM_AGENT=llm:builder`
  set, `edge explore … ground` fails — grounding is analyzer/fixer work. The
  error names the right lane and queue.
- **Name resolution refuses to guess.** Intent/rule keys accept id, exact
  name, or a *unique* name fragment; an ambiguous fragment errors listing
  the candidates.
- **`loom import` only restores into a fresh `loom init`** — never merges.
  Run `loom sync` after import to reconcile with local files.

## Troubleshooting

- `GRAFEO-X001 … database file is locked` → another loom process holds this
  `.loom/`; wait for it or kill it. (Inside a validation command this was a
  bug, fixed: validate releases the session around command execution.)
- `Lane violation: 'llm:X' cannot …` → you're role-scoped; hand the action to
  the named lane or unset `LOOM_AGENT` for solo mode.
- `schema version mismatch` from `loom doctor` → graph predates this binary's
  schema; rebuild via `loom export` (old binary) + fresh `loom init` +
  `loom import`, or re-map.
- `--criterion must be substantive` → the gate, not a bug; write the claim.
