# loom v2 — State Machine

Status: canonical draft. This describes how loom tracks change, propagates staleness, and routes work. Terminology follows `terminology.md`; graph model follows `graph-model.md`.

---

## Foundation

loom is not a linear workflow engine. It is a **truth synchronization system**.

Change can enter from any surface:

```text
human utterance
LLM code edit
LLM or human wiki edit
code change outside loom
validation result
external contract or API change
graph import (porting, federation)
signal confirmation from debt feed
```

These are all **events**. Each event has a canonical owner — the fact type that holds authority for that information. The state machine classifies the event, updates the owner plane, evaluates dependencies, propagates staleness, and routes the next work.

**This is not a phase system.** Greenfield, brownfield, refactor, porting, and external-ripple are entry modes into the same event/ripple engine, not separate linear lifecycles.

---

## Fact ownership

Every fact type has a canonical owner. A change to one expression of truth must either update the canonical owner or surface as a routed inconsistency. It must never silently drift.

| Fact type | Canonical owner | Changed by |
|---|---|---|
| Behavior meaning | `Intent` | builder (loom intent update) |
| Code reality | Filesystem + derived `CodeFile` facts | sync |
| Proof result | `Validation` result / `validates` edge | validator / sync on code change |
| Quality judgment | `governs` edge verdict | quality role |
| Interface contract | `InterfaceSurface` / contract artifact | sync (derived) / builder (asserted) |
| Rationale / decision | `Note` | any role |
| Generated docs | `WikiProjection` + `WikiManifest` | wiki_author / wiki_reviewer |
| Statistical suspicion | `DebtCluster` (computed, not stored) | computed on demand |
| Raw input | `InboxItem` | capture before normalization |
| Operational work | `TaskRecord` | any role |

A wiki prose edit is not a change to behavior meaning unless it is routed as a graph delta through `InboxItem` normalization and accepted.

---

## The core loop

```
event enters
  → classify source and fact type
  → normalize to typed delta / proposal / signal
  → apply to canonical owner plane
  → evaluate dependencies
  → propagate staleness (ripple)
  → route queues
  → LLM or human acts (see llm-driver.md)
  → write-back recorded in graph
  → loom validates transition
  → export / wiki pages settle or remain stale
  → next event
```

This cycle is non-blocking. Multiple events can queue. The LLM works one `WorkItem` at a time; loom routes the most critical next item on each `loom next` call.

---

## Event types

| Event | Source | Primary fact changed |
|---|---|---|
| `HumanUtteranceCaptured` | human | InboxItem (pending normalization) |
| `IntentMeaningChanged` | builder | Intent description/name |
| `IntentLifecycleChanged` | builder | Intent lifecycle |
| `CodeFileChanged` | sync | CodeFile derived facts |
| `CodeExtractionChanged` | sync | symbols, imports, hash |
| `ImplementsLocatorStale` | sync | implements edge locator |
| `ValidationResultChanged` | validator / sync | Validation last_result |
| `QualityVerdictChanged` | quality | governs edge status |
| `RelationshipVerdictChanged` | analyzer | relates/requires/etc edge status |
| `HypothesisProven` | analyzer | Hypothesis status |
| `HypothesisAdopted` | builder | Intent lifecycle (planned spawned) |
| `InterfaceSurfaceChanged` | sync / builder | InterfaceSurface / exposes edge |
| `ExternalContractChanged` | builder / human | InterfaceSurface / contract_ref |
| `GraphImportedAsPlanned` | builder (import) | Intent lifecycle = planned, proofs not_run |
| `DebtClusterConfirmed` | human / LLM | Hypothesis / needs_change Intent / manual edge / Note |
| `DecisionNoteAdded` | any role | Note on target node/edge |
| `WikiPageEdited` | wiki_author / human | WikiPage + pending citation check |
| `TaskRecordCreated` | any role | TaskRecord |
| `TaskRecordClosed` | any role | TaskRecord + optional promoted graph facts |

