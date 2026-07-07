# loom v2 terminology

Status: canonical draft. This document promotes stable language from `docs/scratchpad.md` into terminology the code, CLI, prompt contracts, and future docs should use. Prefer these terms in new design docs. Conversation aliases are listed so they can be translated rather than copied forward.

## Purpose

loom is a graph-backed companion for LLM-driven codebase work. Its terminology must distinguish:

- what is canonical truth vs a projection,
- what the program can compute vs what an LLM must judge,
- durable graph facts vs temporary work records,
- product behavior vs code structure,
- workflow state vs prompt contract.

A term is stable when an LLM can use it consistently in a prompt, a command, and a graph write-back.

---

## Core model

### loom

The CLI/runtime that maintains the graph, routes work, emits prompt contracts, verifies graph writes, tracks staleness, and exports/publishes projections.

Avoid: treating loom as the autonomous actor. The LLM acts; loom routes and checks.

### graph

The durable model of a repository: nodes, edges, facets, evidence, provenance, and state. The runtime graph lives in `.loom/graph.sqlite`; the portable committed export is `loom.graph.json`.

### graph fact

A typed claim stored or derived by loom. Examples: an intent exists, a codefile realizes an intent through an `implements(role=realizes)` edge, a validation passed, a rule verdict failed, an interface surface exists.

Use instead of vague “truth item.”

### canonical owner

The place where a fact type is authoritative.

Examples:

| Fact type | Canonical owner |
|---|---|
| behavior meaning | `Intent` |
| code reality | filesystem + derived `CodeFile` facts |
| proof result | `Validation` result / `VALIDATES` edge |
| quality judgment | `GOVERNS` edge |
| interface contract | `InterfaceSurface` or source contract artifact |
| rationale | `Note` |
| generated docs | `WikiProjection` / manifest, never semantic truth |

### evidence

Observed support for a graph write. Evidence must point to inspected code, command output, validation result, source document, external contract, or human decision.

### provenance

Who or what produced a fact or verdict, when, by which role/source, and from what evidence.

### dependency

A graph/code/proof/wiki fact that another fact depends on for freshness. If a dependency changes, the dependent fact may become stale.

### ripple

The staleness propagation caused by a changed dependency.

Example: codefile changed -> derived code facts recomputed -> affected verdicts/proofs/wiki pages marked stale.

### projection

A generated or rendered expression of graph truth. Wiki pages are projections. They explain the graph; they do not replace it.

---

## Truth classes

### derived fact

A reproducible fact computed by loom from code, files, configuration, or graph structure.

Examples:

- codefile hash,
- imports,
- symbols,
- language,
- generated/vendor classification,
- deterministic interface extraction,
- structural finding if `Finding` becomes a derived node.

Derived facts are recomputed by `loom sync`. Humans do not inspect derived facts into truth.

### asserted fact

A judgment-bearing fact written by a human or LLM with evidence.

Examples:

- an `IMPLEMENTS` grounding,
- a `GOVERNS` verdict,
- a manual relationship,
- a decision note,
- a validation result,
- a confirmed interface contract.

Asserted facts persist until invalidated by dependency changes.

### statistical signal

A heuristic suspicion computed from history or structure. It is not stored as an edge and never gates required work by existing.

Examples:

- co-change cluster,
- clone cluster,
- shotgun surgery signal,
- recurrent trouble,
- proof-locality suspicion.

Use `DebtSignal` or `DebtCluster` for these. A confirmed signal must be promoted to a durable graph fact such as a `Hypothesis`, `Intent`, manual edge, or decision note.

Avoid: storing `statistical` as `edge.truth_class`.

---

## Node types

### Intent

A falsifiable statement of what the codebase should do. It can be user-visible or internal. It may be broad or atomic, but it should have a criterion that can eventually be grounded and proven.

Examples:

- `user can log in with password`,
- `remember-me token restores session after browser restart`,
- `sync invalidates stale verdicts after code changes`.

### CodeFile

A file in the repository known to loom. It carries derived facts such as language, hash, imports, symbols, generated/test/source role, and locators.

