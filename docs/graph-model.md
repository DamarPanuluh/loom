# loom v2 — Graph Model

Status: canonical draft. This is the authoritative description of the graph schema: nodes, edges, truth classes, facets, and type system. Terminology follows `terminology.md`.

---

## Purpose

The graph is the durable memory of a repository. It models:

- which authored flows root delivery (`Journey`)
- what the codebase is supposed to do (`Intent`)
- where it lives (`CodeFile`)
- how it is proven (`Validation`)
- what norms it must satisfy (`QualityRule`, `CodeRule`)
- what issues exist (`Finding`, `Hypothesis`)
- where the system is consumed (`InterfaceSurface`)
- why decisions were made (`Note`)
- what work is in progress (`InboxItem`, `TaskRecord`)

The model is designed so every fact has a canonical owner, a truth class, and a state. The graph never lies by omission: absence of a row means no relationship, not an uninspected one.

---

## Cornerstone nodes

Journey is the authored root of delivery. Intent and CodeFile remain the two implementation cornerstones beneath it.

```text
Journey     who does what, in what order, and what must then be true
Intent      what the code should do
CodeFile    where code lives
```

Everything else is a supporting family member:

```text
Intent family
  Journey         authored semantic root that derives technical Intent
  QualityRule     a reusable behavioral norm held against Intent
  Validation      a proof that Intent holds
  Hypothesis      a proposed change to Intent, proven before adoption

CodeFile family
  CodeRule        a reusable structural norm held against code
  Finding         evidence-backed observation (derived producer or asserted manual finding)
  Question        product question awaiting human answer
  InterfaceSurface  a seam through which behavior is consumed

Cross-cutting
  Note            audit trail prose attached to any node or edge
  InboxItem       raw free-form input before routing
  TaskRecord      temporary operational work record (spike, investigation, etc.)
```

---

## Node types

### Intent

A falsifiable behavioral statement. The smallest unit that can have a criterion, a grounding, and a proof.

**Atomization guard — symbols are locators, not intents.**

Functions, methods, and symbol names are not intents. They are captured as locators on the `implements` edge. An intent named after a function (`sync_build_report`, `capture_payment`, `run_with_sqlite`) is a symptom of function-level granularity.

**Enforcement: symbol-looking intent names are rejected by default.**

`loom intent add` rejects the command when the name matches a symbol pattern (snake_case with no spaces, no verb-phrase structure) unless the author passes `--allow-symbol-name` AND provides a non-empty `--description` that contains a behavioral criterion. Both conditions required; either alone is rejected. The override is recorded in the node so `loom doctor` can audit all overrides for drift.

```text
rejected:   loom intent add --name "capture_payment"
rejected:   loom intent add --name "capture_payment" --allow-symbol-name
rejected:   loom intent add --name "capture_payment" --description "the capture_payment function" --allow-symbol-name
accepted:   loom intent add --name "capture_payment"
              --description "payment is captured and inventory reserved before fulfillment"
              --allow-symbol-name
accepted:   loom intent add --name "payment can be captured" --description "..."
              (no flag needed; not a symbol pattern)
```

`loom doctor` surfaces all intents where `--allow-symbol-name` was used so the graph can be audited for function-level granularity drift over time.

A valid atomic intent must satisfy all three:

1. A falsifiable behavioral criterion (can be proven or disproven)
2. Independent failure modes (its failure is meaningful in isolation)
3. A plausible proof or code grounding at a meaningful altitude

The `implements` edge carries facets to explain the grounding:

```text
Intent: payment can be captured
  implements → src/payment.rs  --locator "fn capture_payment" --role realizes
Intent: checkout calls payment
  implements → src/checkout.rs --locator "POST /payments" --role consumes
```

`role` defaults to `realizes` when the facet is absent, preserving old graph semantics.

Fields:

```text
id
name
description
lifecycle:     planned | implemented | needs_change | deprecated
visibility:    user_visible | internal | untriaged
aspect:        capability | happy | sad | fallback | edge_case | invariant
layer:         (architecture layer — project-defined)
domain:        (product/business facet — discovery/scoring only)
tags:          [] from tag_vocabulary
source_refs:   [] links to source docs, contracts, ADRs
confirmed_at:  last user alignment timestamp
created_at
updated_at
```

### CodeFile

A file registered in the graph. Carries machine-derived and registry-backed facts.

Fields:

