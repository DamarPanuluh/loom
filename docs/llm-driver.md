# loom v2 — LLM Driver

Status: canonical draft. This describes how loom and an LLM cooperate: the division of responsibility, the WorkItem/PromptContract shape, role mindsets, write-back requirements, and stop conditions. Terminology follows `terminology.md`; state machine follows `state-machine.md`.

---

## Core division

```text
loom        routes, verifies, stales, queues, gates, exports, publishes
LLM         acts, judges, inspects, repairs, writes evidence, suggests
human       product authority, constraint source, reviewer when required
```

loom is not the actor. Every **asserted or cognitive** state transition requires an LLM or human write-back; loom validates and applies it. **Derived** transitions are owned by sync alone — no LLM write-back or re-judgment of the machine fact; untriaged or stale Finding nodes may still route to triage for a separate asserted adjudication.

The LLM is not free-form. It reports through **typed graph writes**. Chat output alone changes nothing in the graph.

Pattern drafts may be model-authored, but Pattern ratification remains human
INV-8 authority. An LLM may present the decision and record the human's explicit
host response; it may not supply that response itself. Build/fix packets automatically include applicable, live
Pattern guidance under deterministic count/byte budgets; use the packet's exact
`pattern lookup` command to recover omitted matches. Pattern
guidance adds no maturity gate.

The human is not always present. loom must distinguish which queues can be drained autonomously and which require human presence.

When repository knowledge lacks a current external fact, create a bounded
`kind=research` TaskRecord rather than guessing. The Analyze packet directs the
host LLM to search and browse, prefer primary authoritative sources, read actual
pages, and write each page back with `loom task source-add`. Search snippets are
never evidence. Research outcomes remain dated advisory context—not Fact
verification, human preference, professional authority, or certification—and
may conclude conflicting, inconclusive, or expert review required. This is an
available escalation, not a requirement to browse for every packet.

---

## The driver loop

### Brownfield cold start

For an existing codebase, use this machine-safe sequence rather than treating
`door` as the only entrance:

1. Verify the binary with `loom --version`. If existing `.loom` state predates
   schema v12, preserve it, initialize a fresh v12 graph, and reconstruct
   authored meaning from product evidence; there is no automatic migration.
   `loom sync --rebuild` only reconstructs derived structural state in an
   already compatible v12 graph; it does **not** migrate or reconstruct a
   pre-v12 graph.
2. Run `loom init`, register real source scope with the supported syntax
   `loom codefile add '<glob>'`, then run `loom sync --json`.
3. Run `loom bootstrap suggest`. Suggestions and clues are non-authoritative:
   inspect product evidence and relevant code before adopting structure.
4. Author one or more `loom.journey/v1` root artifacts from that evidence and
   register each with `loom journey add <journey.json>`.
5. Run `loom journey derive <journey> --json`. At the authority gate, stop and
   obtain the human's exact substantive answer—never compose, infer, or
   paraphrase it—before recording acceptance.
6. Continue through `loom journey surface <journey> --json`. Build a stable,
   production-owned black-box consumer/administrative CLI over the same
   application, API, or service boundary as the public behavior. Do not use a
   feature-gated proof binary, test fixture, mock-only path, or privileged
   internal shortcut. Compile the exact profile and run it. `loom door`
   remains the intake path for raw human or external utterances, not the sole
   cold-start route.

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
  mode:            derive | surface | build | coverage | fix | analyze | validate | quality | prove | triage | review | elaborate | rectify | ratify | audit | deepen
  owner_role:      builder | analyzer | fixer | validator | quality | rectify | human
  effort:          low | mid | high
  routing_hint:    mechanical | judgment   # optional; orchestrators map to model tiers
  reason:          why this item is next
  target:          { kind, id, name, from?, to? }
  stale_causes:    [] typed stale_cause facets recorded by sync — symbol-scoped where the
                   grounding locator resolves ("symbol 'x' in <file> changed"), graded by
                   evidence anchoring ("cited evidence intact, cheap re-confirm" vs
                   "cited evidence rewritten, full re-inspection")
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

`loom next --mode <m> --all` roster rows also carry `routing_hint` and (for edge items) `cause_class` (`cheap` | `full` | `other`) so an orchestrator can select mechanical residue without opening every full packet.

