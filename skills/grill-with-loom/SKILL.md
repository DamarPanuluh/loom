---
name: grill-with-loom
description: A relentless interview to sharpen a plan or idea, writing every outcome into the loom graph as it settles — intents, vocabulary, open questions, and decisions. Use when the user wants to stress-test a plan in a loom-managed repository.
disable-model-invocation: true
---

# Grill with loom

Interview the user relentlessly about a plan, decision, or idea until you reach
a shared understanding — and record every outcome in the loom graph the moment
it settles. The graph is the only record: no CONTEXT.md, no ADR files.

## Install

This file ships inside the loom repository but is not auto-discovered there.
Activate it by symlinking (or copying) into a skill root:

```bash
ln -s "$(pwd)/skills/grill-with-loom" ~/.claude/skills/grill-with-loom
```

## Preconditions

- The `loom` binary must be on PATH (`loom --help`).
- The repository must have a graph. If `.loom/` is missing, offer `loom init .`
  and wait for an explicit yes before running it. `init` creates files in the
  user's repository; never run it unasked.

## The interview protocol

Map the subject as a **design tree**: every decision branches into the
decisions that hang off it. Work the tree in **rounds**. The **frontier** is
every decision whose prerequisites are already settled — the questions you can
ask *now* without guessing at answers you have not heard yet. Ask the whole
frontier in one round; then wait for the user's answers before the next round.

Format each question like so:

```
❓ **Q1** - **<question title>**: <question body, may include choices>

➡️ <your recommended answer>
```

Each answered round reshapes the tree — settled decisions push the frontier
outward. Recompute the frontier and ask the next round. A question whose answer
depends on another question still open this round belongs to a *later* round.

Finding **facts** is your job, never the user's. When a frontier question needs
a fact from the environment (the code, loom's docs, the graph), look it up
yourself — `loom find`, `loom explain`, `loom context`, or the repository
itself. The **decisions** are the user's — put each to them and wait.

The session is done when the frontier is empty: every branch visited, nothing
silently assumed.

## Round 1: mint the root

The grill always has a subject; the subject **is** the root intent.

1. Check for an existing node first: `loom find --exact "<name>"`.
   `loom apply` intent creation is create-only — re-declaring an existing name
   is rejected and rolls back the entire batch.
2. If absent, mint it as a planned feature intent in the first write-back
   batch (see below). All open questions hang on this root.

Free-form stray thoughts that do not fit the tree go through `loom door`, not
into invented nodes.

## Write-back: inline, every round

After each answered round, write what settled. Do not batch to the end — if
the session dies, nothing may be lost.

**One `loom apply` batch per round** for structure, sections in dependency
order (`vocab` first, then `intents`, then `tags`):

```jsonc
{
  "vocab":   [ { "term": "…", "why": "…" } ],
  "intents": [ { "name": "…", "description": "…", "level": "feature",
                 "lifecycle": "planned" } ],
  "tags":    [ { "intent": "…", "terms": ["…"] } ]
}
```

**Questions** are first-class and must anchor on an intent:

- A frontier question the user has not answered yet, when the session pauses:
  `loom question add "<question>" --intent <root>`
- The moment the user settles one that was recorded:
  `loom question answer <key> --answer "<answer>"`
  Skipping the answer step leaves questions open forever and the completeness
  `questions` axis never closes.

**Decisions**: record `loom decide` only for a real reversal — all three must
hold: a genuine tradeoff existed, it is hard to reverse, and a future reader
would be surprised without the reasoning. The grill supplies the parts:

```bash
loom decide "<what the user chose>" \
  --instead-of "<the rejected option>" \
  --because "<the user's reason>" \
  --about "<the behavior or file it concerns>"
```

Small answers are not decisions — they just shape intents, vocabulary, and
tags. Decision spam buries the real reversals.

After each round's writes: `loom sync`.

## Ending the session

When the frontier is empty, state the shared understanding, confirm it with
the user, and stop. Print the handoff — do **not** run it:

```
The grill is done and recorded. When you want loom to grow this idea's
forgotten surroundings (missing scenarios, sad paths, prerequisites), run:

    loom next --mode elaborate
```

Loom's elaborate lane is the continuation of this interview, driven by the
graph instead of the model. It will still be there tomorrow.
