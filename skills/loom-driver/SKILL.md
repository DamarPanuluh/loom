---
name: loom-driver
description: Drive a full loom batch session — drain every autonomous queue packet by packet, batch the human-gated remainder into one sitting, checkpoint and export, and end with a before/after ladder report. Use in a repository with a routed loom graph.
disable-model-invocation: true
---

# loom-driver

One invocation is one campaign: drain the loom graph until every remaining
queue is human-gated, then settle the remainder with the human in one sitting.

This file owns only the campaign layer — strategy, pacing, gate batching,
checkpoint cadence, the end report. Everything inside one work packet (role,
allowed actions, evidence, write-back, stop condition) is owned by loom itself:

**The served packet outranks this file. On any conflict, obey the packet.**

## Install

This file ships inside the loom repository but is not auto-discovered there.
Activate it by symlinking (or copying) into a skill root:

```bash
ln -s "$(pwd)/skills/loom-driver" ~/.claude/skills/loom-driver
```

## Preconditions

- The `loom` binary on PATH (`loom --version`).
- An initialized graph (`.loom/`). This skill never runs `loom init`; a repo
  with no graph is not driveable — set it up first (`loom init`, register
  codefiles, seed meaning — `/grill-with-loom` if available).

## Orient

```bash
loom welcome
loom guide
loom sync
loom --json status     # record this ladder — it is the "before" of the report
```

## Choose the strategy by graph state

**Cold graph** — no authored Journey roots, nothing routed: stop early,
honestly. Run `loom bootstrap suggest` for non-authoritative clues, then tell
the human: seeding is interactive product work (authoring Journeys,
`derive-accept`) that needs them present — use `/grill-with-loom` or the
interactive seeding loop, and re-invoke this skill once the graph routes work.
Grill seeds; driver drains. Do not author product meaning in batch mode.

**Warm graph** — routed work exists: drain.

## The drain loop

```bash
loom sync
loom --json status
loom --json next
```

For each served packet:

1. If the packet is human-gated — `mode: ratify`, a non-null `human_gate`, or
   a contract that says ask-and-WAIT — do not work it and do not wait on it:
   it belongs to the end sitting. Pull autonomous work explicitly instead with
   `loom next --mode <lane>` (build, elaborate, fix, validate, quality,
   analyze, prove, triage, review) until those lanes are dry. This deferral is
   the one place this file overrides the packet's "wait": the wait happens at
   the end, batched, not mid-drain.
2. Export `LOOM_AGENT=llm:<owner_role>` to match the packet's `owner_role`
   before writing.
3. Obey the packet's `prompt_contract` exactly: its allowed and forbidden
   actions, required evidence, write-back command, and stop condition.
4. Out-of-scope observations: capture with `loom finding add` (evidence-backed,
   file:line), then return to the packet. Never silently fix, never drop.
5. After the write-back: `loom sync`, back to `loom status`.

Repeat until the autonomous queues are empty — including work the drain itself
creates (new findings, fix items after edits). Lanes that drain alone: build,
elaborate, fix, validate, quality, analyze, prove, triage, review.

### Mechanical residue: the sanctioned batch path

```bash
loom next --mode <m> --all --json
```

Select items where `routing_hint == mechanical` (or `cause_class == cheap`)
and close them with **one** `loom apply` batch reaffirming prior criteria with
fresh evidence. Judgment items go one at a time, each with fresh reading.

### Pacing — this is a hard rule

No parallel subagents for judgment work, and stay under **10 asserted
judgment writes per minute**. Loom's audit flags faster bursts by one actor as
judgment compression unless a human-gated batch authorization envelope covers
them — honest inspection takes time, and the audit cannot tell fast from fake.
Structural detector findings are always judgment triage; never batch-reaffirm
them as mechanical.

### Human gates met mid-drain

Never block on one and never answer for the human:

- A product decision → `loom question add "…" --intent <intent>`, move on.
- A ratify/reject/redefine candidate found outside a served ratify packet →
  `loom judgment propose …` with evidence, move on.
- A blocked proof → record it blocked with the reason, move on.

Silence is never an answer. The remainder is settled at the end, batched.

## Checkpoints

After one Intent or cohesive bundle is proven and synced:

```bash
loom checkpoint recommend --intent <intent> --json
```

If the recommendation is ready and exact: `git add -- <included_paths only>`,
verify the staged set matches exactly, commit with the suggested message, and
**leave the commit local**. On blocked, ambiguous ownership, user-owned
overlap, or staged-set drift: defer — never guess, never widen, never
`git add -A`. Pushing always requires an explicit human decision bound to the
exact repository, remote, branch, and commit; never push on your own.

## The end: one sitting, then the report

When every remaining queue is human-gated (or the graph is fully green):

```bash
loom export
```

**If the human is present**, settle the remainder in one batched sitting:

```bash
loom session
loom question list --status open
loom judgment digest
```

Present each item with a recommendation and consequences, wait for the answer,
and record the human's *exact* words through the mediated write-backs
(`loom question answer …`, `loom judgment confirm <id> --human-decision '…'`,
ratify packets' own commands). Never paraphrase an answer into the gate.

**If the human is absent**, do not wait and do not infer: print the remainder
with the exact command for each item, and exit.

Finish with the report: the ladder before vs. after, what closed per lane,
what was checkpointed, and what still awaits the human. Then stop — this
skill is one-shot; another campaign is another invocation.