There is no top-level `id`, `target_facts`, `allowed_commands`, or `read_set`. The target id is `work_item.target.id`; allowed actions and write-back live in `work_item.prompt_contract`; the file read set lives in `work_item.context.read_set`.

`effort` is a statement about the work, not a model. The harness maps effort to available models. loom never names vendors.

## Operator loops

Loom's enforcement model is role-based (`LOOM_AGENT=llm:<role>`), while worker
attribution is independent (`LOOM_AGENT_PROFILE=<profile>`, for example
`loom-auditor`). Loom validates both once before locking and passes that typed
identity through facts, journals, locks, `whoami`, and self-audit. The profile
explains who claimed to execute; the role alone decides what may be written.
Environment profiles are explicitly reported as self-declared
(`source=environment`, `verified=false`). Bare, empty, or noncanonical
`LOOM_AGENT` values fail closed rather than masquerading as solo.
Operators also need a session strategy. The user or orchestrator chooses the
model and loop; the model does not self-certify capability.

**Seeding mode** spends high-capability reasoning to turn ambiguous product/code
understanding into durable graph artifacts: authored Journeys, human-authorized
technical derivations, scenario families, prerequisites, interface boundaries,
ordinary Intent validations, reasoned exemptions, and crisp product questions. Prefer graph writes over
prose summaries. Do not answer product questions for the human, and do not mark
proofs passed without observed runs. On a cold graph with codefiles registered
and no authored Journey roots, use `loom bootstrap suggest` only for
non-authoritative clues; inspect product evidence, author `loom.journey/v1`
roots, and register them with `loom journey add`. Never adopt inferred code
structure directly as product meaning or invent a spine as `implemented`.

**Draining mode** closes already-routed gaps one packet at a time. A bounded or
cheaper model can run `loom next`, inspect the packet's `read_set`, satisfy
`truth_gap.correct_when`, execute validations, compiled Journey profiles, or scans, and record
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
    "for an explicit type=manual_check only: loom validation verdict 'remember-me session check' <passed|failed|blocked> --evidence '…'"
  ],
  "forbidden_actions": [
    "edit code to make the proof pass",
    "mark passed without observed proof"
  ],
  "required_evidence": "command output, test count, failure message, or a concrete blocker reason",
  "write_back": "loom validation run 'remember-me token restores session after browser restart'; only type=manual_check may instead use loom validation verdict <validation> <passed|failed|blocked> --evidence '…'",
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
    if the intent should be a locator instead, capture the modeling concern as a finding and propose
    the right model (behavioral intent + implements locator) rather than proceeding.
  Add a validation stub if none exists. Do not attach or mutate proof evidence
    yourself — attaching a non-Journey validator's entry surface (`loom edge exercises`)
    is validator work; for Journeys, declare operation `exercises` on the surface
    manifest instead. Hand proof settlement to the validator contract.
  Run sync after code changes.
  Do not mark proof passing — that is validator work.

  For a derive packet, the authored Journey is the root. Map every stable step
    to the smallest falsifiable technical Intents, but do not accept the manifest
    until the human authorizes that exact Journey hash and mapping.
  For a surface packet, implement the packet's structured CLI contract as real
    source in the target repository, then bind every step to a reusable operation.
    Never substitute an executable string or ask Loom to generate application source.

allowed:
  edit code
  loom intent update <intent> --lifecycle implemented --reason '…'
  loom edge implement --role realizes|consumes|configures|verifies
  loom validation add (stub only)
  loom journey derive <journey> (read-only)
  loom journey derive-accept <journey> --manifest <file> --human-decision '<exact human answer>'
  loom journey surface <journey> (read-only)
  loom journey surface-accept <journey> --manifest <file>
  loom sync
  loom finding add '<claim>' --source code_audit --kind code_audit --file <registered-codefile> --evidence '<file:line — observed fact>' --impact '<why it matters>' --confidence <0.0-1.0>
  loom intent add --allow-symbol-name (only when name is a known public symbol
    with a genuine behavioral criterion; must provide full --description)