### QualityRule

A reusable behavioral norm held against an `Intent`.

Examples:

- external input is validated,
- endpoint requires authorization,
- view has loading/empty/error states,
- performance budget is proven.

The rule is the norm. The `GOVERNS` edge is the verdict.

### CodeRule

A reusable structural norm held against code.

Examples:

- file should not exceed N lines,
- function complexity should stay below threshold,
- production boundary should not panic,
- generated files do not count toward ownership debt.

### Finding

A located deterministic structural occurrence, usually tied to a `CodeRule` and `CodeFile`.

Examples:

- oversized file at `src/commands/guide.rs`,
- complex symbol at `src/db/stats.rs:build_ladder`,
- panic marker at `src/server/auth.rs:require_user`.

Do not use `Finding` for statistical co-change or clone clusters unless a deterministic located occurrence is confirmed.

### external diagnostic adapter

A portable scan configuration entry registered by `loom scan add`. It names a linter, type-checker, static analyzer, or bespoke command from any language/toolchain plus an optional parse regex. `loom scan run` converts parsed diagnostics for registered `CodeFile`s into derived `Finding` nodes.

### external diagnostic finding

A derived `Finding` whose body records `kind=external_diagnostic`, adapter name, file, line, message, and optional diagnostic code. It participates in the ordinary finding triage lifecycle. If a later adapter run no longer emits the diagnostic, loom resolves the derived finding.

### pattern pre-screening

A quality-rule assist, not a verdict. Pack rules may carry `patterns[]` as regex strings. When a quality WorkItem is built, loom runs those regexes over the target intent's grounded files and embeds `pre_screened_hits` (`path`, `line`, `pattern`, `excerpt`) in the packet. The hits are never stored; the LLM must confirm or refute each hit before writing a `GOVERNS` verdict.

`detection_kind=llm_judgment` means no machine pre-screening is used. `detection_kind=pattern` means `patterns[]` can produce `pre_screened_hits`, but the final judgment is still asserted evidence from the LLM/human.

### Validation

A proof object: command, manual check, journey, benchmark, assertion, scenario, or contract that validates one or more intents.

A test file is a `CodeFile`; the proof is the `Validation`.

### Hypothesis

A proposed change or redesign claim that must be proven before it becomes work.

A hypothesis has a claim, proposal, predicted outcome, target intents, and proof/refutation evidence.

### InterfaceSurface

A seam through which behavior is consumed or composed.

Examples:

- HTTP endpoint,
- CLI command,
- UI route/screen,
- message topic,
- SDK method,
- internal module interface,
- storage/repository seam.

Do not register every private function. Register surfaces that matter for composition, validation, contract, or consumers.

### Note

Durable prose attached to a graph target. Use for decisions, rationale, confirmations, transitions, questions, and handoffs.

### InboxItem

The single free-form input boundary. Raw human/LLM/wiki/code-audit text enters here before becoming graph truth.

Inbox items are candidates, not truth.

### TaskRecord

An operational record for temporary work such as a spike, investigation, experiment, review, or chore.

Task records guide work but do not certify truth. Durable outcomes must be promoted to graph facts.

Preferred term: `TaskRecord`.

Allowed alias: spike, investigation.

---

## Intent structure

### intent hierarchy

A narrow part-of decomposition relation between intents. It answers: “what is this behavior made of?”

Use hierarchy for coverage and roll-up. Do not use it for every semantic relationship.

### parent intent

An intent decomposed into child intents.

### child intent

An intent that contributes to a parent intent through hierarchy.

### atomic intent

An intent small enough to have:

1. a falsifiable criterion,
2. independent failure modes,
3. plausible proof or code grounding.

**Atomization guard:** functions and symbols are locators, not intents. An intent named after a function is rejected by `loom intent add` unless `--allow-symbol-name` and a behavioral description are both provided. Use the `implements` edge locator to point at specific symbols.

```text
not an intent:  sync_build_report
not an intent:  capture_payment
valid intent:   payment can be captured
valid intent:   sync output is bounded and readable
```

