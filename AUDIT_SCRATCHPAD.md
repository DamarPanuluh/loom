# loom Exhaustive Code Audit — Coverage Matrix

**Status:** COMPLETE — all 66 tracked files accounted for.
**Findings document:** `AUDIT_FINDINGS.md` (final deliverable)

---

## Coverage Summary

| Category | Files | Audited | Excluded | Clean | Files w/issues |
|---|---|---|---|---|---|
| src (Rust) | 37 | 37 | 0 | 19 | 18 |
| tests (Rust) | 10 | 10 | 0 | 0 | 10 |
| docs (Markdown) | 10 | 9 | 1 | 9 | 0 |
| scripts | 2 | 2 | 0 | 2 | 0 |
| config | 6 | 6 | 0 | 6 | 0 |
| data | 1 | 1 | 0 | 0 | 1 |
| **Total** | **66** | **65** | **1** | **36** | **29** |

## Per-File Coverage Matrix

### Config & Entry — audited by main agent

| File | Status |
|---|---|
| Cargo.toml | ✅ clean |
| Cargo.lock | ✅ audited (version coherence) |
| WATCHDOG.yml | ✅ clean |
| .gitignore | ✅ clean |
| README.md | ✅ clean |
| CHANGELOG.md | ✅ clean |

### Source — Core models — audited by subagent core-models

| File | Status | Findings |
|---|---|---|
| src/lib.rs | ✅ clean | None |
| src/main.rs | ✅ info | I-1: unsafe SIGPIPE (intentional) |
| src/cli.rs | ✅ clean | None |
| src/cli/subcommands.rs | ✅ audited | M-15: ValidationType enum unwired |
| src/model.rs | ✅ audited | I-5: ValidationType dead code |
| src/registry.rs | ✅ audited | H-5: exposes uniqueness prevents dual truth-class |
| src/store.rs | ✅ audited | H-1, H-2, H-3, H-4, M-11, M-12, M-13, M-14, M-16, M-17 |

### Source — Pipeline — audited by subagent pipeline

| File | Status | Findings |
|---|---|---|
| src/truth.rs | ✅ clean | None |
| src/maturity.rs | ✅ audited | M-6: lifecycle strings not validated |
| src/completeness.rs | ✅ clean | None (audited by main agent, full read) |
| src/prescan.rs | ✅ clean | None |
| src/scan.rs | ✅ audited | M-8: adapter exit status not checked |
| src/signal.rs | ✅ audited | H-7, M-9, L-2, L-3 |
| src/packs.rs | ✅ clean | None |
| src/sync.rs | ✅ audited | H-6, M-10 |
| src/journey.rs | ✅ audited | H-8 |
| src/travel.rs | ✅ audited | M-7, L-1 |
| src/fsglob.rs | ✅ clean | None |

### Source — Commands — audited by subagent commands

| File | Status | Findings |
|---|---|---|
| src/commands.rs | ✅ clean | Dispatch exhaustive |
| src/commands/intent.rs | ✅ audited | H-9, H-11 |
| src/commands/edge.rs | ✅ audited | M-3 |
| src/commands/codefile_cmd.rs | ✅ clean | None |
| src/commands/status_cmd.rs | ✅ clean | None |
| src/commands/diagnostics_cmd.rs | ✅ clean | None |
| src/commands/proof_cmd.rs | ✅ clean | None |
| src/commands/proposal_cmd.rs | ✅ clean | None |
| src/commands/domain_cmd.rs | ✅ audited | M-4, M-5 |
| src/commands/journey.rs | ✅ audited | H-13 |
| src/commands/misc_cmd.rs | ✅ audited | H-10 |
| src/commands/pulse.rs | ✅ clean | None |
| src/extract/mod.rs | ✅ clean | None |
| src/extract/rust.rs | ✅ clean | None |
| src/extract/langs.rs | ✅ clean | None |
| src/workitem/mod.rs | ✅ audited | M-1 |
| src/workitem/queues.rs | ✅ audited | H-12 |
| src/workitem/context.rs | ✅ clean | None |
| src/workitem/contracts.rs | ✅ audited | M-2 |

### Tests — audited by subagents tests-1, tests-2, tests-3

| File | Status | Findings |
|---|---|---|
| tests/ring1.rs | ✅ audited | L-5, L-15 |
| tests/ring2.rs | ✅ audited | L-5, L-15, T-1 |
| tests/ring3.rs | ✅ audited | L-5, L-15, T-2, T-3 |
| tests/ring4.rs | ✅ audited | L-5 |
| tests/ring5.rs | ✅ audited | L-5, L-14, L-16 |
| tests/ring6.rs | ✅ audited | L-5, L-14, L-17 |
| tests/ring7.rs | ✅ audited | L-5 |
| tests/ring8.rs | ✅ audited | L-5 |
| tests/ring9.rs | ✅ audited | L-5, T-4 |
| tests/ring10.rs | ✅ audited | L-5, I-4 |

### Docs — audited by main agent

| File | Status |
|---|---|
| docs/README.md | ✅ clean |
| docs/terminology.md | ✅ clean |
| docs/graph-model.md | ✅ clean |
| docs/state-machine.md | ✅ clean |
| docs/llm-driver.md | ✅ clean |
| docs/commands.md | ✅ clean |
| docs/build-plan.md | ✅ clean |
| docs/design.md | ✅ clean |
| docs/wiki-projection.md | ✅ clean |
| docs/scratchpad.md | ⏭️ excluded (non-canonical) |

### Scripts — audited by main agent

| File | Status |
|---|---|
| scripts/release.sh | ✅ clean |
| scripts/dogfood.sh | ✅ clean |

### Data — audited by main agent (programmatic validation)

| File | Status | Findings |
|---|---|---|
| loom.graph.json | ✅ audited | M-18, L-4, I-2, I-3 |

---

## Exclusions

| File | Reason |
|---|---|
| docs/scratchpad.md (2412ln) | Non-canonical per docs/README.md |

## Limited-Scope Audits

| File | Scope |
|---|---|
| Cargo.lock | Version coherence verified (loom 0.13.0, all deps resolve, serde_yaml 0.9.34+deprecated noted). Not line-audited — auto-generated lock file. |
## Issue Count Summary

| Severity | Count |
|---|---|
| HIGH | 13 |
| MEDIUM | 18 |
| LOW | 17 |
| INFO | 5 |
| TEST | 4 |
| **Total** | **57** |

All issues detailed in `AUDIT_FINDINGS.md`.
