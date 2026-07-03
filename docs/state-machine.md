# loom v2 — State Machine

Status: canonical draft. This describes how loom tracks change, propagates staleness, and routes work. Terminology follows `terminology.md`; graph model follows `graph-model.md`.

---

## Foundation

loom is not a linear workflow engine. It is a **truth synchronization system**.

Change can enter from any surface:

```text
human utterance
LLM code edit
documentation correction or note
code change outside loom
validation result
external contract or API change
graph import
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
| Generated docs | deferred wiki projection (not current CLI) | routed through InboxItem today |
| Statistical suspicion | `DebtCluster` (computed, not stored) | computed on demand |
| Raw input | `InboxItem` | capture before routing |
| Operational work | `TaskRecord` | any role |

A documentation prose edit is not a change to behavior meaning unless it is routed as a graph delta through `InboxItem` and accepted.

---

## The core loop

```
event enters
  → classify source and fact type
  → route to typed delta / proposal / signal
  → apply to canonical owner plane
  → evaluate dependencies
  → propagate staleness (ripple)
  → route queues
  → LLM or human acts (see llm-driver.md)
  → write-back recorded in graph
  → loom validates transition
  → export settles or remains stale
  → next event
```

This cycle is non-blocking. Multiple events can queue. The LLM works one `WorkItem` at a time; loom routes the most critical next item on each `loom next` call.

---

## Event types

| Event | Source | Primary fact changed |
|---|---|---|
| `HumanUtteranceCaptured` | human | InboxItem (pending routing) |
| `IntentMeaningChanged` | builder | Intent description/name |
| `IntentLifecycleChanged` | builder | Intent lifecycle |
| `CodeFileChanged` | sync | CodeFile derived facts |
| `CodeExtractionChanged` | sync | symbols, imports, hash |
| `ImplementsLocatorStale` | sync | implements edge locator |
| `GroundingRoleChanged` | builder | implements edge `role` facet |
| `GroundingRehomed` | builder | implements edge `superseded_by` facet + successor grounding |
| `ValidationResultChanged` | validator / sync | Validation last_result |
| `QualityVerdictChanged` | quality | governs edge status |
| `RelationshipVerdictChanged` | analyzer | relates/requires/etc edge status |
| `HypothesisProven` | analyzer | Hypothesis status |
| `HypothesisAdopted` | builder | Intent lifecycle (planned spawned) |
| `InterfaceSurfaceChanged` | sync / builder | InterfaceSurface / exposes edge |
| `ExternalContractChanged` | builder / human | InterfaceSurface / contract_ref |
| `GraphImported` | builder (import) | restored graph facts |
| `DebtClusterConfirmed` | human / LLM | Hypothesis / needs_change Intent / manual edge / Note |
| `DocumentationDriftCaptured` | any role | InboxItem |
| `TaskRecordCreated` | any role | TaskRecord |
| `TaskRecordClosed` | any role | TaskRecord + optional promoted graph facts |

---

## Ripple rules

When a canonical fact changes, loom evaluates which downstream facts depend on it and marks them stale.

### Code changed

```text
CodeFile content hash changed
  → re-extract symbols, imports, metrics (derived facts updated)
  → realizing implements edges for this file → needs_reverification; ripple to the intent's dependents
  → consumes/configures/verifies implements edges for this file → needs_reverification only if the file vanished or seam locator drifted
  → governs edges whose intent grounds this file through a realizing edge → needs_reverification
  → Validation.last_result → not_run; linked validates edges → needs_reverification
  → relates/requires/triggers/sequence edges between intents realizing this file → needs_reverification
  → documentation items depending on this file should be captured/routed through InboxItem (wiki projection deferred)
```

### Grounding role changed or rehomed

```text
loom edge set-role <edge> <role> --reason ...
  → if role changed and the edge was settled: edge → needs_reverification with stale_cause starting `role_changed`
  → coverage ownership recomputed immediately; only `realizes` owns

loom edge rehome <edge> --to "<successor intent>" --reason ...
  → old edge gains superseded_by and leaves coverage/staleness/queues
  → successor grounding carries old locator + role
  → if the old edge was settled: successor edge → needs_reverification with stale_cause starting `rehomed`
```

### Intent meaning changed

```text
Intent description changed (redefinition)
  → implements edges grounding this intent → needs_reverification
  → governs edges for this intent → needs_reverification
  → validates edges for this intent → needs_reverification; linked Validation.last_result → not_run
  → relates/requires/etc edges touching this intent → needs_reverification
  → completeness waiver facets are cleared so waived axes re-open under the new meaning
  → old wording and waiver reopening are preserved in decision Notes
  → documentation items depending on this intent should be captured/routed through InboxItem (wiki projection deferred)