```text
id
path
language:       (derived)
loc:            (derived)
content_hash:   (derived)
symbols:        [] (derived)
imports:        [] (derived)
role:           source | test | generated | vendor | config | migration | other
extractor_grade: confident | heuristic | none
created_at
updated_at
```

### QualityRule

A reusable behavioral norm. Pack rules ship with all guidance fields pre-authored. Custom rules start with `detection_kind=llm_judgment`; guidance fields are optional but strongly recommended — without them LLM verdicts drift across sessions.

Fields:

```text
id
name
description
category:           security | performance | defect | style | robustness | ...
severity:           error | warning
effort:             low | mid | high
detection_kind:     llm_judgment | pattern
                      llm_judgment  LLM must inspect; no machine pre-screening.
                      pattern       patterns[] contains regex strings that are
                                    run at quality WorkItem build time against
                                    the target intent's grounded CodeFiles.
                                    Hits are attached to the packet as
                                    pre_screened_hits; the LLM still confirms
                                    or refutes every hit before recording the
                                    verdict. Hits are never stored.
patterns:           [] regex strings for machine pre-screening, e.g.
                      ["\\bunwrap\\(\\)", "\\blet\\s+_\\s*="]
                    A hit means a candidate violation; LLM confirms verdict.
                    Empty patterns[] means no machine pre-screening and is
                    normal for detection_kind=llm_judgment.
inspection_guide:   prose — step-by-step what to read and check
detection_hints:    [] strings — LLM-facing guidance: grep targets, function names,
                                 anti-patterns, mental model for the inspection
evidence_template:
  passing:          suggested phrasing for passing verdict evidence
  failing:          suggested phrasing for failing verdict evidence
passing_example:    { criterion, evidence, confidence }   few-shot positive case
failing_example:    { criterion, evidence, confidence }   few-shot negative case
pack:               (origin pack name if seeded; empty for custom rules)
applies_when:       (optional signal JSON for pack recommendation scoring)
created_at
```

### CodeRule

A reusable structural norm for code. Most CodeRules are `detection_kind=pattern`; sync detects violations automatically and creates `Finding` nodes. LLM judgment is used to confirm whether a finding is intentional.

Fields:

```text
id
name
description
category:           size | complexity | style | safety | ...
severity:           error | warning
effort:             low | mid | high
detection_kind:     pattern | llm_judgment
inspection_guide:   (optional prose for cases requiring LLM confirmation)
evidence_template:
  passing:          suggested phrasing
  failing:          suggested phrasing
threshold:          (optional numeric threshold for pattern detection)
created_at
```

### Validation

A proof object.

Fields:

```text
id
name
description
type:           test | assertion | benchmark | manual_check | journey | scenario | contract
command:        (runnable command or empty for manual)
last_result:    not_run | passed | failed | blocked
last_run_at
last_evidence:  (text evidence from last run)
blocked_reason: (if blocked)
created_at
updated_at
```

The `journey` validation type is compiler-owned. `loom journey compile` creates it for one accepted Journey/profile and wires its exact `proves`, `validates`, `calls`, and `exercises` closure. Operators do not create Journey validations directly.

### Journey

The authored root artifact. A Journey records user or operator meaning before technical Intent, code, interface, or proof choices.

Fields:

```text
id
schema:             loom.journey/v1
stable_id:          stable artifact id
title
summary
artifact:           confined repository-relative JSON or YAML path
semantic_hash:      canonical hash of authored meaning
input_ids:          typed input ids
precondition_ids:   declarative prerequisite ids
step_ids:           ordered semantic actor/action ids
expectation_ids:    outcome ids bound to steps
output_ids:         typed output ids
profile_ids:        named execution scenarios
created_at
updated_at
```

Only `steps` preserve authored array order. Other addressable collections are canonicalized by stable ID. A semantic hash change invalidates every accepted `derives` and `surfaces` projection because prior technical meaning was authorized against different authored behavior.

### Hypothesis

A proposed change to behavior, proven before becoming work.

Fields:

```text
id
name
claim:          what is wrong now
proposal:       the change
predicted_outcome: measurable result
status:         proposed | supported | refuted | adopted | rejected
proof_evidence: (after prove step)
adopted_into:   [] intent refs (after adoption)
created_at
updated_at
```

### Finding

The one node type for evidence-backed observations. Programmatic producers use `truth_class: derived`; LLM/tool observations use `truth_class: asserted` through `loom finding add`. Both share listing, triage, adjudication, and staleness display. Explicit `loom debt promote` also mints an asserted Finding with `source: debt_promotion` (cluster snapshot + operator evidence/confidence in the body) and creates no edges or facets (including no `flags`/`assesses`).

