# loom v2 — LLM Driver

Status: canonical draft. This describes how loom and an LLM cooperate: the division of responsibility, the WorkItem/PromptContract shape, role mindsets, write-back requirements, and stop conditions. Terminology follows `terminology.md`; state machine follows `state-machine.md`.

---

## Core division

```text
loom        routes, verifies, stales, queues, gates, exports, publishes
LLM         acts, judges, inspects, repairs, writes evidence, suggests
human       product authority, constraint source, reviewer when required
```

loom is not the actor. Every **asserted or cognitive** state transition requires an LLM or human write-back; loom validates and applies it. **Derived** transitions are owned by sync alone — no LLM write-back, no judgment, no queue item.

The LLM is not free-form. It reports through **typed graph writes**. Chat output alone changes nothing in the graph.

The human is not always present. loom must distinguish which queues can be drained autonomously and which require human presence.

---

## The driver loop

```
loom status / loom next
  → emit WorkItem + PromptContract

LLM adopts role and mindset
  → reads suggested context (read set)
  → acts (inspects code, runs validation, writes wiki, edits code, interviews)
  → observes evidence

LLM writes back via typed graph command
  → loom validates: role, transition, evidence, confidence

loom applies transition
  → ripples dependencies
  → routes next item

repeat
```

Every turn is: **compute → prompt → act → write-back → verify → ripple → compute**.

---

## WorkItem

A promptable unit of work emitted by `loom next`. In JSON mode the envelope is the real serialized `NextOutput`:

```json
{
  "work_item": { "...": "WorkItem or null" },
  "graph_state": {
    "planned": 0,
    "stale": 0,
    "uninspected": 0,
    "findings": 0,
    "untriaged": 0,
    "stale_findings": 0,
    "needed": 0,
    "open_findings": 0,
    "resolved_findings": 0,
    "inbox": 0,
    "open_questions": 0,
    "low_confidence": 0
  }
}
```

`graph_state.low_confidence` is the tier-coordination channel: asserted `passing` / `independent` verdicts with `0 < confidence < 0.7` route to `loom next --mode review` for independent re-inspection.

Real `WorkItem` fields:

```text
WorkItem
  mode:            build | coverage | fix | analyze | validate | quality | prove | triage | review | elaborate
  owner_role:      builder | analyzer | fixer | validator | quality
  effort:          low | mid | high
  reason:          why this item is next
  target:          { kind, id, name, from?, to? }
  stale_causes:    [] typed stale_cause facets recorded by sync
  prompt_contract: PromptContract
  context:
    purpose
    linked_entities:
      - { role, kind, id, name, description?, status?, edge_kind?, edge_status?, locator? }
    suggested_reads:
      - { reason, command }
    read_set:
      - { path, locator?, why }   # real file paths; missing files are annotated "GONE from disk"
  truth_gap:
    axis
    missing_form
    correct_when
    authoritative_write
    forbidden_write
    after_write
  next_step:       what to do after acting
```

There is no top-level `id`, `target_facts`, `allowed_commands`, or `read_set`. The target id is `work_item.target.id`; allowed actions and write-back live in `work_item.prompt_contract`; the file read set lives in `work_item.context.read_set`.

`effort` is a statement about the work, not a model. The harness maps effort to available models. loom never names vendors.

## Operator loops

Loom's current enforcement model is role-based (`LOOM_AGENT=llm:<role>`), but
operators also need a session strategy. The user or orchestrator chooses the
model and loop; the model does not self-certify capability.

**Seeding mode** spends high-capability reasoning to turn ambiguous product/code
understanding into durable graph artifacts: intents, scenario families,
prerequisites, interface boundaries, validations, journey coverage, invariant
points, reasoned waivers, and crisp product questions. Prefer graph writes over
prose summaries. Do not answer product questions for the human, and do not mark
proofs passed without observed runs.

**Draining mode** closes already-routed gaps one packet at a time. A bounded or
cheaper model can run `loom next`, inspect the packet's `read_set`, satisfy
`truth_gap.correct_when`, execute validations/journeys/scans, and record
evidence, confidence, or blocked prerequisites. Do not rediscover product shape
or invent broad graph structure; if meaning is missing, raise a linked question
or mark the proof blocked.

Invariant: mode routes work; role controls writes; evidence determines truth.
`loom guide --json` exposes this as `operator_loops` for LLM lookup.

---

## PromptContract

The LLM-facing contract embedded in a WorkItem. Defines mindset, allowed actions, forbidden actions, required evidence, write-back, and stop.