Intent name changed only
  → no ripple (cosmetic); documentation references may need InboxItem routing
```

### Validation result changed

```text
Validation passed/failed/blocked
  → validates edge status updated
  → documentation items depending on this validation should be captured/routed through InboxItem
  → if failed: the linked intent surfaces in fix queue
```

### Quality verdict changed

```text
governs edge status changed
  → documentation items depending on this rule/intent pair should be captured/routed through InboxItem
  → if failing: linked intent surfaces in fix queue
```

### Interface surface changed

```text
InterfaceSurface identity/contract changed
  → calls edges from validations → needs_reverification
  → related intents via exposes chain → stale check
  → journey validations: last_result → not_run; linked calls edges → needs_reverification
```

### External contract changed

```text
External contract artifact changed
  → InterfaceSurface contract_ref stale
  → related intents → needs_reverification
  → journey validations: Validation.last_result → not_run; linked validates and calls edges → needs_reverification
  → documentation drift captured as InboxItem if present
```

### Graph imported

```text
Import source graph
  → restore exported graph facts into a fresh store
  → validate before writing; never leave a partial graph
  → run loom status / loom next to continue from the imported state
```

Porting import that drops groundings and proof results as planned work is deferred; the current binary does not expose `import --as-planned`.

### Documentation edited `[deferred wiki projection]`

```text
Documentation prose-only change
  → if it is merely explanatory, no graph change
  → if it contradicts or extends graph truth, capture as InboxItem

Documentation semantic claim differs from graph
  → loom inbox add "<drift>" --source wiki --link <ref>
  → route through door/inbox landing commands
  → graph remains canonical until a typed graph write is accepted
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

First work:
  human product check (elicit meaning from existing code; no current `align` queue)
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

### Port `[deferred/non-current]`

Starting context: a source graph is used as design input for a new codebase. The current binary imports a graph as-is; planned-port import that drops groundings/proofs is deferred.

```text
Initial state:
  Intents planned (from imported graph)
  No groundings (implements edges dropped)
  No proof results (Validation.last_result = not_run, validates edges = uninspected)
  Criteria travel as acceptance contracts

First work once such a graph is prepared:
  build (realize planned intents in new language/codebase)
  validate (re-earn proofs)
  quality (re-measure rules)
```

### External ripple

Starting context: external dependency (API, contract, service) changed.

```text
Initial state:
  InterfaceSurface / contract_ref stale
  Related intents and journeys need reverification

First queues/work:
  fix (update interface groundings)
  validate (re-run journey proofs)
  human product check (decide whether product behavior evolved; no current `align` queue)
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
whether a documentation claim is accurate
```

Therefore, the program state machine **compiles into prompt state** for the LLM:

```text
graph facts + fact ownership context
+ role-specific mindset and constraints
+ suggested read set
+ allowed graph writes
+ required evidence shape
+ truth axis correct_when line
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
8. **No cross-plane silent mutation.** Documentation edits do not change intent meaning without going through InboxItem routing.
9. **Pack definition is the baseline.** A seeded/builtin rule body compared against the shipped pack definition is a structural smell (`pack_drift`) if it differs. Remedy: re-baseline with `loom rule seed <pack>` (idempotent) or keep the customization as its recorded trace.

### Structural smells

The `loom smells` surface reports derived graph-shape signals, each with a remedy. One such smell is `pack_drift`: a quality rule that originated from a seeded pack has been edited in the graph, so its current body no longer matches the shipped pack definition. Another is `consumer_owned_file`: a file whose sole realizing owner is an intent whose other realizing files live in a different top-level directory cluster. Remedies name the concrete edge or pack command; `pack_drift` is resolved either by `loom rule seed <pack>` to re-baseline (idempotent) or by accepting the customization and letting it remain surfaced as a recorded divergence.

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
| `elaborate` | user-visible feature intents with open Definition-of-Complete axes; highest incomplete score first | builder |
| `coverage` | registered codefiles with no live realizing owner | builder |
| `fix` | failing asserted edges of any kind; stale asserted edges except `governs`/`validates` | fixer for failing, analyzer for stale relationship/grounding re-verification |
| `validate` | uninspected / stale `validates` edges only | validator |
| `quality` | uninspected / stale `governs` edges only; or first never-measured rule × root implemented intent pair | quality |
| `analyze` | uninspected non-`governs`/non-`validates` asserted relationships | analyzer |
| `triage` | untriaged/stale derived findings, including external diagnostic findings, and new InboxItems needing routing | analyzer |
| `review` | asserted passing/independent verdicts with `0 < confidence < 0.7`, lowest first | edge kind's registry owner, with independent re-inspection mindset |
| `prove` | proposed hypotheses | analyzer |