Fields:

```text
id
kind:           oversized_file | complex_symbol | tangled_file | code_audit | ...
location:       file path + optional symbol (derived producers)
truth_class:    derived | asserted
source:         code_audit | wiki | validation | llm | debt_promotion (asserted body)
evidence:       observed fact (asserted body)
impact:         why it matters (asserted body)
confidence:     0.0..1.0 (asserted body)
file/link:      registered codefile or graph ref (asserted body)
created_at
```

Note: deterministic structural facts recomputed by sync and external scan diagnostics remain derived findings. Statistical signals are `DebtCluster` values — computed, never stored as nodes. `loom debt` currently computes LOC `size_outlier` and git-history `co_change` clusters (clone/shotgun/recurrence remain design vocabulary, not computed).

### InterfaceSurface

A public seam through which behavior is consumed or composed.

Fields:

```text
id
name
kind:           http | cli | ui_route | message_topic | sdk_method | internal_module | storage | ...
identity:       method+path, command name, topic name, symbol, etc.
description
contract_ref:   path to schema/IDL/OpenAPI artifact if exists
created_at
updated_at
```

For Journey-root delivery, an accepted surface uses `loom.interface-surface/v1`, kind `cli`, structured argv, typed arguments, JSON output, and stable operation IDs. A `surfaces` edge binds the Journey to that surface and stores complete step-to-operation bindings plus the Journey hash. An `exposes` edge points from the surface to the real registered source file. Optional per-operation `exercises` arrays name downstream code entries reached through that public operation; they participate in surface hashing and compile into compiler-owned `Exercises` provenance without changing surface ownership.

### Note

Prose record attached to any node or edge.

Fields:

```text
id
target_id
target_type:    node | edge
kind:           decision | justification | transition | confirm | question | idea | todo | commentary
text
author:         human | llm | (role)
audience_role:  builder | analyzer | fixer | validator | quality | (any)
created_at
```

### InboxItem

Raw human/external free-form input before routing into typed graph facts or disposition.

Fields:

```text
id
raw_text
source:         human | external | support | import
status:         new | routed | rejected | deferred | duplicate
target_refs:    [] optional links to related nodes/refs
created_at
updated_at
```

### Question

Product question awaiting a human answer for an intent. Questions are not inbox items.

Fields:

```text
id
text
status:         open | answered | withdrawn | duplicate | deferred
intent:         intent id (body cache; `questions` edge is canonical link)
created_at
updated_at
```

### TaskRecord

Temporary operational work record. Does not certify truth; promotes durable outcomes.

Fields:

```text
id
title
kind:           spike | investigation | experiment | review | chore
source:         human | llm | queue | wiki | validation
status:         proposed | active | completed | abandoned | blocked
target_refs:    [] links to related nodes/edges/wiki pages
prompt_contract (snapshot of PromptContract at start)
result_summary
evidence_refs:  [] locators from investigation
promoted_to:    [] graph refs created as result
created_by
closed_reason
created_at
updated_at
```

---

## Edge model

All edges share one typed table. The `edge_kind_registry` constrains endpoint types, truth class, and allowed statuses per kind.

### Unified edge schema

```sql
CREATE TABLE edge (
  id           TEXT PRIMARY KEY,
  from_id      TEXT NOT NULL REFERENCES node(id),
  to_id        TEXT NOT NULL REFERENCES node(id),
  kind         TEXT NOT NULL,
  truth_class  TEXT NOT NULL CHECK(truth_class IN ('derived','asserted')),
  status       TEXT NOT NULL DEFAULT 'uninspected',
  criterion    TEXT NOT NULL DEFAULT '',
  evidence     TEXT NOT NULL DEFAULT '',
  confidence   REAL NOT NULL DEFAULT 0.0,
  depends_on   TEXT NOT NULL DEFAULT '[]',  -- JSON typed dependency refs
  inspected_by TEXT NOT NULL DEFAULT '',
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);

CREATE INDEX idx_edge_queue    ON edge(truth_class, status);
CREATE INDEX idx_edge_kind     ON edge(kind, status);
CREATE INDEX idx_edge_from     ON edge(from_id, kind);
CREATE INDEX idx_edge_to       ON edge(to_id, kind);
```

The asserted residue queue is one indexed read:

```sql
SELECT * FROM edge
WHERE truth_class = 'asserted'
  AND status IN ('uninspected', 'needs_reverification');
```