forbidden:
  loom validation run <validation> (validator role; manual verdict only for explicit manual_check)
  loom rule verdict passing (quality role)
  loom edge explore ground (analyzer role)
  creating intents that are just function/method names with no behavioral criterion
  inventing a human derivation decision or accepting a stale hash-bound manifest
  surfacing a Journey before every current derivation is accepted, implemented, and realizing-grounded
  recording a passing Journey proof from the builder role

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
  loom finding add '<claim>' --source code_audit --kind code_audit --file <registered-codefile> --evidence '<file:line — observed fact>' --impact '<why it matters>' --confidence <0.0-1.0>
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
  loom finding add '<claim>' --source code_audit --kind code_audit --file <registered-codefile> --evidence '<file:line — observed fact>' --impact '<why it matters>' --confidence <0.0-1.0>

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
  S3 belongs to this validation's own entry path: inspect `validation show` call_evidence; an intent-wide fallback is visible but not eligible.
  For compiler-owned Journeys, declare cross-process downstream entries on the surface operation `exercises` array — never `loom edge exercises`.
  Record exactly what the command produced.
  Do not edit code to make the proof pass.
  A blocked proof is honest; record it with a reason.

allowed:
  run validation command
  loom validation run <validation>
  # only for an explicit type=manual_check:
  loom validation verdict <validation> passed|failed|blocked --evidence '…'
  loom journey compile <journey> --profile proof
  loom journey run <journey> --profile proof
  loom journey diagnose <journey> --profile proof [--input <key=json>]...
  loom validation run <intent>
  loom edge exercises <validation> <codefile> --locator <entry-symbol> (only for non-Journey validations when command derivation cannot identify the custom runner)

forbidden:
  edit source code
  mark passed without observed proof
  mark failed without inspecting the failure
  rely on an intent-wide `verifies` grounding to strengthen a sibling proof
  change validation command to suppress a real failure
  loom edge exercises on a compiler-owned Journey validation

evidence required:
  command stdout/stderr, test count, failure message, or blocking reason
```

### Journey-root delivery loop

Journey work is ordered by semantic authority, not by convenience:

```text
human authors loom.journey/v1 meaning
  → loom journey add <artifact>
builder inspects loom journey derive <journey>
  → proposes a strict loom.journey-derivation/v1 manifest
  → reconciles explicit create/reuse Intent entries and requires/hierarchy relationships
  → presents one conversational hash-table batch for that exact hash-bound mapping
  → waits
human answers
  → builder records the exact answer with derive-accept; Loom stores an adopted Proposal
builder realizes and grounds every accepted technical Intent
  → inspects loom journey surface <journey>
  → writes the real target-repository CLI source
  → accepts the complete structured surface manifest (optional operation.exercises for downstream entries)
validator compiles the selected profile
  → compiler owns Proves / Validates / Calls / Exercises
  → validator runs it and records only the observed outcome
```

Authored Journey steps contain actors, actions, and expected outcomes. Implementation details belong to the derivation and surface projections. A semantic hash change invalidates those projections and returns the work to derive; it never invites an agent to reinterpret an old acceptance.

A compiled Journey reaches S3 only through its own accepted surface, compiled Exercises closure, and realizing code path. Downstream process/protocol handlers must be declared as operation `exercises` bound to a passed `observed_by` assertion — Loom does not infer handlers from matching names.

`derive-accept` is a human gate. The strict derivation manifest contains `proposal_id`, `proposal_rationale`, `intents[]`, `relationships[]`, and `unresolved_question`; every Intent entry declares `operation: create|reuse`, its stable entry `id`, `step_ids`, `level`, `visibility`, and `rationale`. A `create` entry additionally supplies `name` and a falsifiable `criterion`; a `reuse` entry supplies `intent_id` instead. Every relationship declares `id`, `kind` (`requires|hierarchy`), `from`, `to`, and `rationale`, with endpoints referring to included entry IDs. Loom rejects duplicate entries/relationships, relationship cycles, unresolved questions, stale hashes, and an adopted `proposal_id` paired with different content. An identical accepted replay is an idempotent no-op. The human reviews this as one conversational hash-table batch—proposal ID, Journey hash, manifest hash, entries, criteria, rationales, step IDs, and relationships—and authorizes that exact table, never an LLM summary.

`surface-accept` is not a product decision, but it requires cited live source and complete step bindings. `compile` and `run` belong to Validate. `diagnose` may override typed inputs for investigation but does not settle the proof. A compiled Journey reaches S3 only through its own accepted surface, compiled Exercises closure (including declared operation exercises with passed `observed_by` assertions), and realizing code path. Those `observed_by` assertions are trusted only when the local compiler-owned Journey runtime (compiler version 5) observed them against the canonical accepted-surface proof; imported graphs, deserialized run records, and caller-authored compiled proofs are audit-only until a local rerun of that exact compile. Compiler-v4 proofs must be recompiled and rerun. Schema v12 graphs do not need rebuilding.

### Host-mediated resume and release rehearsal

When `journey run` returns a pending human gate, the LLM presents the prompt and
stable options without choosing. After the human answers, it may relay that
exact answer with `journey resume <token> --choice <id>
--human-decision '<answer>'` (and `--free-form` only for an option that requests
revision). The token is one-shot and candidate-bound: do not move it between
graphs, rebuild or edit the subject and then resume, or retry it after claim.
Loom checks graph root, current semantic/compiled projection, gate binding, and
current subject before continuing. Human authority belongs to the answer; the
LLM is only its attributed executor.

Release preparation adds a second authority seam:

```text
derive each current Journey mapping
  → human reviews the exact canonical manifest batch
  → place only those reviewed manifests in the review-manifests directory
  → loom release authorize-derivations --manifest-dir <dir>
       --human-decision '<exact answer>' --json
  → execute the returned next_command exactly once
  → the outer release-workflow runs the requested detached rehearsal phases
  → inspect the structured candidate/result/fixpoint/effect attestations
