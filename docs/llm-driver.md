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

A promptable unit of work emitted by `loom next`. It carries everything needed to act without a second lookup.

```text
WorkItem
  id
  mode:             build | fix | validate | quality | analyze | align | prove | wiki | inbox | review
  owner_role:       builder | analyzer | fixer | validator | quality | interviewer | wiki_author | wiki_reviewer
  effort:           low | mid | high
  reason:           why this item is next (stale cause, gap, queue trigger)
  target_facts:     [] nodes/edges/codefiles directly involved
  context_refs:     [] suggested supporting facts (hierarchy, related, prior notes)
  stale_causes:     [] typed dependency refs that triggered staleness
  read_set:         [] suggested files/symbols/locators to inspect
  allowed_commands: [] exact CLI commands this role may run for this item
  evidence_shape:   description of what evidence is required
  write_back:       exact command(s) to record result
  stop_condition:   when to return to loom status/next
  human_gate:       null | reason this item needs human presence
  prompt_contract:  embedded PromptContract for this item
```

`effort` is a statement about the work, not a model. The harness maps effort to available models. loom never names vendors.

---

## PromptContract

The LLM-facing contract embedded in or alongside a WorkItem. Defines mindset, allowed actions, forbidden actions, required evidence, write-back, and stop.

```json
{
  "role": "validator",
  "mindset": "Run or mark proof honestly. Do not edit code to make it pass.",
  "why_now": "Validation.last_result reset to not_run after src/auth/session.rs changed.",
  "context": {
    "validation": { "id": "...", "command": "cargo test auth::session_restores" },
    "intent": { "id": "...", "name": "remember-me token restores session after browser restart" },
    "codefiles": ["src/auth/session.rs", "src/auth/cookies.rs"],
    "stale_cause": "src/auth/session.rs content hash changed"
  },
  "allowed_actions": [
    "run validation command",
    "loom validation mark passed --evidence ...",
    "loom validation mark failed --evidence ...",
    "loom validation mark blocked --reason ..."
  ],
  "forbidden_actions": [
    "edit source code",
    "change intent meaning or description",
    "mark passed without observed proof"
  ],
  "required_evidence": "command output, test run result, or concrete blocker reason",
  "write_back": "loom validation mark <id> --result <passed|failed|blocked> --evidence '...'",
  "stop_condition": "After recording result, run loom status.",
  "human_gate": null
}
```

Every `loom next` item should produce a PromptContract in `--json` output. The LLM reads it before acting.

### Quality WorkItem PromptContract shape

When `loom next --mode quality` serves a governs edge, the rule's inspection metadata enriches the contract. This is the primary mechanism for making quality verdicts consistent across LLM sessions.

```json
{
  "role": "quality",
  "mindset": "Measure at the highest honest altitude. independent requires evidence.",
  "why_now": "governs edge uninspected: service-auth-at-boundary → admin delete user",
  "context": {
    "intent": { "id": "...", "name": "admin can delete user", "level": "feature" },
    "codefiles": ["src/routes/admin.rs", "src/middleware/auth.rs"],
    "rule": {
      "name": "service-auth-at-boundary",
      "description": "every externally reachable endpoint authenticates before side effects",
      "category": "security",
      "effort": "mid",
      "detection_kind": "llm_judgment",
      "inspection_guide": "1. Find all handlers for this intent's routes. 2. Check whether each handler verifies authentication before any write/delete/mutate. 3. Look for middleware, guards, or explicit auth calls. 4. If any handler skips auth before a side effect: failing.",
      "detection_hints": [
        "grep: require_auth, require_admin, authenticate, @guard, middleware",
        "red flag: handler body begins with a DB call before any auth check",
        "tree-sitter: function_item where first expression is not an auth call"
      ],
      "evidence_template": {
        "passing": "src/<file>:<lines> — all handlers check <auth method> before side effect",
        "failing": "src/<file>:<lines> — <handler> calls <mutation> at line <N> with no auth check"
      },
      "passing_example": {
        "criterion": "every DELETE handler verifies admin role before executing",
        "evidence": "src/routes/admin.rs:12-40 — require_admin() called at line 14 before delete_user()",
        "confidence": 0.92
      },
      "failing_example": {
        "criterion": "every DELETE handler verifies admin role before executing",
        "evidence": "src/routes/admin.rs:78 — delete_user() called at line 82 with no preceding auth check",
        "confidence": 0.95
      }
    },
    "pre_screened_hits": []
  },
  "allowed_actions": [
    "loom rule verdict 'service-auth-at-boundary' 'admin delete user' --status passing|failing|independent --criterion ... --evidence ... --confidence ..."
  ],
  "forbidden_actions": [
    "edit code",
    "mark passing without inspecting",
    "mark independent without evidence the rule is not applicable here"
  ],
  "required_evidence": "file/line locators showing compliance, violation, or confirmed non-applicability",
  "write_back": "loom rule verdict <rule> <intent> --status <s> --criterion '...' --evidence '...' --confidence <n>",
  "stop_condition": "After recording verdict, run loom status."
}
```

`pre_screened_hits` is populated by sync when `detection_kind=pattern`. Empty when `detection_kind=llm_judgment`.

---

## Roles and mindsets

### builder

