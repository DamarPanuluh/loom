---
type: reference
title: "Quality bars"
tags:
  - architecture
  - correctness
  - performance
  - resource_safety
  - security
---

## Quality bars

The norms loom holds the code to, by category.

### architecture

- **data-lineage-traceable** (warning) — Data: derived datasets name their sources — a consumer can trace a value back to its origin and know when it was computed.
- **green-is-earned** (error) — trust norm: no verdict-bearing edge may default to passing; compliance/proof states start uninspected and are earned by an inspecting agent
- **iso5055-main-no-dead-or-duplicate-code** (warning) — ISO 5055 Maintainability (CWE-561/1041): no unreachable or unused code; no copy-pasted logic where one definition should exist.
- **iso5055-main-single-responsibility** (warning) — ISO 5055 Maintainability (CWE-1080/1120): each unit (file, function, intent) owns one coherent responsibility; oversized or multi-concern units are split.
- **mobile-lifecycle-safe-state** (error) — Mobile: user-visible state survives backgrounding and process death — nothing critical lives only in memory across a lifecycle boundary.
- **mobile-offline-behavior-defined** (error) — Mobile: every network-dependent feature defines its offline behavior — cached, queued, or an explicit user-facing error. Never an indefinite spinner or a crash.
- **mobile-permission-in-context** (error) — Mobile: each platform permission is requested in the context of the feature that needs it, and denial leaves the app functional (degraded, not broken).
- **mobile-platform-divergence-explicit** (warning) — Mobile: platform-specific behavior (iOS vs Android, OS-version gates) is isolated and named, not scattered through feature logic as inline conditionals.
- **mobile-touch-target-size** (warning) — Mobile (HIG ~44pt / Material ~48dp): tap targets meet the platform minimum size and have adequate spacing, so controls are reliably hittable without mis-taps.
- **service-compatible-evolution** (error) — Service: contract changes are additive or versioned — removing/renaming fields or changing semantics requires a version consumers can pin; old versions get a deprecation path.
- **service-contract-artifact** (error) — Service: every exposed interface has a committed, versioned contract artifact (schema/IDL/OpenAPI) that consumers can ground against — the seam's single shared truth.
- **service-observable-failures** (warning) — Service: failures are logged/metric'd with enough context (ids, cause, upstream) to diagnose without reproducing.
- **storage-backend-boundary** (error) — Persistence backends must be isolated behind typed storage/repository operations; command handlers and domain workflows must not depend on backend query language, concrete connection/session types, or backend value/result types.
- **webui-accessible-interactive** (error) — Web UI (WCAG): interactive elements are keyboard-reachable and carry accessible names — real buttons/links, not bare clickable divs; focus is managed on dialogs/route changes.
- **webui-color-contrast** (warning) — Web UI (WCAG 1.4.3 / 1.4.11): text and meaningful UI meet the contrast ratio against their background (≥4.5:1 body text, ≥3:1 large text and UI/graphical components), and color is never the sole carrier of meaning.
- **webui-responsive-declared** (warning) — Web UI: layouts define behavior at small and large viewports — breakpoints are deliberate, content never becomes unreachable.
- **webui-touch-target-size** (warning) — Web UI (WCAG 2.5.5/2.5.8): interactive targets are large enough and spaced to hit reliably on touch and with imprecise pointers (~24px minimum, ~44px for primary actions), not tiny adjacent hit areas.

### correctness