### Edge kind registry

```text
edge_kind_registry
  kind
  from_node_type
  to_node_type
  allowed_truth_classes:  [derived | asserted]
  allowed_statuses:       []
  owner_role:             builder | analyzer | fixer | validator | quality | sync
  description
```

The write-time check verifies the supplied truth class is in the allowed set for that kind.

### Edge kinds

| Kind | From | To | Truth class | Owner role | Meaning |
|---|---|---|---|---|---|
| `hierarchy` | Intent | Intent | asserted | builder | part-of decomposition |
| `requires` | Intent | Intent | asserted | builder/analyzer | this behavior depends on another |
| `scenario_of` | Intent | Intent | asserted | builder | child scenario to parent capability |
| `variant_of` | Intent | Intent | asserted | builder | variant to base behavior |
| `triggers` | Intent | Intent | asserted | builder/analyzer | when condition occurs, response must hold |
| `sequence` | Intent | Intent | asserted | builder | technical readiness ordering between behaviors |
| `implements` | Intent | CodeFile | asserted | builder | behavior grounded at file/locator; `role` facet defaults to `realizes` |
| `validates` | Validation | Intent | asserted | validator | proof checks behavior |
| `governs` | QualityRule | Intent | asserted | quality | norm measured against behavior |
| `targets` | Hypothesis | Intent | asserted | analyzer | hypothesis concerns intent |
| `questions` | Question | Journey or Intent | asserted | builder | product question awaiting a human answer about authored or technical meaning |
| `flags` | Finding | CodeFile | derived | sync | finding concerns codefile |
| `assesses` | Finding | CodeRule | derived | sync | finding is occurrence of code rule |
| `exposes` | InterfaceSurface | CodeFile | asserted | builder | declared surface exposed by a codefile |
| `calls` | Validation | InterfaceSurface | asserted | validator | proof exercises a surface |
| `exercises` | Validation | CodeFile | asserted | validator / Journey compiler | proof reaches a specific code entry point (public adapter or declared downstream operation exercise). For compiler-owned Journeys, S3 still requires a local compiler-v5 run; imported assertion names are audit-only. |
| `relates` | Intent | Intent | asserted | analyzer | manual relationship, kind TBD |
| `derives` | Journey | Intent | asserted | builder | accepted technical Intent derives from stable Journey steps |
| `surfaces` | Journey | InterfaceSurface | asserted | builder | accepted surface exposes every Journey step through real repository code |
| `proves` | Validation | Journey | asserted | validator | compiler-owned Validation proves the hash-bound Journey profile |

**`exposes` truth class note.** `exposes` is asserted-only. Derived `exposes` extraction was never implemented; sync does not create these edges, and attempts to add a derived `exposes` edge are rejected.

**`implements` role note.** `implements` edges carry an asserted `role` facet:
- `realizes` — behavior lives in this file/locator. This is the default when the facet is absent and the only role that owns coverage.
- `consumes` — this file exercises behavior across a seam such as a route, topic, key, import, or interface. It never owns coverage for that behavior.
- `configures` — this file supplies configuration for behavior that lives elsewhere. It never owns coverage.
- `verifies` — this file checks behavior that lives elsewhere. It never owns coverage.

Coverage and navigation use only live, non-superseded `implements` edges with `role=realizes` as owners. A CodeFile grounded only by `consumes`, `configures`, or `verifies` remains unowned until a realizing intent/file grounding exists.

Locators normally name extracted symbols. When a declaration or comment-bearing configuration entry has no stable symbol identity, Loom can issue an optional source anchor with `loom codefile anchor <path> --at-line <line>`. The command returns an exact `// loom:anchor <id>` or `# loom:anchor <id>` marker and canonical `anchor:<id>` locator without editing source or graph state. A referenced anchor must occur exactly once among registered CodeFiles and attach immediately to exactly one smallest supported declaration/config entry; malformed, missing, duplicated, detached, unsupported, and wrong-file anchors fail closed. JSON and other commentless formats cannot carry anchors.

An anchor is locator/navigation metadata only. Its source comment creates no node, edge, relationship, ownership, or proof; the graph remains authoritative. Anchor-backed realizing edges contribute their currently attached callable symbol to impact/risk navigation, but anchors are excluded from proof-facing symbol projections and can never earn S3. Use ordinary symbol locators by default and anchors only where symbol identity is unstable.