Realizes planned or needs_change intents in code. Connects behavior to files. Never self-certifies quality or proofs.

```text
mindset:
  Realize the behavior this intent describes.
  Ground it to the right file and symbol.
  Functions and symbols are locators on the implements edge, not intents.
  If an intent name looks like a function name (snake_case, no spaces), challenge it:
    confirm a behavioral criterion exists in the description;
    if the intent should be a locator instead, capture as InboxItem and propose
    the right model (behavioral intent + implements locator) rather than proceeding.
  Add a validation stub if none exists.
  Run sync after code changes.
  Do not mark proof passing — that is validator work.

allowed:
  edit code
  loom intent mark --lifecycle implemented
  loom edge implement
  loom validation add (stub only)
  loom sync
  loom inbox add (for out-of-scope findings or suspect intents)
  loom intent add --allow-symbol-name (only when name is a known public symbol
    with a genuine behavioral criterion; must provide full --description)

forbidden:
  loom validation mark passed (validator role)
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
  loom intent mark --lifecycle (builder role)
  loom rule verdict (quality role)

evidence required:
  file/line locators, validation output, or runtime evidence; not name similarity or assumption
```

### fixer

Repairs known failing or stale behavior at its root cause. Preserves intent meaning unless evidence shows the product itself changed.

```text
mindset:
  Fix the actual broken behavior, not the symptom.
  Code moving is not the same as behavior changing.
  If the product changed, route through intent update, not silent code change.
  Sync and re-route proofs after repair.

allowed:
  edit code
  loom sync
  loom edge implement (re-ground after fix)
  loom intent mark --lifecycle implemented (after confirmed fix)
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
  loom validation mark passed / failed / blocked
  loom saga run
  loom validate <intent>

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
  Use evidence_template phrasing so verdicts are comparable across sessions.
  Never invent compliance; inspect the code.
  Pattern rules: review pre_screened_hits first; confirm or refute each hit.

allowed:
  loom rule verdict <rule> <intent> --status passing/failing/independent
    --criterion ... --evidence ... [--evidence-locator file:lines] --confidence <n>
  loom codefile show
  read codefiles, detection hints, prior notes

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
  Capture the answer as InboxItem first, then normalize.
  Product decisions belong to the human, not the LLM.
  Intents labeled internal are not interview material.

allowed:
  loom door "<utterance>"
  loom inbox normalize
  loom intent add / update / retire / confirm
  loom session (turn-zero offer)
  ask one clarifying question if materially ambiguous

forbidden:
  edit code
  make product decisions without explicit human confirmation
  ask multiple questions per turn
  flood user with graph vocabulary they did not request

evidence required:
  human confirmation or explicit decision recorded as Note
```

### wiki_author

Generates wiki projection from graph facts with citations.

```text
mindset:
  Every claim must cite a graph fact.
  Do not invent architecture prose.
  A semantic change to the wiki must become an InboxItem, not a direct graph mutation.
  The graph is the source; the wiki explains it.

allowed:
  loom wiki generate / update
  read graph facts, codefiles, notes, validation results
  write wiki pages with citations

forbidden:
  loom intent update (builder role)
  loom edge explore ground (analyzer role)
  writing wiki claims without graph evidence
  treating wiki as source of truth

evidence required:
  graph node/edge ids, codefile locators, or validation evidence for each factual claim
```

### wiki_reviewer

Verifies wiki citations, staleness, and routes semantic drift.

```text
mindset:
  Check citations against current graph.
  Stale citations mean stale page — flag, do not silently accept.
  A wiki claim that contradicts the graph is a graph question, not a prose fix.
  Semantic differences become InboxItems, not silent edits.

allowed:
  loom wiki verify
  loom inbox add (for semantic drift findings)
  flag stale pages

forbidden:
  accepting stale citations as valid
  silently updating graph truth via wiki edit

evidence required:
  citation validity result, stale page list, drift description for routed InboxItems
```

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
  --status passing \
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
wiki inaccuracy found while reviewing
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
after wiki page written: run loom wiki verify, then loom status
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
autonomous (LLM drains alone):
  build, fix, validate, quality, analyze, prove, wiki, inbox normalize

human-gated (requires human):
  align (product meaning re-confirmation)
  blocked proofs (external prerequisite or credential)
  hypothesis adoption/rejection rulings
  major design decisions in InboxItem normalization
```

`loom status` surfaces the human-gated remainder so the LLM can batch questions for one conversation window instead of interrupting repeatedly.

---

## Effort tiers

Every WorkItem carries `effort: low | mid | high`. This is a statement about the work, computed from graph structure.

```text
low:   mechanical grounding, evidence-only re-verification, simple proof re-run
mid:   relationship re-inspection, quality measurement, wiki page generation
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
  → each offer backed by a live queue and count
  → one recommended (scarcity order: human-gated first, then autonomous backlog)
  → LLM asks human one question: "what do you want from this session?"

human answers
  → loom door "<answer>"
  → InboxItem captured
  → normalized to graph command or route
  → session proceeds
```

When the session continues from a known state:

```text
loom status
  → maturity + compass
  → one_turn plan: lane, role, guide command, next queue

loom next
  → single highest-priority WorkItem with full PromptContract
```