---

## Ripple rules

When a canonical fact changes, loom evaluates which downstream facts depend on it and marks them stale.

### Code changed

```text
CodeFile content hash changed
  → re-extract symbols, imports, metrics (derived facts updated)
  → implements edges whose locator references this file → needs_reverification
  → governs edges whose intent grounds this file → needs_reverification
  → Validation.last_result → not_run; linked validates edges → needs_reverification
  → relates/requires/triggers/sequence edges between intents grounding this file → needs_reverification
  → wiki pages whose page_dependency includes this file → stale
```

### Intent meaning changed

```text
Intent description changed (redefinition)
  → implements edges grounding this intent → needs_reverification
  → governs edges for this intent → needs_reverification
  → validates edges for this intent → needs_reverification; linked Validation.last_result → not_run
  → relates/requires/etc edges touching this intent → needs_reverification
  → old wording preserved in a decision Note
  → wiki pages depending on this intent → stale

Intent name changed only
  → no ripple (cosmetic); wiki pages stale if they reference the name
```

### Validation result changed

```text
Validation passed/failed/blocked
  → validates edge status updated
  → wiki pages depending on this validation → stale
  → if failed: parent intent may surface in fix/build queue
```

### Quality verdict changed

```text
governs edge status changed
  → wiki pages depending on this rule/intent pair → stale
  → if failing: intent surfaces in fix queue
```

### Interface surface changed

```text
InterfaceSurface identity/contract changed
  → calls edges from validations → needs_reverification
  → related intents via exposes chain → stale check
  → wiki pages depending on this surface → stale
  → saga Validations: last_result → not_run; linked calls edges → needs_reverification
```

### External contract changed

```text
External contract artifact changed
  → InterfaceSurface contract_ref stale
  → related intents → needs_reverification
  → sagas/validations: Validation.last_result → not_run; linked validates and calls edges → needs_reverification
  → wiki interface pages → stale
```

### Graph imported as planned

```text
Import source graph (--as-planned)
  → all intents arrive lifecycle=planned
  → all implements groundings dropped (code differs)
  → all Validation.last_result → not_run; linked validates edges → needs_reverification
  → build queue drives realization
  → validate queue drives re-proving
```

### Wiki page edited

```text
Wiki page prose-only change
  → run citation check (do graph facts referenced still exist?)
  → if citations valid: accept
  → if citations broken: flag stale

Wiki page semantic claim differs from graph
  → capture as InboxItem(kind=docs_gap or missing_intent or ...)
  → normalize → proposed graph delta
  → graph remains canonical until delta accepted
```

### Debt cluster confirmed

```text
Statistical cluster confirmed by human/LLM
  → Hypothesis → targeted intents (if redesign claim)
  → needs_change Intent (if concrete known issue)
  → manual relates edge + Note (if indirect coupling)
  → decision Note dismissing it (if deliberate)
  → never: raw Finding node (unless deterministic structural fact)
```

---

## Entry modes

The same event/ripple engine handles all starting contexts. Entry mode determines the initial graph state and first queue priorities.

### Greenfield

Starting context: desired behavior, no code yet.

```text
Initial state:
  Intents exist as planned
  No codefiles registered
  No validations

First queues:
  build (realize planned intents in code)
  validate (add/run proofs as code is written)
  quality (seed relevant rules)
```

### Brownfield

Starting context: code exists, intent graph incomplete or missing.

```text
Initial state:
  Codefiles registered + extracted
  Intents sparse or absent
  Validations may exist as codefiles but not as Validation nodes

First queues:
  seed/align (elicit meaning from existing code)
  build (ground discovered intents)
  validate (connect test files as Validation nodes)
  quality (measure existing code against rules)
```

### Refactor

Starting context: behavior should stay the same; structure changes.