- **conc-atomic-multi-step** (error) — Concurrency (CWE-362/367): multi-step state transitions (check-then-act, read-modify-write, exists-then-create) are atomic — one lock/transaction span — or explicitly designed to tolerate interleaving.
- **conc-deadlock-ordering** (error) — Concurrency (CWE-833): when more than one lock can be held at once, acquisition follows a single documented global order.
- **conc-no-lock-across-io** (error) — Concurrency (CWE-667): no lock is held across I/O, network calls, or await points — contention windows stay bounded by computation, not by external latency.
- **conc-sync-discipline** (error) — Concurrency (CWE-362/366): every piece of shared mutable state names its synchronization discipline — a lock, a single-writer thread/actor, atomics, or message passing. No ad-hoc unsynchronized access.
- **data-idempotent-reruns** (warning) — Data: pipeline stages re-run without duplicating or corrupting output — upsert/partition-overwrite semantics, not blind append.
- **data-migration-reversible** (error) — Data: schema migrations are ordered and repeatable, with a tested rollback — or an explicitly documented point of no return.
- **data-no-silent-loss** (error) — Data: pipelines account for every record — rejects go to a dead-letter/quarantine with a cause, never dropped silently; counts in vs out reconcile.
- **data-validated-at-ingest** (error) — Data (CWE-20): data entering storage is validated at the boundary, and invariants live in the schema (constraints, types, NOT NULL) — not only in application code.
- **docker-build-proven** (error) — Docker: each containerization intent carries a passing Validation that builds the image and exercises its entrypoint (`docker build` plus `docker run … --help` or an equivalent smoke command). A Dockerfile without a graph proof is only packaging text, not proven packaging behavior.
- **endpoint-matched-edges** (error) — ISO 5055 reliability: no GQL may match or filter a relationship by its own property (grafeo 0.5.x returns nondeterministic results); edges are keyed by endpoint nodes or scanned and filtered in Rust
- **migration-parity-before-cutover** (error) — A storage backend migration cannot replace the global/control backend until structured read parity, mutation parity, deterministic export parity, and rollback/cutover smoke checks pass on scratch graphs built from the same loom.graph.json.
- **service-compensation-defined** (warning) — Service (sagas): multi-step workflows define compensation or abort for partial failure — no half-completed state without a recovery path an operator or the code can take.
- **service-idempotent-handlers** (error) — Service: handlers for retriable inputs — webhooks, queue messages, payments — are idempotent; replaying the same message yields no duplicate effect.
- **webui-feedback-on-action** (warning) — Web UI: user actions give immediate feedback — pending/disabled/optimistic states; no silent in-flight gaps or double-submit windows.
- **webui-url-state-recoverable** (warning) — Web UI: state needed to recreate a view travels in the URL — refresh, back, and shared links land where the user expects.
- **webui-view-states-complete** (error) — Web UI: every data-driven view defines loading, empty, and error states — not just the populated happy state.

### performance

- **docker-cache-friendly-layers** (warning) — Docker: dependency manifests are copied and installed before frequently-changing source code, and package caches use BuildKit cache mounts where useful, so ordinary source edits reuse dependency layers.
- **docker-context-pruned** (warning) — Docker: the build context is pruned with `.dockerignore` so VCS data, local build outputs, dependencies, secrets, tests/docs not needed at runtime, and large artifacts do not enter the build context.
- **docker-multistage-minimal-runtime** (warning) — Docker: production images use a multi-stage build or an equivalently minimal runtime image (scratch/distroless/slim/alpine as appropriate), so build tools, caches, and source-only artifacts do not ship in the runtime layer.
- **iso5055-perf-bounded-work** (warning) — ISO 5055 Performance Efficiency (CWE-834/1050): no unbounded loops/recursion over external-sized data; iteration and queries are bounded, paginated, or capped.
- **iso5055-perf-no-redundant-work** (warning) — ISO 5055 Performance Efficiency (CWE-1042/1046): no repeated identical I/O, queries, or allocation in hot paths — cache or hoist invariant work out of loops.
- **mobile-main-thread-clear** (error) — Mobile: no blocking I/O, parsing, or heavy compute on the UI thread — frame budget is ~16ms.
- **perf-budget-proven** (error) — Measured performance: hot-path intents declare a performance budget in their criterion (e.g. 'p99 < 50ms at 10k entries') AND carry a benchmark validation proving it — fast is a claim, proven-fast is a state.

### resource_safety