```

Do not manufacture, persist in project files, edit, split, or replay the
`LOOM_RELEASE_DERIVATION_AUTHORITY` token. Authorization is read-only with
respect to the graph but creates a sealed temporary one-shot capsule. The
outer run atomically claims it and receives only candidate permits bound to the
approved batch and outer proof. Candidate runs may reauthorize the copied
reviewed manifests; they may not broaden the batch or acquire new human
authority. `release rehearse` is readiness evidence only: even
`gated-preparation` stops before release, install, commit, push, or other caller
mutation.

The operator must preserve the inventory boundary rather than “helpfully”
detecting language ecosystems. For Loom 0.30, candidates run only the exact
ordered Cargo gates and use only the declared `CARGO_HOME` and `RUSTUP_HOME`
cache roots documented in [`commands.md`](commands.md). Confirm structured
`status`, caller effects, cache before/after attestation, candidate hash, and
semantic result hash. Fixpoint equality compares deterministic Journey
summaries and semantic inputs, not target-directory or cache bytes.

### Semantic local checkpoints

After one Intent or cohesive accepted bundle is implemented, relevant tests
pass, sync and doctor are clean, and the portable export reflects the graph,
ask Loom for the exact evidence-bearing checkpoint scope:

```text
loom checkpoint recommend --intent <intent> [--intent <intent> ...] --json
```

Loom only recommends. It never stages, commits, or pushes, and a Git commit is
repository-history evidence rather than Loom truth. A ready response includes
the exact included paths, excluded dirty paths and reasons, checks and commands,
scope rationale, and suggested message. There is no “N changes” threshold.

The acting LLM then decides autonomously whether the local commit improves
historical tracing, reviewability, or regression bisecting:

```text
ready and exact:
  git add -- <only recommendation.included_paths>
  verify `git diff --cached --name-only -z` equals that set exactly
  git commit -m '<suggested_message>'
  leave the commit local

blocked, ambiguous ownership, user-owned overlap, or cached-set drift:
  defer; do not guess, widen the stage, or use `git add -A`
```

Creating or deferring the local commit does not interrupt the human. Publication
is the separate authority boundary. If push would be useful, present one table
containing the canonical repository, remote name and URL, full branch ref, and
full local commit OID, with Push / Keep local choices and a recommendation.
Only an explicit answer authorizes that exact tuple. Re-resolve it immediately
before push; a changed repository, remote, branch, or commit invalidates the
answer and requires a new decision. Silence or refusal keeps the checkpoint
local. Never treat a previous answer as blanket approval and never force-push.

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

### rectify

Clears needless ratify friction without deciding wantedness. INV-8 stays human: this role may demote visibility, relate scenarios, retire false duplicates, or escalate a real product call — it may never invent a yes or no.

```text
mindset:
  Clear NEEDLESS ratify friction.
  Structural fixes only — false duplicates, mis-marked visibility, missing scenario_of/relates.
  If the behavior is a real user-visible product call an LLM cannot honestly decide, escalate.
  Never invent a yes or no on wantedness.