```text
Initial state:
  Existing graph with passing verdicts
  Code change proposed or in progress

First queues:
  sync (after code changes)
  fix (repair stale groundings and relationships)
  validate (re-run proofs)
  quality (verify structural rules hold post-refactor)

Key invariant:
  Intent meaning does not change unless evidence shows product changed.
  Code moving never implies behavior changed.
```

### Port

Starting context: source graph imported as design for a new codebase.

```text
Initial state:
  Intents planned (from imported graph)
  No groundings (implements edges dropped)
  No proof results (Validation.last_result = not_run, validates edges = uninspected)
  Criteria travel as acceptance contracts

First queues:
  build (realize planned intents in new language/codebase)
  validate (re-earn proofs)
  quality (re-measure rules)
```

### External ripple

Starting context: external dependency (API, contract, service) changed.

```text
Initial state:
  InterfaceSurface / contract_ref stale
  Related intents and sagas need reverification

First queues:
  fix (update interface groundings)
  validate (re-run sagas)
  align (check if product behavior evolved)
```

---

## Program state vs prompt state

The binary owns legal transitions. It cannot perform cognitive work.

**Program state** can know:

```text
edge status
truth class
queue priority
intent lifecycle
dependency set
stale cause
allowed transitions
integrity constraints
role of writer
confidence threshold
```

**Program state cannot know:**

```text
whether the code semantically satisfies the intent
whether evidence is sufficient
whether a design decision is good
whether a human requirement evolved
how to repair code
whether a wiki claim is accurate
```

Therefore, the program state machine **compiles into prompt state** for the LLM:

```text
graph facts + fact ownership context
+ role-specific mindset and constraints
+ suggested read set
+ allowed graph writes
+ required evidence shape
+ stop condition
```

This is the `PromptContract`. See `llm-driver.md` for the full contract shape.

---

## Integrity invariants

These must hold at all times and be enforced at every write boundary.

1. **Statistical never required.** No `DebtCluster` signal is a stored edge or a `loom next` required item.
2. **Derived never judged.** Sync writes derived facts. No human/LLM verdict path touches derived edges.
3. **Absence is default.** `independent` edges exist only with non-empty evidence. No row means no relationship, not uninspected.
4. **No grid materialization.** Adding N unrelated intents does not produce O(N²) relationship rows.
5. **Evidence gates.** Asserted edges with empty criterion or evidence are rejected at write time.
6. **Role gates.** An asserted write from a role not allowed for that edge kind is rejected.
7. **Ripple completeness.** Every dependency change reaches every dependent fact. No silent staleness.
8. **No cross-plane silent mutation.** Wiki edit does not change intent meaning without going through InboxItem normalization.

---

## State transitions: intent lifecycle

```text
planned
  → implemented    (builder grounds and code satisfies)
  → needs_change   (explicit known issue)
  → deprecated     (retired via loom intent retire)

implemented
  → needs_change   (quality or validation failure reveals issue)
  → deprecated     (superseded by different intent)

needs_change
  → implemented    (fix applied, re-grounded, re-proven)
  → deprecated     (intent retired instead of fixed)

deprecated
  (terminal — no forward transition)
  (invisible to computation: queues, coverage, centrality)
  (visible to history: nodes, edges, notes remain)
```

---

## Queue routing summary

| Queue | Triggered by | Owner role |
|---|---|---|
| `build` | planned / needs_change intents | builder |
| `fix` | stale implements locators, failing relationships | fixer |
| `validate` | not_run / failing validations | validator |
| `quality` | uninspected / failing governs edges | quality |
| `analyze` | stale / uninspected asserted relationships | analyzer |
| `review` | low-confidence verdicts (< 0.7) | analyzer (re-inspection mindset — form own hypothesis before reading prior evidence, then confirm or overturn) |
| `align` | stale user-visible intents (churn × centrality) | interviewer |
| `prove` | proposed / stale-supported hypotheses | analyzer |
| `wiki` | stale wiki pages after graph/code change | wiki_author |
| `inbox` | new InboxItems pending normalization | any |
| `debt` | DebtCluster feed (advisory only) | human / LLM |