Sync reopens realizing groundings on file content changes and ripples to the intent's dependents. Non-realizing groundings reopen only when their seam locator drifts: the file vanished, or the locator string (or its last token) no longer appears in the file content. They do not ripple to the consumed/configured/verified intent.

`loom edge rehome` supersedes rather than deletes: the old grounding receives a `superseded_by` facet and stops counting for coverage, staleness, and queues; the successor grounding carries the old locator and role and receives a `stale_cause` beginning with `rehomed`.

Direction note for asymmetric edges:

```text
scenario_of:  scenario intent -> parent capability
variant_of:   variant intent -> base intent
triggers:     triggering context -> required-response intent
```

### Edge status state machine

```text
uninspected ──────────────────────────── first inspection
    │
    ├─► passing          (criterion + evidence + confidence ≥ 0.7)
    │       │
    │       └─► needs_reverification   (dependency changed via sync)
    │                   │
    │                   └─► passing | failing | independent
    │
    ├─► failing          (criterion + evidence; confidence any)
    │       │
    │       ├─► needs_reverification   (dependency changed)
    │       └─► passing | independent  (after repair)
    │
    ├─► independent      (evidence-bearing verdict: no relationship)
    │       │
    │       └─► needs_reverification   (dependency changed)
    │
    └─► blocked          (proof cannot run; carries reason)
```

`independent` is a rare asserted judgment with evidence. It is never the default; absence of a row is the default.

### Depends-on typed refs

An asserted edge records what it relied on, so sync can re-open it precisely.

```json
[
  {"node": "<id>"},
  {"edge": "<id>"},
  {"locator": "<file>:<symbol>"},
  {"scope": "<kind>", "args": {}, "fingerprint": "<hash>"}
]
```

---

## Truth classes

Every graph fact belongs to one plane by how it becomes true.

### Derived

Reproducible. Computed by `loom sync` from code, files, or graph structure. Wipe and re-run sync: byte-identical result.

Examples: file hash, imports, symbols, language, derived Finding nodes, `flags` edges, `assesses` edges.

Rules:
- Derived facts themselves are never re-judged and never enter the asserted residue queue (`loom next` truth-verdict work).
- Untriaged or stale derived Finding nodes may still surface in triage; the write is a separate asserted adjudication on the Finding id, never a conversion of the derived occurrence into asserted truth and never a rewrite of derived `flags`/`assesses` edges.
- Written only by sync (including sync-owned scan reconciliation of external diagnostics).
- Can never be stale-but-trusted as a derived occurrence: sync rebuilds or retires the fact.

### Asserted

Judgment-bearing. Written by human or LLM with evidence, criterion, and confidence. Persisted until a dependency changes.

Examples: `implements`, `governs`, `validates`, `hierarchy`, manual `relates`.

Rules:
- Only the asserted residue enters `loom next`.
- Written only through role-gated verdict paths.
- Every write must carry non-empty criterion, evidence, and confidence.

### Statistical signals (not stored)

Heuristics computed from history or structure. Never stored as edges or nodes. Never gate required work, maturity rungs, or `loom next` counts by mere existence.

Shipped examples: LOC size-outlier clusters, git-history co-change clusters. Unimplemented design examples (not computed yet): clone clusters, shotgun surgery signals, recurrence patterns, proof-locality suspicion.

Surface: `loom debt` ranked advisory feed with stable `cluster_id`s. Explicit `loom debt promote <cluster-id> --evidence <TEXT> [--confidence <0..1>]` creates exactly one asserted Finding (`source: debt_promotion`) that preserves the cluster snapshot and enters ordinary finding triage. Promotion never stores the statistical signal as an edge/node, never writes derived `flags`/`assesses`, and never enqueues the raw cluster as required residue. Follow-on facts (Hypothesis, needs_change Intent, manual edge, decision Note) emerge later from ordinary triage and existing commands, not as the first-surface promotion destinations.

---

## Facets

Facets allow nodes and edges to carry typed, canonical, searchable attributes.

### Property

A typed `key=value` attribute from the property schema registry. Canonical keys prevent drift. Enables filtering: `visibility=user_visible`, `aspect=sad`, `layer=domain`, `language=rust`.

```text
property_schema_registry
  key
  value_type:      string | enum | bool | int | float
  allowed_values:  [] (enum values)
  applies_to:      node_type | edge_kind | any
  truth_class:     derived | asserted
  description
```

Facets also have a truth class:

- `derived` facets are recomputed by sync: `language`, `loc`, `symbol_count`.
- `asserted` facets are pinned judgments: `visibility`, `layer`, `aspect`, tags.
- edge facets include `locator`, `role`, `stale_cause`, and `superseded_by`. `role` is asserted and only valid on `implements`; `superseded_by` marks a rehomed edge that no longer counts for coverage, staleness, or queues.

### Tag

A membership label from the tag vocabulary. Many-to-many. Used for grouping/discovery.

```text
tag_vocabulary
  term
  description
  created_at
```

### Portable config

Runtime configuration that must travel with the graph is exported under the top-level `config` map in `loom.graph.json`. The map's values are deterministic serialized JSON payloads keyed by:

```text
layer_order      ordered architecture layer labels from `loom layer order`
ignores          coverage-exclusion globs plus recorded reasons
codefile_globs   original registration globs from `loom codefile add`
scan_adapters    external diagnostic adapter name/command/map entries
```

Import restores these keys before continuing normal graph work, so layer ordering, ignores, registered codefile globs, and scan adapters do not silently disappear when the graph travels.

---

## Core schema enums

These are core value families used by the store and CLI. Some values are registry-backed or stored as JSON strings rather than hard `CHECK` constraints.

```text
NodeType =
  Intent | CodeFile | QualityRule | CodeRule | Validation | Hypothesis |
  Finding | Question | InterfaceSurface | Note | InboxItem | TaskRecord | Proposal |
  Journey | WikiPage | UpstreamIntent | Pattern

EdgeTruthClass =
  derived | asserted

InspectionStatus =
  current | uninspected | passing | failing | independent |
  needs_reverification | blocked

IntentLifecycle =
  planned | implemented | needs_change | deprecated

ValidationType =
  test | assertion | benchmark | manual_check | journey | scenario | contract

InterfaceKind =
  http | cli | ui_route | message_topic | sdk_method | internal_module | storage | other
```

`statistical` is a core Rust enum for `DebtSignal`/`DebtCluster` computation only. It is never a stored `edge.truth_class`.

Registry-backed (validated at write time, extensible without migration):

```text
edge_kind
property_schema_key
tag_vocabulary_term
aspect value
layer value
scope_kind
```

---

## Deterministic gap detection

The graph shape exposes gaps without cognitive judgment. The LLM confirms whether a structural gap is a real problem.

| Query | Gap type |
|---|---|
| Broad intent with no children | Decomposition gap |
| `user_visible` capability with only `happy` aspect | Missing sad/fallback/edge scenarios |
| `requires` target is `planned` | Prerequisite unimplemented |
| Scenario/variant with no `validates` edge | Proof gap |
| Leaf intent with no realizing `implements` edge | Grounding gap |
| CodeFile with no live realizing owner | Ownership gap |
| `triggers` with no response intent | Reaction gap |
| Journey step with no current `derives` mapping | Technical derivation gap |
| Journey whose current derivations are not implemented and realizing-grounded | Realization gap |
| Realized Journey with no accepted complete `surfaces` projection | Consumer-surface gap |
| Surfaced Journey with no current compiler-owned passing S3 proof | Journey proof gap |
| `governs` edge `uninspected` | Unmeasured quality norm |
| `Validation.last_result = not_run`; linked `validates` edge `needs_reverification` | Unrun proof |
| Realizing `implements` locator stale, or non-realizing seam locator drifted | Grounding mismatch after code change |
| Many unrelated intents implementing one CodeFile | Tangle |
| Two intents sharing tags in disjoint code with no edge | Duplicated responsibility; often appears after removing shared groundings that previously masked an undocumented relationship gap |
| File whose only realizing owner is an intent whose other realizing files live in a different top-level directory cluster | `consumer_owned_file` smell |
| Settled `consumes` grounding without a seam in criterion and without a locator | `consumes_without_seam` doctor issue |

---

## Key invariants

These must hold from the first commit and be tested continuously.

1. **No grid materialization.** Adding N unrelated intents produces O(N) hierarchy edges, not O(N²) rows.
2. **Derived rebuildable.** Delete all derived edges, run sync, get byte-identical result.
3. **Statistical never required.** No statistical signal is ever a stored edge, a gate input, or a `loom next` required item.
4. **Absence is default.** `independent` rows exist only with non-empty evidence.
5. **Class-partitioned writes.** Derived status is written only by sync; asserted only by a verdict path. No function writes both.
6. **Evidence gates.** An asserted edge with empty criterion or evidence is rejected at write time.
7. **Role gates.** An asserted write from a role not allowed for that edge kind is rejected at write time.