allowed:
  loom next --mode rectify
  loom intent update <intent> --visibility internal --reason '…'
  loom intent update <intent> --rectify escalated|clear --reason '…'
  loom edge relate scenario-of / relates
  loom intent retire --replaced-by <keeper>
  loom intent update --description '…' --reword (same meaning, clearer words)

forbidden:
  loom intent ratify / reject (product decision — escalate instead)
  supplying --human-decision or treating obviousness as ratification
  editing production code to silence a divergence
  loom edge implement (does not ground new behavior)

evidence required:
  file:line or graph structure showing why the friction was false,
  or a concrete reason the human must decide
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

`loom next --mode elaborate` serves the most-incomplete user-visible feature intent. The packet embeds the Definition-of-Complete scorecard, including the open axes: `scenarios`, `prerequisites`, `boundary`, `proof`, `journey`, and `questions`. Journey ancestry is satisfied by a current accepted `derives` path from an authored root, or by a canonical human-approved Journey exemption; an ordinary waiver cannot fabricate either.

The loop is deliberately cognitive-cognitive:

```text
LLM first tells the user, in plain language, that a partial idea is enough
  → reflects what is already clear and explains it can fill technical/inferable gaps
  → does not assume the user knows Loom, scorecards, axes, or graph commands
LLM proposes missing surroundings
  → add scenario intents with --aspect sad|fallback|edge_case and scenario-of edges
  → add prerequisite edges or route missing Journey ancestry through authored roots and derivation
  → for a true product decision, record and directly ask ONE plain-language question:
       loom question add "<one crisp product question>" --intent <intent>
  → offer a recommended default and consequences when useful, then WAIT
human answers in the conversation
  → record the answer: loom question answer <question> --answer "<human answer>"
LLM continues elaboration
  → waive non-question axes only with a real reason:
       loom intent waive <intent> <axis> --reason "<why it deliberately does not apply>"

if no human is present, questions remain batched
  → surfaced by loom session and graph_state.open_questions
  → never infer an answer from silence; stop and resume when a human can answer
```

The LLM must not answer product questions for the human or offload safely inferable implementation choices onto them. It either creates the missing graph artifact, records a non-question waiver with a reason, or raises one crisp linked question, asks it directly in ordinary product language, and waits.

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

Wiki work is not a gated WorkItem lane: `loom wiki next` emits a verified brief — the documented intents' descriptions, groundings, and proof status — and the agent writes reader-first prose at the page's path, then stamps it fresh with `loom wiki record <title>`. The mindset: explain the graph, never contradict it; a claim the brief cannot support is documentation drift — capture it with `loom finding add ... --source wiki` (or `--source code_audit` when found during code review) instead of writing it.

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

The same contract runs in reverse at serve time (uniform adjudicability): every served packet's `write_back` names the exact runnable command(s) that close it, and that command accepts the packet's own target id (short-id prefix, name, or edge endpoints count — the commands resolve them). `fix` and `audit` packets close through state re-reads (`loom sync`, `loom audit --json`), so their closeout takes no target argument. A packet whose closure cannot be named is never served: the default walk skips it, `--mode` refuses with the defect named, and it is journaled as `unservable_packet`. You will not be handed work you cannot close; if a queue seems to skip an item `loom status` still counts, grep the journal for `unservable_packet`.

---

## Out-of-scope finding capture

While working a `WorkItem`, the LLM may encounter something outside the current task.

**Do not silently fix it. Do not silently dismiss it. Capture evidence-backed observations with `loom finding add`.**

```text
loom finding add '<claim>' --source code_audit --kind code_audit --file <registered-codefile> --evidence '<file:line — observed fact>' --impact '<why it matters>' --confidence <0.0-1.0>
```

Non-blocking means do not detour into editing; it does not mean drop the observation. Return to the current WorkItem after capture; triage adjudicates it later.

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
after capturing Finding: return to current WorkItem
after opening Question: ask/batch human answer, then `loom question answer` or `loom question close`; after routing InboxItem: mark routed/rejected/duplicate/deferred, then loom status
after interview turn: wait for human response or run loom status
```

Escalation rules:

```text
confidence < 0.7 after inspection:
  record verdict with actual confidence; review queue routes it

out-of-scope product decision:
  open a Question with `loom question add`; do not decide; return to WorkItem

missing prerequisite for human gate:
  record blocked with reason; return to loom status