- **conc-bounded-concurrency** (warning) — Concurrency (CWE-400/770): spawns, queues, and in-flight work have explicit limits and backpressure — load sheds or blocks, it never grows unbounded.
- **conc-cancellation-safe** (warning) — Concurrency: tasks/threads are cancellation-safe — interruption (timeout, shutdown, dropped future) leaves no half-written state and releases resources.
- **docker-runtime-contract-declared** (warning) — Docker: the runtime contract is explicit — entrypoint/cmd, exposed port when relevant, healthcheck for long-running services, and resource expectations/limits in compose or deployment config.
- **iso5055-rel-boundary-validation** (error) — ISO 5055 Reliability (CWE-20): external input (CLI args, file content, env vars, network data) is validated before use; invalid input yields a typed error, never corruption or a crash.
- **iso5055-rel-no-unchecked-failure** (error) — ISO 5055 Reliability (CWE-252/248/391): every fallible operation's failure path is handled or explicitly propagated — no silently ignored return value, no exception/panic escaping a boundary uncaught.
- **iso5055-rel-resource-release** (error) — ISO 5055 Reliability (CWE-772/404): every acquired resource (file, lock, connection, handle) is released on ALL paths, including error paths.
- **mobile-battery-respect** (warning) — Mobile: no unbounded polling, wake locks, or sensor/location subscriptions without lifecycle-bound teardown.
- **service-graceful-degradation** (warning) — Service: a dependency outage degrades the service (fallback, partial answer, fast error) — it never cascades into hangs or crash loops.
- **service-timeout-retry-explicit** (error) — Service (CWE-1088): every outbound call carries an explicit timeout and a bounded retry policy with backoff — no infinite waits, no unbounded retry storms.

### security

- **data-pii-handled** (error) — Data (CWE-359): personal/sensitive fields are identified, and access, retention, and deletion paths exist — a deletion request can actually be fulfilled.
- **docker-no-secrets-in-image** (error) — Docker (CWE-798): secrets are never baked into image layers or build args that persist in history; credentials arrive at runtime through env/secret mounts or BuildKit secret mounts.
- **docker-non-root-runtime** (error) — Docker (CWE-250): production containers run as a non-root user and avoid privilege escalation by default; root is limited to build/install steps or explicitly justified development images.
- **iso5055-sec-least-surface** (error) — ISO 5055 Security (CWE-284/732): expose the minimum — no debug/admin paths reachable in production flows, no overly-permissive file modes or defaults.
- **iso5055-sec-no-hardcoded-secrets** (error) — ISO 5055 Security (CWE-798): no credentials, tokens, or keys in source or config committed to the repo; secrets come from the environment or a secret store.
- **iso5055-sec-no-injection** (error) — ISO 5055 Security (CWE-89/78/79): untrusted data is never concatenated into SQL/shell/HTML/query strings — parameterize, escape at the boundary, or reject.
- **mobile-external-entry-validated** (error) — Mobile (CWE-20/939): externally-triggered entry points — deep links, intents/universal links, push payloads — validate their input before navigation or action.
- **sec-dependency-squatting** (error) — Security (AI hallucination): every external dependency resolves to a real, published package — AI models suggest non-existent packages (5-21% hallucination rate) that attackers can register as malware vectors (slopsquatting).
- **sec-minimal-response** (warning) — Security (CWE-200/359): API responses return only the fields the consumer needs, not full internal objects — no sensitive fields (password hashes, internal IDs, PII) leak through serialization.
- **sec-rate-limiting** (error) — Security (CWE-307/770): mutating and authentication endpoints carry explicit rate limits — brute-force, credential stuffing, and resource exhaustion are bounded, not unbounded.
- **sec-upload-validated** (error) — Security (CWE-434/79): file uploads validate type (MIME + content sniff), size, and filename — uploaded files are stored outside the webroot and never executed as code.
- **service-auth-at-boundary** (error) — Service (CWE-306/862): every externally reachable endpoint authenticates and authorizes before side effects — including 'internal' endpoints reachable from outside the trust zone.
- **webui-no-client-side-trust** (error) — Web UI (CWE-602): no secrets in the client bundle, and no authorization decision enforced only in the client — the server re-checks everything the UI hides.
- **webui-no-unescaped-render** (error) — Web UI (CWE-79): user-controlled content never reaches innerHTML / dangerouslySetInnerHTML / raw template interpolation without sanitization.


<!-- loom:prose-start -->








<!-- loom:prose-end -->