### capability intent

An intent at capability altitude: the system can do something meaningful. It is still an `Intent`; do not introduce a separate `Capability` node unless proven necessary.

### scenario intent

A concrete case of a behavior, usually linked by `scenario_of`. Scenario families are ordinary intents connected to a parent capability by `ScenarioOf`; the scenario intent's `aspect` facet marks whether it is `happy`, `sad`, `fallback`, or `edge_case`.

Example: `invalid password is rejected without session creation` scenario_of `user can log in with password` with `aspect=sad`.

### variant intent

A named variation of a behavior, usually linked by `variant_of`.

### reaction intent

A behavior that must hold when a condition/event occurs, usually linked with `triggers`.

### invariant intent

A behavior that must always hold across operations or states.

### intent neighborhood

The query result around an intent: hierarchy, required intents, variants/scenarios, triggers, codefiles, validations, rules, notes, interfaces, wiki pages, and relevant signals.

Preferred term: `intent neighborhood`.

Avoid: `subgraph` when it sounds like a separate graph. If used, define it as a view/query.

---

## Edge kinds

### hierarchy

`Intent -> Intent`. Part-of decomposition only.

### requires

`Intent -> Intent`. This behavior depends on another behavior/capability.

Use for reusable atomic concepts. Do not give reusable atoms multiple hierarchy parents just to show reuse.

### scenario_of

`Intent -> Intent`. Child scenario to parent capability.

Direction: scenario -> parent.

### variant_of

`Intent -> Intent`. Variant to base behavior.

Direction: variant -> base.

### triggers

`Intent -> Intent`. When condition/event on the edge occurs, the response intent must hold.

The condition belongs as an edge facet/property.

### sequence

`Intent -> Intent`. Ordered step relation inside a journey. Asserted by judgment via `loom edge relate sequence`; `loom journey add` does not create it, because a spec's step order is a test script, not a domain ordering claim.

### implements

`Intent -> CodeFile`. Grounding edge from behavior to a file/locator. Its `role` edge facet says whether behavior lives there or the file is only a supporting surface; missing `role` means `realizes`.

### grounding role

The role on an `implements` edge. Canonical values are `realizes`, `consumes`, `configures`, and `verifies`.

### realizes

`implements.role=realizes`. The behavior lives in this file/locator. This is the default role and the only grounding role that owns coverage.

### consumes

`implements.role=consumes`. The file exercises behavior across a seam — route, topic, key, import, or similar boundary. It never owns coverage for that behavior.

### configures

`implements.role=configures`. The file supplies configuration for behavior that lives elsewhere. It never owns coverage.

### verifies

`implements.role=verifies`. The file checks or proves behavior that lives elsewhere. It never owns coverage.

### validates

`Validation -> Intent`. Proof checks behavior.

### governs

`QualityRule -> Intent`. Norm is measured against behavior. Verdict lives on the edge.

### flags

`Finding -> CodeFile`. Located finding concerns a codefile/symbol.

### assesses

`Finding -> CodeRule`. Finding is an occurrence of a code rule.

### exposes

`InterfaceSurface -> CodeFile`. Code exposes a surface. `exposes` is asserted-only; derived surface extraction is not implemented.

### calls / exercises

`Validation -> InterfaceSurface`. A proof exercises a surface.

### targets

`Hypothesis -> Intent`. A hypothesis concerns an intent.

### relates

Manual or asserted relationship between intents that does not fit a more specific edge kind yet. Prefer specific kinds when available.

---

## Validation terminology

### proof

Observed evidence that an intent holds. In the graph, durable proof is represented by `Validation` and `VALIDATES` edges.

### validation type

The proof mechanism.

Examples:

- test,
- assertion,
- benchmark,
- manual_check,
- journey,
- scenario,
- contract.

### proof granularity

The claim altitude the validation can falsify.

Examples:

| Intent shape | Validation shape |
|---|---|
| atomic leaf | unit/module test, assertion, property test |
| internal capability | integration test through seam |
| external interface | contract/API/CLI/UI test |
| scenario/journey | journey or flow test |
| reaction | event/scenario test |
| performance | benchmark |
| visual/product acceptance | manual or visual proof |