```

---

## Human gate patterns

Some queues require a human decision. loom distinguishes autonomous and human-gated queues without requiring the human to operate the CLI.

```text
autonomous until they raise a human question (LLM drains alone):
  build, elaborate, fix, validate, quality, analyze, prove, triage, review

human-gated (requires human):
  align/product meaning re-confirmation
  blocked proofs (external prerequisite or credential)
  hypothesis adoption/rejection rulings
  major product decisions captured through InboxItem routing
```

A ratification packet carries a structured `human_gate` with three options
(Keep, Remove, Revise), recommendation guidance, and exact write-back commands.
The LLM summarizes the evidence, recommends one option with consequences, asks
through the host's ask-user interaction, and waits. When the human selects Keep
or Remove, the LLM executes the corresponding command with
`--human-decision '<exact human answer>'`. Loom records the authority as human
and the journal actor as the executing lane. If no answer arrives, nothing is
written. Direct terminal confirmation remains available when no mediated answer
is supplied.

```json
{
  "human_gate": {
    "question": "Should 'users can export reports' remain a wanted behavior?",
    "options": [
      { "id": "ratify", "label": "Keep behavior", "description": "…", "write_back": "loom intent ratify … --human-decision '<exact human answer>'" },
      { "id": "reject", "label": "Remove behavior", "description": "…", "write_back": "loom intent reject … --human-decision '<exact human answer>'" },
      { "id": "revise", "label": "Revise criterion", "description": "…", "write_back": "loom intent update …" }
    ],
    "recommendation": "The presenting LLM must recommend one option from the packet evidence…",
    "after_answer": "Wait; record the exact human answer, or write nothing."
  }
}
```

`loom session`, `loom question list --status open`, and `graph_state.open_questions` surface the human-gated remainder so the LLM can batch questions for one conversation window instead of interrupting repeatedly.

### The judgment inbox (staged proposals)

Candidates the LLM discovers OUTSIDE a served ratify packet — a junk intent found during triage, an intent whose statement drifted from the code — should not wait in the LLM's memory or squat in a work queue. Stage them:

```text
loom judgment propose reject <intent> --evidence "<why unwanted>"            # candidate removal
loom judgment propose ratify <intent> --evidence "<why wanted>"              # candidate ratification
loom judgment propose redefine <intent> --evidence "<why>" --description "<replacement statement>"
```

Staging is ungated (recommending is not deciding) and deduplicated per (kind, intent). The human reviews `loom judgment digest` and the LLM executes each `loom judgment confirm <id> --human-decision '<exact human answer>'` — the SAME mediated gate as the direct commands: if no human answer arrives, confirm refuses (INV-8) and the proposal stays staged. Withdraw wrong candidates with `loom judgment withdraw <id> --reason …`. `loom status` shows the staged count.

---

## Effort tiers and routing_hint

Every WorkItem carries `effort: low | mid | high`. This is a statement about the work, computed from graph structure and sync grading.

```text
low:   mechanical grounding, evidence-only re-verification (cheap re-confirm), simple proof re-run
mid:   relationship re-inspection, quality measurement, structural finding cohesion triage, inbox triage
high:  design reasoning, hypothesis proof, complex repair, intent alignment
```

`routing_hint` is a separate axis for harness routing:

```text
mechanical: cheap re-confirm (cited evidence intact) or a fully prefilled write_back with prior criterion
judgment:   full re-inspection, rewritten evidence, structural size/complexity cohesion, or any packet that still needs fresh reading
```

Structural detector findings (`oversized_file`, complexity/nesting/args, …) are **judgment** triage: the detector only crossed a calibrated gate; the LLM must name one cohesive concern (`justified`) or a split plan (`needed`). Owner-count in the reason is a hint, not a verdict. Do not batch-reaffirm these as mechanical residue.

Orchestrator contract for mechanical residue (no dedicated reconfirm command):

```text
loom next --mode analyze --all --json
  → select items where routing_hint == mechanical (or cause_class == cheap)
  → loom apply batch.json   # verdicts[] reaffirming prior criterion with fresh evidence
  → loom next               # judgment items one-at-a-time
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
  → graph_state, validation summary, code ownership, queues, and debt pulse (statistical clusters carry stable `cluster_id`; `loom debt promote` is the only write path from the feed and mints an asserted Finding for triage)

loom next
  → single highest-priority WorkItem with full PromptContract
```
