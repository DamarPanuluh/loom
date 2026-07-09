---
name: loom-driver
description: Drive the loom CLI as a solo operator for a repo's living intent graph. Use whenever the user mentions loom, intent graphs, loom.graph.json, `.loom/`, mapping code to behavior, `loom sync`, `loom next`, quality verdicts, validations, journeys, hypotheses, completeness, elaborate, scan adapters, coverage, smells, calibrate, thresholds, bootstrap, explain, or systematic codebase understanding/refactor tracking.
---

# Drive loom (thin skill — do not duplicate recipes)

loom routes; you inspect code and record evidence through typed graph writes.
Never record a verdict you did not earn by inspection.

## Canonical sources (read these; do not invent commands)

| Need | Source |
|---|---|
| Driving loop, roles, write-back, stop conditions | `docs/llm-driver.md` in the loom checkout, or `loom guide --json` |
| Shipped CLI surface | `docs/commands.md` or `loom --help` / `loom <cmd> --help` |
| Vocabulary | `docs/terminology.md` or `loom schema` |
| This repo's local facts (build/test) | repo `AGENTS.md` / `CLAUDE.md` if present — facts only, not workflow |

**Sync this file to `~/.agents/skills/loom-driver/SKILL.md` after pulling loom** so agent harnesses pick up the thin skill. Do not paste command recipes into the home copy — they drift.

## Session start

```bash
loom status --json
# or: loom session --json
```

- Graph missing but `loom.graph.json` exists: `loom init . && loom import loom.graph.json && loom sync --json`
- No graph yet: `loom detect --json`, `loom init .`, then `loom guide` (and `loom bootstrap suggest` when codefiles are registered and intents are empty)
- Existing graph: `loom sync --json` after code changes, then `loom next --json` (or `loom next --all --json`)
- Free-form topic: `loom door "<utterance>" --json` → pick one landing → `loom inbox mark <id> routed --reason '…'`

`loom next` is the work router. Do not invent queue priority.

## Intake routing (binary-enforced)

| Input | Command |
|---|---|
| Human/external free-form | `loom door` / `loom inbox add` (`--source human\|external\|support\|import` only) |
| Evidence-backed observation | `loom finding add` |
| Product decision | `loom question add "…" --intent <intent>` |
| Structured plan | `loom proposal add` |
| Falsifiable redesign claim | `loom hypothesis add` |
| Timeboxed investigation | `loom task add` |

Never use `inbox add --source question` or `inbox add --source code_audit` — the binary rejects them.

## Honesty floor

- Criterion = what would falsify the claim. Evidence = file:line or runtime output you actually read.
- Confidence below 0.7 is legitimate (routes to review). A confident guess corrupts the graph.
- Fixer lane: repair + `loom sync` only — never record the passing verdict from the fixer hat.
- Capture out-of-band findings with `loom finding add`; product questions with `loom question add`.
- Mechanical residue (`routing_hint: mechanical` / cheap re-confirm) may be batch-reaffirmed via `loom apply` verdicts; judgment packets stay one-at-a-time.

## Hard cuts

- Prefer `loom guide --json` / `--help` over memorized flags.
- No new workflow invented outside loom's queues while a loom loop is active.
- Statistical `loom debt` is advisory — never treat it as required work.
- No new CLI families until cheap-residue routing + hybrid relates ripple keep analyze depth economical (see `docs/build-plan.md` LLM-fidelity hard cuts).
- After structural sync on a new graph: `loom calibrate --write` before mass finding triage. Justified/rejected findings stay settled across hash churn unless the metric worsens past the band.
- Structural size/complexity findings from `loom next --mode triage` are **judgment** packets — name one concern or mark `needed` with a split plan. Dogfood may record those verdicts via the binary, but a bash case table is not a substitute for the triage prompt contract.