```json
{
  "role": "validator",
  "mindset": "Run it; do not guess. Record exactly what the command produced.",
  "why_now": "validates edge is needs_reverification",
  "allowed_actions": [
    "run: cargo test auth::session_restores",
    "loom validation run 'remember-me token restores session after browser restart'",
    "loom validation verdict 'remember-me session test' <passed|failed|blocked> --evidence '…'"
  ],
  "forbidden_actions": [
    "edit code to make the proof pass",
    "mark passed without observed proof"
  ],
  "required_evidence": "command output, test count, failure message, or a concrete blocker reason",
  "write_back": "loom validation run 'remember-me token restores session after browser restart'  (or)  loom validation verdict 'remember-me session test' <passed|failed|blocked> --evidence '…'",
  "stop_condition": "after recording the result, return to loom status",
  "human_gate": null
}
```

Every `loom next --json` item carries this contract inside `work_item.prompt_contract`. The LLM reads it before acting.

### Quality WorkItem PromptContract shape

When `loom next --mode quality` serves a `governs` edge or the fallback never-measured `(rule × root implemented intent)` pair, the rule's inspection metadata enriches the contract in the real serialized shape. Quality metadata is not nested under `prompt_contract.context.rule`.

```json
{
  "work_item": {
    "mode": "quality",
    "owner_role": "quality",
    "effort": "mid",
    "reason": "rule 'service-auth-at-boundary' has never been measured against 'admin delete user' — the verdict creates the governs edge",
    "target": {
      "kind": "rule_intent_pair",
      "id": "intent-id",
      "name": "service-auth-at-boundary —governs?→ admin delete user",
      "from": "service-auth-at-boundary",
      "to": "admin delete user"
    },
    "prompt_contract": {
      "role": "quality",
      "mindset": "Measure this rule at the highest honest altitude. Follow the rule's inspection guide; do not invent your own protocol. independent requires evidence of non-applicability. Phrase evidence with the rule's evidence_template so verdicts are comparable across sessions. Guide: Find handlers and verify auth before side effects.",
      "why_now": "'service-auth-at-boundary' is seeded but unmeasured against this intent",
      "allowed_actions": [
        "loom codefile show <file>",
        "read the grounded code",
        "loom rule verdict 'service-auth-at-boundary' 'admin delete user' <passing|failing|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
        "hint: grep: require_auth, require_admin, authenticate, middleware",
        "hint: red flag: handler body mutates state before an auth check"
      ],
      "forbidden_actions": [
        "edit code",
        "mark passing without inspecting",
        "mark independent without evidence the rule does not apply"
      ],
      "required_evidence": "file/line locators showing compliance, violation, or non-applicability",
      "evidence_template": {
        "passing": "src/<file>:<lines> — all handlers check <auth method> before side effect",
        "failing": "src/<file>:<lines> — <handler> calls <mutation> before auth"
      },
      "examples": {
        "passing": {
          "criterion": "every DELETE handler verifies admin role before executing",
          "evidence": "src/routes/admin.rs:12-40 — require_admin() called at line 14 before delete_user()",
          "confidence": 0.92
        },
        "failing": {
          "criterion": "every DELETE handler verifies admin role before executing",
          "evidence": "src/routes/admin.rs:78-84 — delete_user() runs with no preceding auth check",
          "confidence": 0.95
        }
      },
      "pre_screened_hits": [
        {
          "path": "src/routes/admin.rs",
          "line": 78,
          "pattern": "delete_user\\(",
          "excerpt": "delete_user(id);"
        }
      ],
      "write_back": "loom rule verdict 'service-auth-at-boundary' 'admin delete user' <passing|failing|independent> --criterion '…' --evidence '…' --confidence <0.0-1.0>",
      "stop_condition": "after recording the verdict, return to loom status",
      "human_gate": null
    },
    "context": {
      "purpose": "Read the rule's inspection guide, then measure the intent's grounded code against it.",
      "linked_entities": [
        {
          "role": "target",
          "kind": "intent",
          "id": "intent-id",
          "name": "admin delete user",
          "description": "admins can delete a user only after authorization"
        },
        {
          "role": "measuring_rule",
          "kind": "quality_rule",
          "id": "rule-id",
          "name": "service-auth-at-boundary",
          "description": "externally reachable endpoints authenticate before side effects"
        }
      ],
      "suggested_reads": [
        { "reason": "the measuring stick — its inspection guide and examples", "command": "loom rule show rule-id" }
      ],
      "read_set": [
        { "path": "src/routes/admin.rs", "locator": "delete_user", "why": "grounded implementation for the target intent" }
      ]
    },
    "truth_gap": {
      "axis": "verdict",
      "missing_form": "an asserted claim is uninspected or stale",
      "correct_when": "every asserted edge status was earned by fresh inspection: the criterion states what would falsify the claim, the evidence cites file/line or runtime output that was actually read, and the confidence is honest — below 0.7 is a legitimate answer that routes to review",
      "authoritative_write": "record the verdict with criterion, evidence, and honest confidence",
      "forbidden_write": "code edits, proof runs, or prose summaries that do not update the asserted edge",
      "after_write": "return to loom status so the router can recompute"
    },
    "next_step": "after recording the verdict, run `loom status`"
  },
  "graph_state": {
    "planned": 0,
    "stale": 0,
    "uninspected": 0,
    "findings": 0,
    "untriaged": 0,
    "stale_findings": 0,
    "needed": 0,
    "open_findings": 0,
    "resolved_findings": 0,
    "inbox": 0,
    "open_questions": 0,
    "low_confidence": 0
  }
}
```