### journey

A proof that multiple behaviors work together in an ordered flow. The term is `journey`. `saga` is the retired v1 name — as of 0.15 the binary accepts no `saga` alias, validation type, or spec key; use it only when narrating history.

Child proofs alone do not necessarily prove parent composition.

### blocked proof

A validation that cannot currently run because of an explicit external prerequisite. It must carry a reason.

---

## Wiki terminology

### WikiProjection

Reader-first documentation tracked as a projection of graph facts — the graph governs each page's truth and freshness (`loom wiki`, shipped v0.20.0); an agent writes the prose.

Preferred term: wiki projection.

Avoid: wiki truth.

### WikiPage

One tracked page in a wiki projection: a node carrying the page's path and freshness, linked to the intents it documents by `documents` edges. Created by `loom wiki plan`, marked fresh by `loom wiki record`, staled by `sync` when a documented intent, its code, or its proof drifts.

### WikiManifest `[historical design/not current]`

The run manifest: page plan, nav tree, graph export hash, git commit, page dependencies, and citations. Part of the fuller `wiki-projection.md` design; the shipped wiki tracks freshness per page via its scope fingerprint instead.

### citation

A reference from authored prose to graph evidence, code locators, validation results, source contracts, or decision notes.

### page dependency

A graph/code/proof fact that a wiki page relies on. If the dependency changes, the page becomes stale.

### preview run `[historical design/not current]`

An isolated wiki generation under `.loom/wiki-runs/<run-id>/` before publishing — part of the fuller `wiki-projection.md` design; the shipped wiki has no run pipeline (an agent authors the page in place, then `loom wiki record` stamps it fresh).

### publish `[historical design/not current]`

Promotion of a verified wiki preview into `docs/loom/**` — same status as preview run above.

---

## State machine and LLM driver terminology

### event

A change entering loom from human, LLM, code, documentation, validation, import, or external contract.

### source

Where an event came from: human, llm, code, wiki/documentation, validation, external, import, signal.

### route

Translate captured input or a graph state into a typed next step, task, proposal, signal, or rejection.

### queue route

Choose the next queue/work item based on graph state.

### WorkItem

A promptable unit of work emitted by `loom next`. Real fields are `mode`, `owner_role`, `effort`, `reason`, `target`, `stale_causes`, `prompt_contract`, `context`, `truth_gap`, and `next_step`. File hints live at `context.read_set`; allowed actions and write-back live inside `prompt_contract`.

### PromptContract

The LLM-facing contract for a WorkItem: mindset, allowed actions, forbidden actions, required evidence, exact write-back, stop condition, and escalation/human gate.

### role

The hat the LLM must adopt for a work item.

Current WorkItem owner roles:

- builder,
- analyzer,
- fixer,
- validator,
- quality.

Human-protocol/implicit roles:

- interviewer (implemented as door/inbox routing protocol, not a current WorkItem owner role),
- wiki author (the agent driving `loom wiki next`/`record`; not a gated WorkItem owner role — wiki work is served by `loom wiki next`, not `loom next`).

### mindset

Role-specific mental posture included in the prompt contract.

Example: validator mindset = run or mark proof honestly; do not edit code to make it pass.

### write-back

The exact graph command or structured report the LLM must produce after acting.

### stop condition

The point where the LLM must stop acting and return to loom status/next instead of wandering.

### human gate

A state requiring human decision or input. The LLM may frame choices, but must not silently decide product authority.

### effort

A model-neutral difficulty tier for the work: low, mid, high. The harness maps effort to available model/tooling; loom does not name vendors.


### Definition-of-Complete

The per-intent scorecard reported by `loom completeness` and embedded in `loom next --mode elaborate`. Its axes are `scenarios`, `prerequisites`, `boundary`, `proof`, `journey`, and `questions`.

### completeness waiver