Rule-authored `detection_hints` are folded into `prompt_contract.allowed_actions` as `hint: ...`. Rule-authored phrasing lives at `prompt_contract.evidence_template`; few-shot verdicts live at `prompt_contract.examples`. For rules with `detection_kind=pattern`, `patterns[]` are regex strings run over the target intent's grounded files when the quality packet is built; the resulting `pre_screened_hits` (`path`, `line`, `pattern`, `excerpt`) are candidates only, are never stored, and must be confirmed or refuted by the LLM before writing a verdict.

---

## Roles and mindsets

### builder

Realizes planned or needs_change intents in code. Connects behavior to files. Never self-certifies quality or proofs.

```text
mindset:
  Realize the behavior this intent describes.
  Ground it to the right file and symbol.
  Functions and symbols are locators on the implements edge, not intents.
  Ask: does the behavior LIVE in this file? If yes, use `implements --role realizes`.
  If the file only calls behavior elsewhere, create the realizing intent for this surface
    and add `implements --role consumes` edges for the consumer seams instead.
  If an intent name looks like a function name (snake_case, no spaces), challenge it:
    confirm a behavioral criterion exists in the description;
    if the intent should be a locator instead, capture as InboxItem and propose
    the right model (behavioral intent + implements locator) rather than proceeding.
  Add a validation stub if none exists.
  Run sync after code changes.
  Do not mark proof passing — that is validator work.

allowed:
  edit code
  loom intent update <intent> --lifecycle implemented --reason '…'
  loom edge implement --role realizes|consumes|configures|verifies
  loom validation add (stub only)
  loom sync
  loom inbox add (for out-of-scope findings or suspect intents)
  loom intent add --allow-symbol-name (only when name is a known public symbol
    with a genuine behavioral criterion; must provide full --description)

forbidden:
  loom validation verdict passed (validator role)
  loom rule verdict passing (quality role)
  loom edge explore ground (analyzer role)
  creating intents that are just function/method names with no behavioral criterion

evidence required:
  code written, locator confirmed, sync clean
```

### analyzer

Inspects relationships, hypotheses, and conceptual truth. Does not fix code; does not certify quality.

```text
mindset:
  Read both sides.
  Form hypothesis before inspecting code.
  Inspect actual code and evidence.
  Record exactly what was found.
  Do not preserve old verdict by assumption.

allowed:
  loom edge explore ground / issue / independent
  loom hypothesis prove
  loom inbox add (out-of-scope findings)
  read codefiles, notes, prior evidence

forbidden:
  edit code
  loom intent update --lifecycle (builder role)
  loom rule verdict (quality role)

evidence required:
  file/line locators, validation output, or runtime evidence; not name similarity or assumption
```

### fixer

Repairs known failing behavior at its root cause. Never records verdicts: after the repair, `loom sync` re-opens the claim and its owning lane (analyze/quality/validate) re-measures. Preserves intent meaning unless evidence shows the product itself changed.

```text
mindset:
  Fix the actual broken behavior, not the symptom.
  Code moving is not the same as behavior changing.
  If the product changed, route through intent update, not silent code change.
  Sync and re-route proofs after repair.

allowed:
  edit code
  loom sync
  loom edge implement --role realizes|consumes|configures|verifies (re-ground after fix)
  loom intent update <intent> --lifecycle implemented --reason '…' (after confirmed fix)
  loom inbox add (out-of-scope findings)

forbidden:
  loom intent update --description (unless evidence confirms meaning changed)
  mark verdicts passing without re-verification
  suppress symptoms without root cause fix

evidence required:
  code change, sync clean, stale cause resolved
```

### validator

Runs or marks proofs. Records what was actually observed.

```text
mindset:
  Run it. Do not guess.
  Record exactly what the command produced.
  Do not edit code to make the proof pass.
  A blocked proof is honest; record it with a reason.

allowed:
  run validation command
  loom validation verdict <validation> passed|failed|blocked
  loom journey run
  loom validation run <intent>

forbidden:
  edit source code
  mark passed without observed proof
  mark failed without inspecting the failure
  change validation command to suppress a real failure

evidence required:
  command stdout/stderr, test count, failure message, or blocking reason
```

### quality

Measures QualityRules against Intents. Records verdicts with criterion and evidence. The rule's `inspection_guide`, `detection_hints`, `evidence_template`, and examples are embedded in the PromptContract — use them; do not invent your own inspection protocol.

```text
mindset:
  Measure the intent at the highest honest altitude.
  A rule passing at component level covers descendants unless a leaf needs a specific verdict.
  independent means "measured; this rule does not apply here" — it requires evidence.
  Follow the rule's inspection_guide — it is the consistent protocol for this rule.
  Use detection_hints to focus grep/read; do not guess what to look for.
  If pre_screened_hits are attached, inspect and confirm or refute EVERY hit.
  Use evidence_template phrasing so verdicts are comparable across sessions.
  Never invent compliance; inspect the code.

allowed:
  loom rule verdict <rule> <intent> passing/failing/independent
    --criterion ... --evidence ... --confidence <n>
  loom codefile show
  read codefiles, detection hints, prior notes
  inspect pre_screened_hits as candidates, not conclusions

forbidden:
  edit code
  mark passing without inspecting
  mark independent without evidence that the rule is not applicable
  deviate from the rule's inspection_guide without a recorded reason

evidence required:
  file/line locators showing compliance, violation, or confirmed non-applicability;
  phrased using evidence_template when possible
```

### interviewer

Translates human language into graph options. One question per turn. Captures before proposing.

```text
mindset:
  Translate, do not decide.
  Humans speak product language; the graph uses typed nodes.
  One question closes one gap.
  Capture the answer as InboxItem first; use the door landing menu to route it.
  Product decisions belong to the human, not the LLM.
  Intents labeled internal are not interview material.

allowed:
  loom door "<utterance>"
  loom inbox mark <key> routed|rejected|duplicate|deferred
  loom intent add / update / retire / confirm
  loom hypothesis add
  loom task add --kind investigation
  loom session (turn-zero offer)
  ask one clarifying question if materially ambiguous

forbidden:
  edit code
  make product decisions without explicit human confirmation
  ask multiple questions per turn
  flood user with graph vocabulary they did not request

evidence required:
  human confirmation, door capture id, or explicit decision recorded as Note
```

### elaborate builder loop

`loom next --mode elaborate` serves the most-incomplete user-visible feature intent. The packet embeds the Definition-of-Complete scorecard, including the open axes: `scenarios`, `prerequisites`, `boundary`, `proof`, `journey`, and `questions`.

The loop is deliberately cognitive-cognitive:

```text
LLM proposes missing surroundings
  → add scenario intents with --aspect sad|fallback|edge_case and scenario-of edges
  → add prerequisite edges or proof/journey coverage where the answer is graph-derivable
  → raise product decisions as questions:
       loom inbox add "<one crisp product question>" --source question --link intent:<id>
  → waive non-question axes only with a real reason:
       loom intent waive <intent> <axis> --reason "<why it deliberately does not apply>"

human answers batched questions
  → surfaced by loom session and graph_state.open_questions
  → route/answer the linked InboxItems before the questions axis closes
```

The LLM must not answer product questions for the human. It either creates the missing graph artifact, records a non-question waiver with a reason, or raises one crisp linked question.

### coverage / missing-file contract

A coverage WorkItem points at a registered `CodeFile` with no live realizing owner. A file grounded only by `consumes`, `configures`, or `verifies` remains unowned because those roles describe support for behavior that lives elsewhere.

Ask the disambiguating question before writing back: does the behavior live in this file (`realizes`), or does this file only call behavior across a route/topic/key/import seam (`consumes`)? For a consumer file, create the realizing intent for the behavior surface and add consumes edges for the callers; the consumer file stays unowned until its realizing intent exists.

When that file is missing from disk, the packet is a dedicated missing-file contract:

- `read_set` is empty — there is nothing to read.
- `prompt_contract.mindset`: the file is gone; do not try to inspect it.
- `allowed_actions`: identify successor file(s), re-ground affected intents there, then `loom codefile remove` the ghost.
- `write_back`: `loom codefile remove` after any re-grounding.