A reasoned facet written by `loom intent waive <intent> <axis> --reason "<why>"` for non-question completeness axes. A waiver says an axis deliberately does not apply right now; it is cleared when the intent is redefined, because the reason was granted against the previous meaning.

### open question

An unanswered product/design question captured as an `InboxItem` with `--source question`, usually linked to an intent by `--link intent:<id>`. Open questions are counted in `graph_state.open_questions` and surfaced by `loom session`; the linked intent's `questions` axis stays open until the inbox item is answered, routed, rejected, duplicated, deferred, or otherwise withdrawn from the open question set.

### elaborate queue

The builder queue behind `loom next --mode elaborate`. It serves the most-incomplete user-visible feature intent and asks the LLM to grow its forgotten surroundings: scenarios, prerequisites, boundary/proof/journey coverage, crisp human questions, or reasoned non-question waivers.

### state transition

A legal status change accepted by loom after LLM write-back and validation.

---

## Code intelligence terminology

### code map

The deterministic extracted view of codefiles, symbols, imports, surfaces, hashes, and metrics.

### locator

A stable-ish reference to code: file path plus symbol or line range.

### read set

The files/symbols/evidence loom suggests the LLM should inspect for a WorkItem.

### dig `[deferred/not current]`

A deferred focused helper over a WorkItem's `context.read_set`, `stale_causes`, target, and prior evidence. The current CLI surfaces this context directly in `loom next`.

### extraction

Deterministic code analysis done by loom. Extraction creates derived facts.

### investigation

Cognitive code reading/debugging done by the LLM with tools. Investigation may produce asserted facts, notes, hypotheses, or task results.

---

## Lifecycle and status terminology

### current

A derived fact or projection is up to date relative to its dependencies.

### uninspected

An asserted fact requires first judgment.

### passing

An asserted verdict/proof currently holds with evidence.

### failing

An asserted verdict/proof currently does not hold with evidence.

### independent

Evidence-bearing judgment that a rule/relationship does not apply. It is rare and deliberate; absence is the default.

### needs_reverification

A previously asserted fact became stale because a dependency changed.

### blocked

Work or proof cannot proceed due to an explicit prerequisite. Must include reason.

### planned

Behavior exists in the graph as intended work but is not yet implemented.

### implemented

Behavior is believed realized in code and should be grounded/proven.

### needs_change

Known behavior/code issue. Honest work state, not a fake failing verdict.

### deprecated

Superseded intent retained for history, excluded from active computation.

---

## Avoid / aliases table

| Conversation term | Prefer | Reason |
|---|---|---|
| subgraph | intent neighborhood | Avoid implying separate graph storage. |
| wiki truth | wiki projection / wiki claim | Wiki explains graph; it is not canonical. |
| spike | TaskRecord with kind=spike | Spike is operational work, not product truth. |
| debt | statistical signal / confirmed work | Raw signal is not required work. |
| component | parent intent / capability intent / code area | Component is overloaded. Use precise term. |
| test file proves X | Validation validates X | Test file is code; validation is proof. |
| source of truth | graph / canonical owner | Different fact types have owners; docs are projection. |
| code intelligence decides | extraction suggests, LLM judges | Static facts do not replace semantic judgment. |
| workflow phase | route / WorkItem / PromptContract | Loom is event/ripple-driven, not linear. |
| task proves behavior | Validation proves behavior | TaskRecord can produce evidence but is not proof. |

---

## Naming rules

1. If it describes behavior, use `Intent`.
2. If it describes source reality, use `CodeFile` or derived code fact.
3. If it proves behavior, use `Validation`.
4. If it measures a behavioral norm, use `QualityRule` + `GOVERNS` verdict.
5. If it measures a structural norm, use `CodeRule` + `Finding`.
6. If it is temporary work, use `TaskRecord`.
7. If it is generated documentation, use `WikiProjection` / `WikiPage`.
8. If it is heuristic suspicion, use `DebtSignal` / `DebtCluster` until confirmed.
9. If it guides the LLM, use `WorkItem` + `PromptContract`.
10. If it is raw free-form input, use `InboxItem`.