If a grounded file mentioned in another lane's `read_set` is deleted, the read_set entry carries a `GONE from disk` annotation so the worker knows the locator is stale before trying to read it.

### wiki author (served by `loom wiki next`, not `loom next`)

Wiki work is not a gated WorkItem lane: `loom wiki next` emits a verified brief — the documented intents' descriptions, groundings, and proof status — and the agent writes reader-first prose at the page's path, then stamps it fresh with `loom wiki record <title>`. The mindset: explain the graph, never contradict it; a claim the brief cannot support is documentation drift — route it through `loom inbox add ... --source wiki` (or `--source code_audit` when found during code review) instead of writing it.

---

## Write-back contract

The LLM is a state reporter. It reports through typed graph commands, not chat prose.

### Wrong

```text
"I checked the code and it looks secure. The auth is fine."
```

### Right

```text
loom rule verdict "service-auth-at-boundary" "admin delete user" \
  passing \
  --criterion "delete endpoint checks admin permission before side effect" \
  --evidence "src/routes/admin.rs:42-61 requires admin before delete_user call" \
  --confidence 0.91
```

loom validates the write-back:

```text
role allowed for this edge kind?
transition legal from current status?
criterion non-empty?
evidence non-empty?
confidence in range?
confidence < 0.7 → route to review queue?
target nodes exist?
dependency stale cause handled?
```

---

## Out-of-scope finding capture

While working a `WorkItem`, the LLM may encounter something outside the current task.

**Do not silently fix it. Do not dismiss it. Capture it.**

```text
loom inbox add "<finding>" --source code_audit --link <node-or-file>
```

Route after the current item is complete. The inbox is the holding area; the graph is the durable record.

This applies to:

```text
debt noticed while building
ambiguous requirement found while analyzing
missing validation noticed while quality-checking
security concern spotted while fixing
documentation inaccuracy found while reviewing
```

A card is one line to dismiss later. An uncaptured gap is gone from context forever.

---

## Stop conditions

The LLM must not wander between queues without surfacing to `loom status` / `loom next`. Each WorkItem carries a stop condition.

Canonical stop conditions:

```text
after recording verdict: run loom status
after code change: run loom sync, then loom status
after grounding: run loom sync, then loom status
after capturing InboxItem: return to current WorkItem
after routing InboxItem: mark routed/rejected/duplicate/deferred, then loom status
after interview turn: wait for human response or run loom status
```

Escalation rules:

```text
confidence < 0.7 after inspection:
  record verdict with actual confidence; review queue routes it

out-of-scope product decision:
  capture as InboxItem; do not decide; return to WorkItem

missing prerequisite for human gate:
  record blocked with reason; return to loom status
```

---

## Human gate patterns

Some queues require human presence. loom distinguishes autonomous and human-gated queues.

```text
autonomous until they raise a human question (LLM drains alone):
  build, elaborate, fix, validate, quality, analyze, prove, triage, review

human-gated (requires human):
  align/product meaning re-confirmation
  blocked proofs (external prerequisite or credential)
  hypothesis adoption/rejection rulings
  major product decisions captured through InboxItem routing
```

`loom session` and `graph_state.open_questions` surface the human-gated remainder so the LLM can batch questions for one conversation window instead of interrupting repeatedly.

---

## Effort tiers

Every WorkItem carries `effort: low | mid | high`. This is a statement about the work, computed from graph structure.

```text
low:   mechanical grounding, evidence-only re-verification, simple proof re-run
mid:   relationship re-inspection, quality measurement, finding/inbox triage
high:  design reasoning, hypothesis proof, complex repair, intent alignment
```

Mapping to models/agents is the harness's job. loom never names vendors.

Low-confidence verdicts (< 0.7) route to the review queue regardless of original effort tier, because confidence is the coordination channel between tiers.

---

## Session start protocol

When the session begins without a specific task:

```text
loom session
  → emits offer menu: every way this session could be spent
  → each offer backed by a live queue/count or graph-state signal
  → surfaces graph_state.open_questions when product questions are waiting
  → one recommended command from the compass
  → LLM asks human one question: "what do you want from this session?"

human answers
  → loom door "<answer>"
  → InboxItem captured with a landing menu
  → LLM runs the chosen typed command
  → loom inbox mark <id> routed
```

When the session continues from a known state:

```text
loom status
  → maturity + compass
  → graph_state, validation summary, code ownership, queues, and debt pulse

loom next
  → single highest-priority WorkItem with full PromptContract
```
