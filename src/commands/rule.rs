use anyhow::Result;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cli::RuleCmd;
use crate::commands::resolve::resolve_intent_with_db;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::gate;
use crate::output::{fmt_rule_row, Printer};
use crate::types::QualityRule;

/// The hardcoded-secrets rule id — single source of truth, so the pack
/// definition and the effort lookup can't drift on the spelling (a rename in
/// one place silently dropping the rule to the "mid" default).
const ISO5055_SEC_NO_HARDCODED_SECRETS: &str = "iso5055-sec-no-hardcoded-secrets";
const ISO5055_MAIN_NO_DEAD_OR_DUPLICATE: &str = "iso5055-main-no-dead-or-duplicate-code";
const MOBILE_LIFECYCLE_SAFE_STATE: &str = "mobile-lifecycle-safe-state";

/// The ISO 5055 measuring sticks: (name, severity, description, detection_logic).
/// Two-to-three CWE-grounded rules per quality characteristic, written so an
/// LLM holding one against an intent's code knows exactly what to look for.
/// They are sticks, not detectors — verdicts still come from inspection.
const ISO5055_PACK: &[PackRule] = &[
    // Reliability → resource_safety
    PackRule::new("iso5055-rel-no-unchecked-failure", "error",
     "ISO 5055 Reliability (CWE-252/248/391): every fallible operation's failure path is handled or explicitly propagated — no silently ignored return value, no exception/panic escaping a boundary uncaught.",
     "Inspect the intent's error paths: ignored Results/return codes, unwrap/expect on external input, bare catch-alls, missing error branches at I/O, parse, lock, and network boundaries.",
     "resource_safety"),
    PackRule::new("iso5055-rel-resource-release", "error",
     "ISO 5055 Reliability (CWE-772/404): every acquired resource (file, lock, connection, handle) is released on ALL paths, including error paths.",
     "Look for acquisitions without RAII/defer/finally protection, locks held across I/O or awaits, and early returns that skip cleanup.",
     "resource_safety"),
    PackRule::new("iso5055-rel-boundary-validation", "error",
     "ISO 5055 Reliability (CWE-20): external input (CLI args, file content, env vars, network data) is validated before use; invalid input yields a typed error, never corruption or a crash.",
     "Trace each external input to its first use: is there a validation/parse step with an error path before the value reaches logic or storage?",
     "resource_safety"),
    // Security
    PackRule::with_evidence("iso5055-sec-no-injection", "error",
     "ISO 5055 Security (CWE-89/78/79): untrusted data is never concatenated into SQL/shell/HTML/query strings — parameterize, escape at the boundary, or reject.",
     "Trace untrusted inputs to every interpreter sink (exec/system calls, query strings, format/eval, HTML output) and check the escaping/parameterization at each.",
     "security",
     "{\"pass\":\"SQL queries use parameterized binds at src/db/queries.rs:45 — no string concatenation of user input\",\"independent\":\"no interpreter sinks — no SQL, shell, or HTML generation\",\"common_false_positive\":\"escaping exists in one path but not all — check every sink, not just the obvious one\"}",
     "[[\"parameterized\",\"prepared\",\"bind\",\"escape\",\"sanitize\",\"format\",\"template\"]]"
    ),
    PackRule::new(ISO5055_SEC_NO_HARDCODED_SECRETS, "error",
     "ISO 5055 Security (CWE-798): no credentials, tokens, or keys in source or config committed to the repo; secrets come from the environment or a secret store.",
     "Scan the intent's files for key-like literals, connection strings with passwords, and tokens; check how the code obtains credentials.",
     "security"),
    PackRule::new("iso5055-sec-least-surface", "error",
     "ISO 5055 Security (CWE-284/732): expose the minimum — no debug/admin paths reachable in production flows, no overly-permissive file modes or defaults.",
     "Enumerate what the intent exposes (endpoints, files written, flags) and check each against who actually needs it.",
     "security"),
    // Performance efficiency
    PackRule::new("iso5055-perf-bounded-work", "warning",
     "ISO 5055 Performance Efficiency (CWE-834/1050): no unbounded loops/recursion over external-sized data; iteration and queries are bounded, paginated, or capped.",
     "Look for loops over unbounded collections nested in loops (N+1 patterns), recursion without a depth guard, and full scans where a limit exists.",
     "performance"),
    PackRule::new("iso5055-perf-no-redundant-work", "warning",
     "ISO 5055 Performance Efficiency (CWE-1042/1046): no repeated identical I/O, queries, or allocation in hot paths — cache or hoist invariant work out of loops.",
     "Find work inside loops that is invariant across iterations (reads, compiles, allocations) and repeated identical calls that could be batched.",
     "performance"),
    // Maintainability → architecture
    PackRule::new("iso5055-main-single-responsibility", "warning",
     "ISO 5055 Maintainability (CWE-1080/1120): each unit (file, function, intent) owns one coherent responsibility; oversized or multi-concern units are split.",
     "Check unit sizes and concern count; cross-check `loom smells` (tangled_file / scattered_intent) for the same intent.",
     "architecture"),
    PackRule::new(ISO5055_MAIN_NO_DEAD_OR_DUPLICATE, "warning",
     "ISO 5055 Maintainability (CWE-561/1041): no unreachable or unused code; no copy-pasted logic where one definition should exist.",
     "Look for unused functions/exports, commented-out blocks kept 'just in case', and near-identical logic in sibling files. Before deleting, resolve what each candidate SERVES via the intent graph (`loom explain <file>` / `loom codefile show <path>`): absence of an IMPLEMENTS grounding is a COVERAGE GAP, not proof of death — loom's map is known-incomplete; an ungrounded symbol whose file or importers are owned by a live intent should be GROUNDED, not removed. Delete only code grounded solely to a retired/superseded intent, or genuinely unreachable code that should never have existed. Cross-check `loom smells` (code_clone / string_contract_duplicate) for the same evidence.",
     "architecture"),
];

/// Mobile vantage point: lifecycle, offline, permissions, the main thread,
/// battery, platform divergence, externally-triggered entry points.
const MOBILE_PACK: &[PackRule] = &[
    PackRule::new(MOBILE_LIFECYCLE_SAFE_STATE, "error",
     "Mobile: user-visible state survives backgrounding and process death — nothing critical lives only in memory across a lifecycle boundary.",
     "Trace each screen's state to its save/restore path (saved-state handles, persisted stores). Look for in-flight work assumed to finish after the app is backgrounded without an OS-sanctioned mechanism.",
     "architecture"),
    PackRule::new("mobile-offline-behavior-defined", "error",
     "Mobile: every network-dependent feature defines its offline behavior — cached, queued, or an explicit user-facing error. Never an indefinite spinner or a crash.",
     "For each network call reachable from UI: what renders when the request can't start or times out? Look for fetches with no offline/error branch.",
     "architecture"),
    PackRule::new("mobile-permission-in-context", "error",
     "Mobile: each platform permission is requested in the context of the feature that needs it, and denial leaves the app functional (degraded, not broken).",
     "List the manifest/Info.plist permissions; trace each to the feature using it, where it's requested, and the denial path.",
     "architecture"),
    PackRule::new("mobile-main-thread-clear", "error",
     "Mobile: no blocking I/O, parsing, or heavy compute on the UI thread — frame budget is ~16ms.",
     "Look for synchronous file/DB/network access, large JSON decoding, or image work on the main thread/dispatcher.",
     "performance"),
    PackRule::new("mobile-battery-respect", "warning",
     "Mobile: no unbounded polling, wake locks, or sensor/location subscriptions without lifecycle-bound teardown.",
     "Find timers, location/sensor listeners, and sockets; check each is released when the screen/app stops.",
     "resource_safety"),
    PackRule::new("mobile-platform-divergence-explicit", "warning",
     "Mobile: platform-specific behavior (iOS vs Android, OS-version gates) is isolated and named, not scattered through feature logic as inline conditionals.",
     "Grep platform checks (Platform.OS, Build.VERSION, #available); flag feature files mixing both platforms' branches inline.",
     "architecture"),
    PackRule::new("mobile-external-entry-validated", "error",
     "Mobile (CWE-20/939): externally-triggered entry points — deep links, intents/universal links, push payloads — validate their input before navigation or action.",
     "Trace each deep-link/push handler: is the payload parsed and validated with a rejection path before it drives navigation, auth, or writes?",
     "security"),
    PackRule::new("mobile-touch-target-size", "warning",
     "Mobile (HIG ~44pt / Material ~48dp): tap targets meet the platform minimum size and have adequate spacing, so controls are reliably hittable without mis-taps.",
     "Check interactive controls' rendered size + padding against the platform minimum; flag dense rows, small icon buttons, and edge-crowded or closely-stacked tap targets.",
     "architecture"),
];

/// Web-UI vantage point: view states, accessibility, XSS, responsiveness,
/// feedback, client-side trust, URL-recoverable state.
const WEBUI_PACK: &[PackRule] = &[
    PackRule::new("webui-view-states-complete", "error",
     "Web UI: every data-driven view defines loading, empty, and error states — not just the populated happy state.",
     "For each component that renders fetched data: what shows while pending, when the result is empty, and when the request fails? A missing branch is a violation.",
     "correctness"),
    PackRule::new("webui-accessible-interactive", "error",
     "Web UI (WCAG): interactive elements are keyboard-reachable and carry accessible names — real buttons/links, not bare clickable divs; focus is managed on dialogs/route changes.",
     "Look for onClick on non-interactive elements, icon buttons without labels, custom widgets without key handlers, and focus traps/restores on modals.",
     "architecture"),
    PackRule::new("webui-no-unescaped-render", "error",
     "Web UI (CWE-79): user-controlled content never reaches innerHTML / dangerouslySetInnerHTML / raw template interpolation without sanitization.",
     "Trace user-originated strings to every raw-HTML sink; check the sanitizer (or its absence) at each.",
     "security"),
    PackRule::new("webui-no-client-side-trust", "error",
     "Web UI (CWE-602): no secrets in the client bundle, and no authorization decision enforced only in the client — the server re-checks everything the UI hides.",
     "Scan client code/env for key-like literals; for each hidden/disabled privileged control, verify the corresponding server endpoint enforces the same rule.",
     "security"),
    PackRule::new("webui-feedback-on-action", "warning",
     "Web UI: user actions give immediate feedback — pending/disabled/optimistic states; no silent in-flight gaps or double-submit windows.",
     "For each mutating action: what changes on screen between click and response? Look for submit buttons that stay active mid-flight.",
     "correctness"),
    PackRule::new("webui-responsive-declared", "warning",
     "Web UI: layouts define behavior at small and large viewports — breakpoints are deliberate, content never becomes unreachable.",
     "Check key views at narrow widths: fixed widths, overflow without scroll, controls pushed off-canvas with no alternative.",
     "architecture"),
    PackRule::new("webui-url-state-recoverable", "warning",
     "Web UI: state needed to recreate a view travels in the URL — refresh, back, and shared links land where the user expects.",
     "For each stateful view: refresh it. If the result differs from what was on screen (lost filters/selection/page), the state isn't URL-recoverable.",
     "correctness"),
    PackRule::new("webui-color-contrast", "warning",
     "Web UI (WCAG 1.4.3 / 1.4.11): text and meaningful UI meet the contrast ratio against their background (≥4.5:1 body text, ≥3:1 large text and UI/graphical components), and color is never the sole carrier of meaning.",
     "Check the design tokens / computed styles for text-on-background and state indicators (error/success/disabled) against the WCAG ratio; flag low-contrast pairs and any status conveyed by color alone with no icon/label/text backup.",
     "architecture"),
    PackRule::new("webui-touch-target-size", "warning",
     "Web UI (WCAG 2.5.5/2.5.8): interactive targets are large enough and spaced to hit reliably on touch and with imprecise pointers (~24px minimum, ~44px for primary actions), not tiny adjacent hit areas.",
     "Measure interactive elements' rendered hit area and inter-target spacing; flag icon-only controls, dense list affordances, and close-packed buttons below the target size with no larger alternative.",
     "architecture"),
];

/// Service/integration vantage point: contracts, idempotency, timeouts,
/// compensation (sagas), boundary auth, observability, degradation, compat.
const SERVICE_PACK: &[PackRule] = &[
    PackRule::with_evidence("service-contract-artifact", "error",
     "Service: every exposed interface has a committed, versioned contract artifact (schema/IDL/OpenAPI) that consumers can ground against — the seam's single shared truth.",
     "For each endpoint/event/queue the service exposes: where is the contract file, is it in the repo, and does the implementation actually match it?",
     "architecture",
     // evidence_examples:
     "{\"pass\":\"OpenAPI spec at openapi.yaml defines /v1/orders; handler matches schema\",\"independent\":\"no exposed interfaces — internal CLI/library only\",\"common_false_positive\":\"a Markdown doc describing the API is NOT a contract artifact — it's prose, not a consumer-groundable single shared truth\"}",
     // signal_expectations: groups where at least one keyword must appear in evidence
     "[[\"openapi\",\"swagger\",\"proto\",\"graphql\",\"idl\",\"avro\",\"json-schema\",\"asyncapi\"]]"
    ),
    PackRule::new("service-idempotent-handlers", "error",
     "Service: handlers for retriable inputs — webhooks, queue messages, payments — are idempotent; replaying the same message yields no duplicate effect.",
     "For each handler: what happens on exact redelivery? Look for inserts without dedup keys, counters without idempotency tokens, side effects before the dedup check.",
     "correctness"),
    PackRule::with_evidence("service-timeout-retry-explicit", "error",
     "Service (CWE-1088): every outbound call carries an explicit timeout and a bounded retry policy with backoff — no infinite waits, no unbounded retry storms.",
     "Find each HTTP/DB/queue client call: is a timeout set (not the library's infinite default)? Is retry bounded with backoff and jitter?",
     "resource_safety",
     "{\"pass\":\"reqwest client at src/client.rs:23 sets .timeout(Duration::from_secs(30)) and .retry(3) with backoff\",\"independent\":\"no outbound network calls — all operations are local\",\"common_false_positive\":\"a default timeout exists but is not explicitly set in code — relying on library defaults is not explicit\"}",
     "[[\"timeout\",\"retry\",\"backoff\",\"duration\",\"deadline\",\"context\"]]"
    ),
    PackRule::new("service-compensation-defined", "warning",
     "Service (sagas): multi-step workflows define compensation or abort for partial failure — no half-completed state without a recovery path an operator or the code can take.",
     "For each workflow spanning >1 service or transaction: enumerate the failure point after each step and name the compensating action. A missing one is the violation.",
     "correctness"),
    PackRule::with_evidence("service-auth-at-boundary", "error",
     "Service (CWE-306/862): every externally reachable endpoint authenticates and authorizes before side effects — including 'internal' endpoints reachable from outside the trust zone.",
     "Enumerate reachable routes; for each, find the auth check and confirm it runs before any write or privileged read.",
     "security",
     "{\"pass\":\"Auth middleware at src/auth.rs:45-60 extracts Bearer token before handler; route table confirms all mutating routes require it\",\"independent\":\"no externally reachable endpoints — CLI/library with no network surface\",\"common_false_positive\":\"auth exists in config but not enforced on the route — config is intent, not enforcement\"}",
     "[[\"auth\",\"bearer\",\"jwt\",\"session\",\"token\",\"middleware\",\"extractor\",\"apikey\",\"oauth\",\"permission\",\"rbac\"]]"
    ),
    PackRule::with_evidence("service-observable-failures", "warning",
     "Service: failures are logged/metric'd with enough context (ids, cause, upstream) to diagnose without reproducing.",
     "Pick the main failure paths: what exactly lands in logs/metrics? Catch-and-ignore blocks and bare 500s with no context are violations.",
     "architecture",
     "{\"pass\":\"error handler at src/error.rs:80-95 logs request_id, error type, and stack trace via tracing::error\",\"independent\":\"no failure paths that produce observable side effects — pure computation\",\"common_false_positive\":\"a JSON error response alone is NOT observable — it returns to the caller but is not logged/metric'd for operators\"}",
     "[[\"tracing\",\"log\",\"metrics\",\"telemetry\",\"request_id\",\"trace_id\",\"span\",\"sentry\",\"datadog\",\"prometheus\"]]"
    ),
    PackRule::new("service-graceful-degradation", "warning",
     "Service: a dependency outage degrades the service (fallback, partial answer, fast error) — it never cascades into hangs or crash loops.",
     "For each hard dependency: trace what happens when it's down. Look for unguarded startup dependencies and synchronous calls on the hot path with no circuit/fallback.",
     "resource_safety"),
    PackRule::with_evidence("service-compatible-evolution", "error",
     "Service: contract changes are additive or versioned — removing/renaming fields or changing semantics requires a version consumers can pin; old versions get a deprecation path.",
     "Diff the contract's history (or its change discipline): were fields ever removed/renamed in place? Is there a versioning convention at all?",
     "architecture",
     "{\"pass\":\"API versioned at /v1/ and /v2/ routes; breaking changes land in /v2/ with /v1/ deprecated but still served\",\"independent\":\"no versioned contract surface — internal API only\",\"common_false_positive\":\"versioned /v1 routes alone are a PARTIAL story — not the same as compatibility tests or schema-diff enforcement; consider partial status\"}",
     "[[\"version\",\"v1\",\"v2\",\"deprecat\",\"schema-diff\",\"compat\",\"breaking\"]]"
    ),
];

/// Data vantage point: migrations, ingest validation, loss accounting,
/// PII handling, rerun safety, lineage.
const DATA_PACK: &[PackRule] = &[
    PackRule::new("data-migration-reversible", "error",
     "Data: schema migrations are ordered and repeatable, with a tested rollback — or an explicitly documented point of no return.",
     "Check the migration set: do down-migrations exist and run? For irreversible ones, is the irreversibility stated where the operator will see it?",
     "correctness"),
    PackRule::new("data-validated-at-ingest", "error",
     "Data (CWE-20): data entering storage is validated at the boundary, and invariants live in the schema (constraints, types, NOT NULL) — not only in application code.",
     "Trace each write path to storage: what rejects bad data? Look for app-side-only checks the schema doesn't enforce, and ingestion that bypasses the validated path.",
     "correctness"),
    PackRule::new("data-no-silent-loss", "error",
     "Data: pipelines account for every record — rejects go to a dead-letter/quarantine with a cause, never dropped silently; counts in vs out reconcile.",
     "Find each filter/catch/skip in the pipeline: where do the excluded records go, and is the count surfaced anywhere a human looks?",
     "correctness"),
    PackRule::new("data-pii-handled", "error",
     "Data (CWE-359): personal/sensitive fields are identified, and access, retention, and deletion paths exist — a deletion request can actually be fulfilled.",
     "List fields holding personal data (and copies in logs/derived tables). For each: who can read it, how long it lives, and what a delete actually removes.",
     "security"),
    PackRule::new("data-idempotent-reruns", "warning",
     "Data: pipeline stages re-run without duplicating or corrupting output — upsert/partition-overwrite semantics, not blind append.",
     "For each stage: run it twice on the same input (mentally or actually). Appends without keys and non-deterministic transforms are violations.",
     "correctness"),
    PackRule::new("data-lineage-traceable", "warning",
     "Data: derived datasets name their sources — a consumer can trace a value back to its origin and know when it was computed.",
     "Pick a derived table/report: can you find what produced it, from what inputs, when? Untraceable derived data is the violation.",
     "architecture"),
];

/// Concurrency & measured-performance vantage point: synchronization
/// discipline, lock hygiene, atomicity, deadlock ordering, cancellation,
/// backpressure — plus the bridge rule that demands hot paths carry a
/// PROVEN budget (a benchmark validation), not a vibe.
const CONCURRENCY_PACK: &[PackRule] = &[
    PackRule::new("conc-sync-discipline", "error",
     "Concurrency (CWE-362/366): every piece of shared mutable state names its synchronization discipline — a lock, a single-writer thread/actor, atomics, or message passing. No ad-hoc unsynchronized access.",
     "Inventory state reachable from more than one thread/task; for each, name the discipline that guards it. State you cannot name a discipline for is the violation.",
     "correctness"),
    PackRule::new("conc-no-lock-across-io", "error",
     "Concurrency (CWE-667): no lock is held across I/O, network calls, or await points — contention windows stay bounded by computation, not by external latency.",
     "Find each lock acquisition; trace what runs before release. File/DB/network access or an .await/blocking call inside the critical section is the violation.",
     "correctness"),
    PackRule::new("conc-atomic-multi-step", "error",
     "Concurrency (CWE-362/367): multi-step state transitions (check-then-act, read-modify-write, exists-then-create) are atomic — one lock/transaction span — or explicitly designed to tolerate interleaving.",
     "Find check-then-act sequences on shared state (or storage): can another actor run between the steps? If yes and nothing tolerates that, it's a TOCTOU violation.",
     "correctness"),
    PackRule::new("conc-deadlock-ordering", "error",
     "Concurrency (CWE-833): when more than one lock can be held at once, acquisition follows a single documented global order.",
     "List sites holding ≥2 locks; check the acquisition order is consistent everywhere and written down. Two sites taking A→B and B→A is the violation.",
     "correctness"),
    PackRule::new("conc-cancellation-safe", "warning",
     "Concurrency: tasks/threads are cancellation-safe — interruption (timeout, shutdown, dropped future) leaves no half-written state and releases resources.",
     "For each spawned task: what happens if it's killed between its side effects? Look for multi-step writes without cleanup/transactions and resources freed only on the happy exit.",
     "resource_safety"),
    PackRule::new("conc-bounded-concurrency", "warning",
     "Concurrency (CWE-400/770): spawns, queues, and in-flight work have explicit limits and backpressure — load sheds or blocks, it never grows unbounded.",
     "Find each spawn/enqueue driven by external input; name its bound (pool size, channel capacity, semaphore). An unbounded channel or per-request spawn with no cap is the violation.",
     "resource_safety"),
    PackRule::new("perf-budget-proven", "error",
     "Measured performance: hot-path intents declare a performance budget in their criterion (e.g. 'p99 < 50ms at 10k entries') AND carry a benchmark validation proving it — fast is a claim, proven-fast is a state.",
     "Cross-check `loom hotspots`: for each high-centrality intent on a hot path, does its criterion state a number, and does a `benchmark`-type validation exist and pass? A budget without a benchmark (or vice versa) is the violation.",
     "performance"),
];

/// Container packaging vantage point: image size, build-cache shape, runtime
/// hardening, secret hygiene, and graph-native proof that the image builds and
/// starts. The Dockerfile/.dockerignore stay CodeFiles; these rules are the
/// normative plane that makes container creation measurable.
const DOCKER_APPLIES: &str = "{\"signals\":[{\"source\":\"intent_text\",\"terms\":[\"docker\",\"container\",\"image\",\"dockerfile\"],\"weight\":0.35,\"reason\":\"intent text mentions Docker/container/image packaging\"},{\"source\":\"path\",\"terms\":[\"dockerfile\",\".dockerignore\",\"docker-compose.yml\",\"docker-compose.yaml\",\"compose.yml\",\"compose.yaml\"],\"weight\":0.45,\"reason\":\"grounded files include Dockerfile/.dockerignore/compose artifacts\"}]}";
const DOCKER_BUILD_APPLIES: &str = "{\"signals\":[{\"source\":\"intent_text\",\"terms\":[\"docker\",\"container\",\"image\",\"dockerfile\"],\"weight\":0.35,\"reason\":\"intent text mentions Docker/container/image packaging\"},{\"source\":\"path\",\"terms\":[\"dockerfile\",\".dockerignore\",\"docker-compose.yml\",\"docker-compose.yaml\",\"compose.yml\",\"compose.yaml\"],\"weight\":0.45,\"reason\":\"grounded files include Dockerfile/.dockerignore/compose artifacts\"},{\"source\":\"missing_validation_all\",\"groups\":[[\"docker build\",\"podman build\"],[\"docker run\",\"podman run\"]],\"weight\":0.30,\"reason\":\"no linked validation command proves both image build and image run\"}]}";

const DOCKER_PACK: &[PackRule] = &[
    PackRule::with_evidence("docker-build-proven", "error",
     "Docker: each containerization intent carries a passing Validation that builds the image and exercises its entrypoint (`docker build` plus `docker run … --help` or an equivalent smoke command). A Dockerfile without a graph proof is only packaging text, not proven packaging behavior.",
     "For a containerization/deployment intent, inspect its linked validations: is there a passed command that builds the image and a passed command (or combined smoke) proving the produced image starts? Mark independent for intents unrelated to container packaging.",
     "correctness",
     "{\"pass\":\"Validation 'docker image smoke' runs `docker build -t app:local . && docker run --rm app:local --help` and last_result=passed\",\"independent\":\"this intent is pure CLI parsing and has no container packaging responsibility\",\"common_false_positive\":\"a Dockerfile exists but no passing build/run validation is linked to the packaging intent\"}",
     "[[\"docker build\",\"docker run\",\"podman build\",\"podman run\"],[\"passed\",\"last_result\"]]"
    ).with_applies_when(DOCKER_BUILD_APPLIES),
    PackRule::new("docker-multistage-minimal-runtime", "warning",
     "Docker: production images use a multi-stage build or an equivalently minimal runtime image (scratch/distroless/slim/alpine as appropriate), so build tools, caches, and source-only artifacts do not ship in the runtime layer.",
     "Inspect Dockerfile stages and final FROM: build dependencies should live in builder stages, with only the compiled app/runtime assets copied into the final image. Flag single-stage full distro/toolchain images unless this is explicitly a development container.",
     "performance").with_applies_when(DOCKER_APPLIES),
    PackRule::new("docker-cache-friendly-layers", "warning",
     "Docker: dependency manifests are copied and installed before frequently-changing source code, and package caches use BuildKit cache mounts where useful, so ordinary source edits reuse dependency layers.",
     "Read the Dockerfile layer order: dependency lockfiles/manifests should precede broad `COPY . .`; expensive package install/build steps should not be invalidated by unrelated source or docs churn.",
     "performance").with_applies_when(DOCKER_APPLIES),
    PackRule::new("docker-context-pruned", "warning",
     "Docker: the build context is pruned with `.dockerignore` so VCS data, local build outputs, dependencies, secrets, tests/docs not needed at runtime, and large artifacts do not enter the build context.",
     "Inspect `.dockerignore` (or equivalent build context controls) alongside the Dockerfile. Flag missing ignores for `.git`, target/dist/build outputs, dependency directories, local env files, and bulky non-runtime assets.",
     "performance").with_applies_when(DOCKER_APPLIES),
    PackRule::new("docker-non-root-runtime", "error",
     "Docker (CWE-250): production containers run as a non-root user and avoid privilege escalation by default; root is limited to build/install steps or explicitly justified development images.",
     "Inspect the final runtime stage for `USER` and ownership of copied files. Flag final images that default to root without a recorded reason.",
     "security").with_applies_when(DOCKER_APPLIES),
    PackRule::new("docker-no-secrets-in-image", "error",
     "Docker (CWE-798): secrets are never baked into image layers or build args that persist in history; credentials arrive at runtime through env/secret mounts or BuildKit secret mounts.",
     "Scan Dockerfile, compose files, and copied config for key-like literals, ARG/ENV secrets, private registry tokens, and credential files copied into the image. Check that secret inputs use runtime injection or BuildKit `--mount=type=secret`.",
     "security").with_applies_when(DOCKER_APPLIES),
    PackRule::new("docker-runtime-contract-declared", "warning",
     "Docker: the runtime contract is explicit — entrypoint/cmd, exposed port when relevant, healthcheck for long-running services, and resource expectations/limits in compose or deployment config.",
     "For service containers, inspect Dockerfile plus compose/deployment files: is startup explicit, is health observable, and are CPU/memory expectations documented or limited? Mark independent for one-shot CLI images where health/ports do not apply.",
     "resource_safety").with_applies_when(DOCKER_APPLIES),
];

/// AI-generated code security gaps: patterns AI models reproduce that the
/// baseline ISO 5055 security rules don't cover. Distilled from the
/// sec-context anti-pattern taxonomy (Arcanum, CC-BY 4.0) — the 4 novel
/// patterns not already in iso5055/web-ui/service packs.
const SECURITY_DEEP_PACK: &[PackRule] = &[
    PackRule::new("sec-dependency-squatting", "error",
     "Security (AI hallucination): every external dependency resolves to a real, published package — AI models suggest non-existent packages (5-21% hallucination rate) that attackers can register as malware vectors (slopsquatting).",
     "List every external dependency/import in the intent's code. For each: does it resolve to a real published package in the language's registry? A dependency that doesn't exist (typo-squat, hallucinated name, or withdrawn) is the violation.",
     "security"),
    PackRule::new("sec-rate-limiting", "error",
     "Security (CWE-307/770): mutating and authentication endpoints carry explicit rate limits — brute-force, credential stuffing, and resource exhaustion are bounded, not unbounded.",
     "For each endpoint that accepts external input and causes a side effect (login, signup, password reset, write, delete): is there a rate limit? A mutating/auth endpoint with no explicit limit is the violation.",
     "security"),
    PackRule::new("sec-minimal-response", "warning",
     "Security (CWE-200/359): API responses return only the fields the consumer needs, not full internal objects — no sensitive fields (password hashes, internal IDs, PII) leak through serialization.",
     "For each API response/serialization point: does it return a typed DTO/ projection or the raw internal model? Returning a full internal object that includes sensitive fields is the violation.",
     "security"),
    PackRule::new("sec-upload-validated", "error",
     "Security (CWE-434/79): file uploads validate type (MIME + content sniff), size, and filename — uploaded files are stored outside the webroot and never executed as code.",
     "For each file upload path: what validates the type, size, and filename? Where are uploads stored? An upload with no type/size validation or stored in a web-served directory is the violation.",
     "security"),
];

/// All seedable packs, by name. `iso5055` is the baseline (applies to any code);
/// the rest are repo-kind vantage points — `loom detect` recommends which fit.
/// One rule in a seedable pack. The 5-tuple carries the rule's identity and
/// detection guidance; the optional `evidence_examples` and `signal_expectations`
/// carry evidence-steering metadata (v12) — empty string means no examples/
/// expectations (the rule predates v12 or has no static signal pattern).
pub struct PackRule {
    pub name: &'static str,
    pub severity: &'static str,
    pub description: &'static str,
    pub detection: &'static str,
    pub kind: &'static str,
    pub evidence_examples: &'static str,
    pub signal_expectations: &'static str,
    pub applies_when: &'static str,
}

impl PackRule {
    /// Convenience for rules without evidence examples or signal expectations.
    const fn new(
        name: &'static str,
        severity: &'static str,
        description: &'static str,
        detection: &'static str,
        kind: &'static str,
    ) -> Self {
        Self {
            name,
            severity,
            description,
            detection,
            kind,
            evidence_examples: "",
            signal_expectations: "",
            applies_when: "",
        }
    }
    /// Rules with evidence steering metadata.
    const fn with_evidence(
        name: &'static str,
        severity: &'static str,
        description: &'static str,
        detection: &'static str,
        kind: &'static str,
        evidence_examples: &'static str,
        signal_expectations: &'static str,
    ) -> Self {
        Self {
            name,
            severity,
            description,
            detection,
            kind,
            evidence_examples,
            signal_expectations,
            applies_when: "",
        }
    }

    const fn with_applies_when(mut self, applies_when: &'static str) -> Self {
        self.applies_when = applies_when;
        self
    }
}

type Pack = (&'static str, &'static [PackRule]);
const PACKS: &[Pack] = &[
    ("iso5055", ISO5055_PACK),
    ("security-deep", SECURITY_DEEP_PACK),
    ("mobile", MOBILE_PACK),
    ("web-ui", WEBUI_PACK),
    ("service", SERVICE_PACK),
    ("data", DATA_PACK),
    ("concurrency", CONCURRENCY_PACK),
    ("docker", DOCKER_PACK),
];

/// Names of all seedable packs (for help/errors/`loom detect`).
pub fn pack_names() -> Vec<&'static str> {
    PACKS.iter().map(|(n, _)| *n).collect()
}

/// Inspection effort per pack rule — how much capability holding this rule
/// against code actually needs. Annotated where the pack author KNOWS it
/// statically: a secrets scan is near-mechanical (low); atomicity, deadlock
/// ordering, compensation, and lifecycle-survival demand deep semantic reading
/// (high); everything else is read-and-judge (mid, the default). This is a
/// statement about the WORK — the harness decides which model answers.
fn pack_rule_effort(name: &str) -> &'static str {
    match name {
        // Near-mechanical scans.
        ISO5055_SEC_NO_HARDCODED_SECRETS
        | ISO5055_MAIN_NO_DEAD_OR_DUPLICATE
        | "sec-dependency-squatting"
        | "docker-context-pruned"
        | "docker-non-root-runtime"
        | "docker-no-secrets-in-image" => "low",
        // Deep semantic reading.
        "conc-atomic-multi-step"
        | "conc-deadlock-ordering"
        | "conc-cancellation-safe"
        | "service-compensation-defined"
        | "service-idempotent-handlers"
        | MOBILE_LIFECYCLE_SAFE_STATE
        | "data-pii-handled"
        | "sec-rate-limiting"
        | "sec-minimal-response"
        | "sec-upload-validated" => "high",
        _ => "mid",
    }
}

pub fn run(cmd: RuleCmd, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    match cmd {
        RuleCmd::List { limit } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_list_with_db(&db, limit, printer)
        }
        RuleCmd::Show { identifier } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_show_with_db(&db, &identifier, printer)
        }
        RuleCmd::Check { intent_id } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_check_with_db(&db, intent_id, printer)
        }
        RuleCmd::Recommend {
            intent_id,
            all,
            limit,
        } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_recommend_with_db(&db, intent_id, all, limit, printer)
        }
        cmd => {
            ensure_initialized(&cwd)?;
            run_with_sqlite(&cwd, cmd, printer)
        }
    }
}

fn run_with_sqlite(root: &std::path::Path, cmd: RuleCmd, printer: &Printer) -> Result<()> {
    let mut store = crate::db::sqlite::SqliteGraphStore::open(&crate::db::sqlite_db_path(root))?;
    match cmd {
        RuleCmd::Add {
            name,
            description,
            severity,
            kind,
            effort,
            applies_when,
        } => {
            gate::acting_in_lane(&gate::lane::ADD_RULE, None)?;
            severity
                .parse::<crate::types::Severity>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            if let Some(e) = &effort {
                if !matches!(e.as_str(), "low" | "mid" | "high") {
                    anyhow::bail!("--effort must be low, mid, or high (a statement about the inspection WORK, not about models).");
                }
            }
            // Validate the norm category; its default effort applies when
            // --effort is omitted, so effort derives from kind+override.
            let governs_kind = match &kind {
                Some(k) => Some(
                    k.parse::<crate::types::GovernsKind>()
                        .map_err(|e| anyhow::anyhow!("{}", e))?,
                ),
                None => None,
            };
            let inspection_effort = effort
                .or_else(|| governs_kind.map(|gk| gk.default_effort().to_string()))
                .unwrap_or_default();
            let applies_when = normalize_applies_when(applies_when.as_deref())?;
            let id = Uuid::new_v4().to_string();
            let rule = QualityRule {
                id: id.clone(),
                name: name.clone(),
                description,
                detection_logic: String::new(),
                kind: kind.unwrap_or_default(),
                inspection_effort,
                severity,
                evidence_examples: String::new(),
                signal_expectations: String::new(),
                applies_when,
            };
            store.insert_rule(&rule)?;

            if printer.json {
                printer.print_json(&rule);
            } else {
                println!("✓ Rule '{}' created  (id: {})", name, id);
            }
        }

        RuleCmd::Seed { pack, update } => {
            gate::acting_in_lane(&gate::lane::SEED_RULES, None)?;
            let Some((_, rules)) = PACKS.iter().find(|(n, _)| *n == pack) else {
                anyhow::bail!(
                    "Unknown pack '{}'. Available: {} — `loom detect` recommends which fit this repo.",
                    pack,
                    pack_names().join(", ")
                );
            };
            let existing: std::collections::HashMap<String, QualityRule> = store
                .list_rules()?
                .into_iter()
                .map(|r| (r.name.clone(), r))
                .collect();
            let mut created: Vec<QualityRule> = Vec::new();
            let mut skipped = 0usize;
            let mut updated_count = 0usize;
            for rule_def in *rules {
                if let Some(existing_rule) = existing.get(rule_def.name) {
                    // Backfill recommendation/evidence metadata when --update
                    // is passed and the pack carries richer metadata.
                    if update
                        && (existing_rule.evidence_examples.is_empty()
                            != rule_def.evidence_examples.is_empty()
                            || (existing_rule.signal_expectations.is_empty()
                                || existing_rule.signal_expectations == "[]")
                                != rule_def.signal_expectations.is_empty()
                            || (existing_rule.applies_when.is_empty()
                                || existing_rule.applies_when == "{}")
                                != rule_def.applies_when.is_empty())
                    {
                        let mut u = existing_rule.clone();
                        if !rule_def.evidence_examples.is_empty() {
                            u.evidence_examples = rule_def.evidence_examples.to_string();
                        }
                        if !rule_def.signal_expectations.is_empty() {
                            u.signal_expectations = rule_def.signal_expectations.to_string();
                        }
                        if !rule_def.applies_when.is_empty() {
                            u.applies_when = rule_def.applies_when.to_string();
                        }
                        store.insert_rule(&u)?;
                        updated_count += 1;
                    } else {
                        skipped += 1;
                    }
                    continue;
                }
                let rule = QualityRule {
                    id: Uuid::new_v4().to_string(),
                    name: rule_def.name.to_string(),
                    description: rule_def.description.to_string(),
                    detection_logic: rule_def.detection.to_string(),
                    kind: rule_def.kind.to_string(),
                    inspection_effort: pack_rule_effort(rule_def.name).to_string(),
                    severity: rule_def.severity.to_string(),
                    evidence_examples: rule_def.evidence_examples.to_string(),
                    signal_expectations: if rule_def.signal_expectations.is_empty() {
                        "[]".to_string()
                    } else {
                        rule_def.signal_expectations.to_string()
                    },
                    applies_when: if rule_def.applies_when.is_empty() {
                        "{}".to_string()
                    } else {
                        rule_def.applies_when.to_string()
                    },
                };
                store.insert_rule(&rule)?;
                created.push(rule);
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "pack": pack,
                    "created": created, "skipped_existing": skipped,
                    "updated": updated_count,
                    "next": "loom next --mode quality now serves every coded intent these rules were never held against; one command resolves each — loom rule verdict.",
                }));
            } else {
                println!(
                    "✓ Seeded pack '{}': {} rule(s) created, {} already present, {} updated.",
                    pack,
                    created.len(),
                    skipped,
                    updated_count
                );
                for r in &created {
                    println!("  + [{}] {}", r.severity, r.name);
                }
                if updated_count > 0 {
                    println!("  ↻ {updated_count} rule(s) updated with v12 evidence examples/signal expectations.");
                }
                println!("  → `loom next --mode quality` now serves every coded intent these were never held against;");
                println!("    one command resolves each: `loom rule verdict` (independent = measured, doesn't apply;");
                println!("    a verdict at component altitude covers its descendants ONLY with --covers-descendants).");
            }
        }

        RuleCmd::Apply {
            rule_id,
            intent_id,
            criterion,
        } => {
            gate::acting_in_lane(&gate::lane::APPLY_RULE, None)?;
            let rule_id = store.resolve_rule(&rule_id)?;
            let intent_id = resolve_intent_with_db(&store, &intent_id)?;
            let now = chrono::Utc::now().to_rfc3339();
            let crit = criterion.as_deref().unwrap_or("");
            if !crit.is_empty() {
                gate::require_substantive("criterion", crit, gate::GOVERNS_CRITERION_PURPOSE)?;
            }
            store.insert_governs(&rule_id, &intent_id, crit, &now)?;
            let edge_id =
                crate::db::schema::edge_key(crate::db::schema::edge::GOVERNS, &rule_id, &intent_id);
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status":    "ok",
                    "edge_id":   edge_id,
                    "rule_id":   rule_id,
                    "intent_id": intent_id,
                    "message":   "GOVERNS edge created with inspection_status=uninspected. Inspect and update via `loom rule check`.",
                    "next_step": format!("Run `loom rule check {}` to inspect.", intent_id),
                }));
            } else {
                println!("{}", crate::output::governs_edge_created_line(&edge_id));
                println!("  rule   → {}", rule_id);
                println!("  intent → {}", intent_id);
                println!("  Run `loom rule check {}` to inspect.", intent_id);
            }
        }

        RuleCmd::Verdict {
            rule_id,
            intent_id,
            status,
            criterion,
            evidence,
            evidence_locator,
            confidence,
            inspected_by,
            covers_descendants,
        } => {
            let by = gate::acting_in_lane(&gate::lane::GOVERNS_VERDICT, inspected_by.as_deref())?;
            let rule_id = store.resolve_rule(&rule_id)?;
            let intent_id = resolve_intent_with_db(&store, &intent_id)?;
            if status != "passing"
                && status != "failing"
                && status != "independent"
                && status != "partial"
            {
                anyhow::bail!(
                    "Invalid --status '{}'. A verdict is passing, failing, independent, or partial.",
                    status
                );
            }
            gate::require_substantive(
                "criterion",
                &criterion,
                "what compliance looks like for this rule on this intent (falsifiable)",
            )?;
            gate::require_substantive(
                "evidence",
                &evidence,
                if status == "independent" {
                    gate::VERDICT_EVIDENCE_INDEPENDENT_PURPOSE
                } else {
                    gate::VERDICT_EVIDENCE_FAILING_PURPOSE
                },
            )?;
            gate::require_passing_locator(&status, &evidence_locator)?;
            gate::require_locators_resolve(root, &evidence_locator)?;
            if covers_descendants && evidence.trim().is_empty() {
                anyhow::bail!("--covers-descendants requires evidence justifying why the same criterion applies to every child");
            }

            let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
            let now = chrono::Utc::now().to_rfc3339();
            let mut found = store.update_governs_verdict(
                &rule_id,
                &intent_id,
                &status,
                &criterion,
                &evidence,
                confidence,
                &by,
                &now,
                covers_descendants,
            )?;
            let mut edge_created = false;
            if !found {
                store.insert_governs(&rule_id, &intent_id, &criterion, &now)?;
                found = store.update_governs_verdict(
                    &rule_id,
                    &intent_id,
                    &status,
                    &criterion,
                    &evidence,
                    confidence,
                    &by,
                    &now,
                    covers_descendants,
                )?;
                edge_created = true;
            }
            if !found {
                anyhow::bail!(
                    "Could not record the GOVERNS verdict between rule '{}' and intent '{}'.",
                    rule_id,
                    intent_id
                );
            }
            let next_step = if status == "failing" {
                format!(
                    "a failing gate blocks done — discharge it honestly, ONE of three ways: FIX the code then re-verdict passing; DEFER as tracked work (`loom hypothesis add` with the violation as the claim, then `loom hypothesis adopt --spawned` — a planned refactor the build lane owns, not a dead note); or if the violation is DELIBERATE, JUSTIFY it (`loom note add --intent {} --kind decision --text \"<why this is accepted>\"`, or re-verdict `independent` if the rule truly doesn't apply here). Marking needs_change alone discharges nothing.",
                    intent_id
                )
            } else {
                "`loom next --mode quality` for the next pair".to_string()
            };
            if printer.json {
                printer.print_json(&crate::output::with_read_anchor(
                    serde_json::json!({
                        "status":            "ok",
                        "rule_id":           rule_id,
                        "intent_id":         intent_id,
                        "inspection_status": status,
                        "criterion":         criterion,
                        "evidence":          evidence,
                        "confidence":        confidence,
                        "inspected_by":      by,
                        "last_inspected":    now,
                        "edge_created":      edge_created,
                    }),
                    &store,
                    &next_step,
                )?);
            } else {
                let mark = match status.as_str() {
                    "passing" => "✓",
                    "independent" => "◦",
                    _ => "✗",
                };
                println!(
                    "{} GOVERNS verdict recorded: {}{}",
                    mark,
                    status,
                    if edge_created {
                        "  (edge created — the verdict is the measurement)"
                    } else {
                        ""
                    }
                );
                println!("  rule   → {}", rule_id);
                println!("  intent → {}", intent_id);
                let snapshot = store.query_snapshot()?;
                let graph_state = store.graph_state(&snapshot)?;
                println!("  → Next: {next_step}");
                println!("  {}", crate::output::fmt_pulse(&graph_state));
            }
        }

        RuleCmd::List { limit } => run_list_with_db(&store, limit, printer)?,
        RuleCmd::Show { identifier } => run_show_with_db(&store, &identifier, printer)?,
        RuleCmd::Check { intent_id } => run_check_with_db(&store, intent_id, printer)?,
        RuleCmd::Recommend {
            intent_id,
            all,
            limit,
        } => run_recommend_with_db(&store, intent_id, all, limit, printer)?,
    }
    Ok(())
}

fn run_list_with_db(db: &dyn GraphReadRepository, limit: usize, printer: &Printer) -> Result<()> {
    let mut rules = db.list_rules()?;
    let total = crate::output::apply_limit(&mut rules, limit);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "rules":     rules,
            "total":     total,
            "truncated": rules.len() < total,
        }));
    } else if rules.is_empty() {
        println!("(no rules defined)");
    } else {
        for r in &rules {
            println!("{}", fmt_rule_row(r));
        }
        if let Some(m) =
            crate::output::more_marker(total, rules.len(), "`loom rule list --limit 0`")
        {
            println!("  {m}");
        }
    }
    Ok(())
}

/// `loom rule show <identifier>` — one rule's full record. Matches by NAME
/// first (the handle `loom rule list` prints), then by id, so a driver with
/// either works. The detail a quality-lane agent needs to hold the rule against
/// an intent without listing all rules and grepping.
fn run_show_with_db(
    db: &dyn GraphReadRepository,
    identifier: &str,
    printer: &Printer,
) -> Result<()> {
    let rules = db.list_rules()?;
    // Name is the human handle; id (a UUID) is the stable key. Try name first
    // — that's what a driver pastes from `loom rule list`'s left column.
    let rule = rules
        .iter()
        .find(|r| r.name == identifier)
        .or_else(|| rules.iter().find(|r| r.id == identifier))
        .ok_or_else(|| {
            let mut known: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
            known.sort();
            let sample = known.iter().take(8).copied().collect::<Vec<_>>().join(", ");
            anyhow::anyhow!(
                "no rule matches '{identifier}'. Known rule names: {sample}{}",
                if known.len() > 8 { " …" } else { "" }
            )
        })?;
    if printer.json {
        printer.print_json(rule);
    } else {
        println!("  {}  ({})", rule.name, rule.id);
        println!("  severity:           {}", rule.severity);
        if rule.kind.is_empty() {
            println!("  kind:               (uncategorized)");
        } else {
            println!("  kind:               {}", rule.kind);
        }
        println!(
            "  inspection_effort:  {}",
            if rule.inspection_effort.is_empty() {
                "mid (default)"
            } else {
                rule.inspection_effort.as_str()
            }
        );
        println!("  description:        {}", rule.description);
        println!("  detection_logic:    {}", rule.detection_logic);
        if !rule.evidence_examples.is_empty() {
            if let Ok(examples) = serde_json::from_str::<serde_json::Value>(&rule.evidence_examples)
            {
                println!("  evidence examples:");
                if let Some(pass) = examples.get("pass").and_then(|v| v.as_str()) {
                    println!("    pass:                {pass}");
                }
                if let Some(indep) = examples.get("independent").and_then(|v| v.as_str()) {
                    println!("    independent:         {indep}");
                }
                if let Some(fp) = examples
                    .get("common_false_positive")
                    .and_then(|v| v.as_str())
                {
                    println!("    common false pos:    {fp}");
                }
            }
        }
        if !rule.signal_expectations.is_empty() && rule.signal_expectations != "[]" {
            if let Ok(signals) = serde_json::from_str::<Vec<Vec<String>>>(&rule.signal_expectations)
            {
                if !signals.is_empty() {
                    println!(
                        "  signal expectations (keywords that should appear in passing evidence):"
                    );
                    for (i, group) in signals.iter().enumerate() {
                        println!("    group {}: {}", i + 1, group.join(" | "));
                    }
                }
            }
        }
        if !rule.applies_when.is_empty() && rule.applies_when != "{}" {
            println!("  applies_when:       {}", rule.applies_when);
        }
    }
    Ok(())
}

fn run_check_with_db(
    db: &dyn GraphReadRepository,
    intent_id: String,
    printer: &Printer,
) -> Result<()> {
    let intent_id = resolve_intent_with_db(db, &intent_id)?;
    let governs = db.list_governs_for_intent(&intent_id)?;
    let failing: Vec<_> = governs
        .iter()
        .filter(|g| g.inspection_status == "failing")
        .collect();
    let passing: Vec<_> = governs
        .iter()
        .filter(|g| g.inspection_status == "passing")
        .collect();
    let uninspected: Vec<_> = governs
        .iter()
        .filter(|g| g.inspection_status == "uninspected")
        .collect();
    let measure_hint = format!(
        "loom rule verdict <rule-id> {} --status passing|failing|independent --criterion … --evidence …",
        intent_id
    );
    if printer.json {
        let mut payload = serde_json::json!({
            "governs": governs,
            "total": governs.len(),
            "failing": failing.len(),
            "passing": passing.len(),
            "uninspected": uninspected.len(),
            "truncated": false,
        });
        if governs.is_empty() {
            payload["note"] = serde_json::json!(format!(
                "no rules measured against this intent — {measure_hint}"
            ));
        }
        printer.print_json(&payload);
    } else if governs.is_empty() {
        println!(
            "No GOVERNS edges for intent '{}' — no rules measured.",
            intent_id
        );
        println!("  → Measure a rule against it: {measure_hint}");
        println!("    (the verdict creates the edge and measures it in one step; independent = the rule does not apply)");
    } else {
        println!(
            "GOVERNS edges for intent '{}':  {} failing, {} passing, {} uninspected",
            intent_id,
            failing.len(),
            passing.len(),
            uninspected.len()
        );
        println!();
        for g in &failing {
            println!(
                "  [FAILING]  rule={rname}  criterion={crit}",
                rname = g.rule_name,
                crit = g.criterion,
            );
            if !g.evidence.is_empty() {
                println!("    evidence: {}", g.evidence);
            }
        }
        for g in &uninspected {
            println!("  [uninspected]  rule={}  (edge id: {})", g.rule_name, g.id);
        }
        for g in &passing {
            println!("  [passing]  rule={}", g.rule_name);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct RuleRecommendation {
    rule: RuleRecommendationRule,
    intent: RuleRecommendationIntent,
    score: f64,
    confidence: &'static str,
    reasons: Vec<String>,
    existing_governs_status: Option<String>,
    suggested_command: String,
}

#[derive(Debug, Clone, Serialize)]
struct RuleRecommendationRule {
    id: String,
    name: String,
    kind: String,
    severity: String,
}

#[derive(Debug, Clone, Serialize)]
struct RuleRecommendationIntent {
    id: String,
    name: String,
}

fn run_recommend_with_db(
    db: &dyn GraphReadRepository,
    intent_id: Option<String>,
    all: bool,
    limit: usize,
    printer: &Printer,
) -> Result<()> {
    let snapshot = db.query_snapshot()?;
    let target_ids = if all {
        snapshot
            .intents
            .iter()
            .filter(|intent| {
                intent.status != "deprecated" && snapshot.with_code.contains(intent.id.as_str())
            })
            .map(|intent| intent.id.clone())
            .collect::<Vec<_>>()
    } else {
        let key = intent_id.ok_or_else(|| {
            anyhow::anyhow!(
                "provide an intent id/name, or pass --all to recommend across coded intents"
            )
        })?;
        vec![crate::db::queries::resolve_intent_from_snapshot(
            &snapshot, &key,
        )?]
    };

    let mut recommendations = recommend_rules_from_snapshot(&snapshot, &target_ids);
    let total = recommendations.len();
    let n = if limit == 0 { total } else { limit.min(total) };
    recommendations.truncate(n);

    let next_step = "Inspect the suggested rule×intent pairs, then record truth with `loom rule verdict … --status passing|failing|independent`; recommendations are deterministic triage, not verdicts.";
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok",
            "total": total,
            "returned": recommendations.len(),
            "truncated": recommendations.len() < total,
            "recommendations": recommendations,
            "next_step": next_step,
        }));
        return Ok(());
    }

    if recommendations.is_empty() {
        println!("No high-signal rule recommendations found.");
        println!("  → {next_step}");
        return Ok(());
    }

    println!(
        "── Rule recommendations ({}/{total}) ─────────────────────────────",
        recommendations.len()
    );
    for rec in &recommendations {
        println!();
        println!(
            "  [{:.2} {}] {} → {}",
            rec.score, rec.confidence, rec.rule.name, rec.intent.name
        );
        for reason in &rec.reasons {
            println!("    - {reason}");
        }
        if let Some(status) = &rec.existing_governs_status {
            println!("    existing GOVERNS status: {status}");
        }
        println!("    next: {}", rec.suggested_command);
    }
    if recommendations.len() < total {
        println!();
        println!(
            "  (+{} more; use --limit 0 for all)",
            total - recommendations.len()
        );
    }
    println!();
    println!("  → {next_step}");
    Ok(())
}

fn recommend_rules_from_snapshot(
    snapshot: &crate::db::queries::QuerySnapshot,
    target_ids: &[String],
) -> Vec<RuleRecommendation> {
    let intent_by_id: std::collections::HashMap<&str, &crate::types::Intent> = snapshot
        .intents
        .iter()
        .map(|intent| (intent.id.as_str(), intent))
        .collect();
    let codefile_by_id: std::collections::HashMap<&str, &crate::types::CodeFile> = snapshot
        .codefiles
        .iter()
        .map(|codefile| (codefile.id.as_str(), codefile))
        .collect();
    let validates_by_intent = group_validations_by_intent(snapshot);
    let governs_by_pair: std::collections::HashMap<(&str, &str), &crate::types::Governs> = snapshot
        .governs
        .iter()
        .map(|g| ((g.rule_id.as_str(), g.intent_id.as_str()), g))
        .collect();

    let mut out = Vec::new();
    for intent_id in target_ids {
        let Some(intent) = intent_by_id.get(intent_id.as_str()).copied() else {
            continue;
        };
        if intent.status == "deprecated" {
            continue;
        }
        let groundings: Vec<&crate::types::Implements> = snapshot
            .implements
            .iter()
            .filter(|im| im.intent_id == intent.id)
            .collect();
        let codefiles: Vec<&crate::types::CodeFile> = groundings
            .iter()
            .filter_map(|im| codefile_by_id.get(im.codefile_id.as_str()).copied())
            .collect();
        let validations = validates_by_intent
            .get(intent.id.as_str())
            .cloned()
            .unwrap_or_default();
        let signals = IntentRuleSignals::new(intent, &groundings, &codefiles, &validations);

        for rule in &snapshot.rules {
            let existing = governs_by_pair
                .get(&(rule.id.as_str(), intent.id.as_str()))
                .copied();
            if existing.is_some_and(|g| {
                matches!(
                    g.inspection_status.as_str(),
                    "passing" | "failing" | "independent" | "partial"
                )
            }) {
                continue;
            }
            let Some((score, reasons)) = score_rule_for_intent(rule, &signals) else {
                continue;
            };
            if score < 0.5 {
                continue;
            }
            out.push(RuleRecommendation {
                rule: RuleRecommendationRule {
                    id: rule.id.clone(),
                    name: rule.name.clone(),
                    kind: rule.kind.clone(),
                    severity: rule.severity.clone(),
                },
                intent: RuleRecommendationIntent {
                    id: intent.id.clone(),
                    name: intent.name.clone(),
                },
                score,
                confidence: confidence_label(score),
                reasons,
                existing_governs_status: existing.map(|g| g.inspection_status.clone()),
                suggested_command: format!(
                    "loom rule verdict {} {} --status passing|failing|independent --criterion \"<criterion>\" --evidence \"<evidence>\"",
                    rule.name, intent.id
                ),
            });
        }
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.intent.name.cmp(&b.intent.name))
            .then_with(|| a.rule.name.cmp(&b.rule.name))
    });
    out
}

#[derive(Debug)]
struct IntentRuleSignals {
    text: String,
    paths: Vec<String>,
    imports: Vec<String>,
    validations: Vec<String>,
}

impl IntentRuleSignals {
    fn new(
        intent: &crate::types::Intent,
        groundings: &[&crate::types::Implements],
        codefiles: &[&crate::types::CodeFile],
        validations: &[&crate::types::Validation],
    ) -> Self {
        let mut text = format!(
            "{} {} {} {} {} {} {} {} {}",
            intent.name,
            intent.description,
            intent.criterion,
            intent.domain,
            intent.layer,
            intent.aspect,
            intent.lifecycle,
            intent.visibility,
            intent.boundary
        )
        .to_ascii_lowercase();
        for g in groundings {
            text.push(' ');
            text.push_str(&g.locator.to_ascii_lowercase());
            text.push(' ');
            text.push_str(&g.notes.to_ascii_lowercase());
        }
        let paths = codefiles
            .iter()
            .map(|cf| cf.path.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let imports = codefiles
            .iter()
            .flat_map(|cf| cf.imports.iter().map(|i| i.to_ascii_lowercase()))
            .collect::<Vec<_>>();
        let validations = validations
            .iter()
            .map(|v| {
                format!(
                    "{} {} {} {} {}",
                    v.name, v.description, v.validation_type, v.command, v.last_result
                )
                .to_ascii_lowercase()
            })
            .collect::<Vec<_>>();
        Self {
            text,
            paths,
            imports,
            validations,
        }
    }

    fn text_has_any(&self, needles: &[&str]) -> bool {
        needles.iter().any(|needle| self.text.contains(needle))
    }

    fn path_has_any(&self, needles: &[&str]) -> bool {
        self.paths
            .iter()
            .any(|path| needles.iter().any(|needle| path.contains(needle)))
    }

    fn import_has_any(&self, needles: &[&str]) -> bool {
        self.imports
            .iter()
            .any(|import| needles.iter().any(|needle| import.contains(needle)))
    }

    fn validation_has_all(&self, groups: &[&[&str]]) -> bool {
        self.validations.iter().any(|validation| {
            groups
                .iter()
                .all(|group| group.iter().any(|needle| validation.contains(needle)))
        })
    }

    fn text_has_any_owned(&self, needles: &[String]) -> bool {
        needles.iter().any(|needle| self.text.contains(needle))
    }

    fn path_has_any_owned(&self, needles: &[String]) -> bool {
        self.paths
            .iter()
            .any(|path| needles.iter().any(|needle| path.contains(needle)))
    }

    fn import_has_any_owned(&self, needles: &[String]) -> bool {
        self.imports
            .iter()
            .any(|import| needles.iter().any(|needle| import.contains(needle)))
    }

    fn validation_has_all_owned(&self, groups: &[Vec<String>]) -> bool {
        self.validations.iter().any(|validation| {
            groups
                .iter()
                .all(|group| group.iter().any(|needle| validation.contains(needle)))
        })
    }
}

fn score_rule_for_intent(
    rule: &crate::types::QualityRule,
    signals: &IntentRuleSignals,
) -> Option<(f64, Vec<String>)> {
    if !rule.applies_when.is_empty() && rule.applies_when != "{}" {
        return score_applies_when(&rule.applies_when, signals)
            .or_else(|| legacy_score_rule_for_intent(rule, signals));
    }
    legacy_score_rule_for_intent(rule, signals)
}

fn legacy_score_rule_for_intent(
    rule: &crate::types::QualityRule,
    signals: &IntentRuleSignals,
) -> Option<(f64, Vec<String>)> {
    let mut score: f64 = 0.0;
    let mut reasons: Vec<String> = Vec::new();
    let rule_name = rule.name.as_str();

    if rule_name.starts_with("docker-") {
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.text_has_any(&["docker", "container", "image", "dockerfile"]),
            "intent text mentions Docker/container/image packaging",
        );
        add_if(
            &mut score,
            &mut reasons,
            0.45,
            signals.path_has_any(&[
                "dockerfile",
                ".dockerignore",
                "docker-compose.yml",
                "docker-compose.yaml",
                "compose.yml",
                "compose.yaml",
            ]),
            "grounded files include Dockerfile/.dockerignore/compose artifacts",
        );
        if rule_name == "docker-build-proven" {
            let has_build_run = signals.validation_has_all(&[
                &["docker build", "podman build"],
                &["docker run", "podman run"],
            ]);
            add_if(
                &mut score,
                &mut reasons,
                0.30,
                !has_build_run,
                "no linked validation command proves both image build and image run",
            );
        }
    }

    if rule_name.starts_with("service-") {
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.text_has_any(&[
                "service", "endpoint", "http", "api", "webhook", "queue", "consumer", "boundary",
            ]),
            "intent text names a service/API/boundary surface",
        );
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.import_has_any(&[
                "axum", "actix", "rocket", "warp", "express", "fastapi", "reqwest", "hyper",
                "tonic",
            ]),
            "grounded files import service/client framework symbols",
        );
    }

    if rule_name.starts_with("data-") {
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.text_has_any(&[
                "data",
                "database",
                "migration",
                "schema",
                "sql",
                "pii",
                "record",
                "ingest",
            ]),
            "intent text names data/database/migration concerns",
        );
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.path_has_any(&[".sql", "migrations/", "/migrations"]),
            "grounded files include SQL or migrations",
        );
    }

    if rule_name.starts_with("webui-") {
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.text_has_any(&["ui", "screen", "view", "component", "button", "form"]),
            "intent text names UI/view/component behavior",
        );
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.path_has_any(&[".tsx", ".jsx", ".svelte", ".vue", ".css", ".html"]),
            "grounded files include frontend UI assets",
        );
    }

    if rule_name.starts_with("mobile-") {
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.text_has_any(&["mobile", "ios", "android", "screen", "permission"]),
            "intent text names mobile/platform concerns",
        );
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.path_has_any(&[".swift", ".kt", ".kts", ".dart", "ios/", "android/"]),
            "grounded files include mobile platform assets",
        );
    }

    if rule_name.starts_with("conc-") || rule_name == "perf-budget-proven" {
        add_if(
            &mut score,
            &mut reasons,
            0.35,
            signals.text_has_any(&[
                "concurrent",
                "thread",
                "async",
                "lock",
                "mutex",
                "channel",
                "latency",
                "throughput",
                "hot path",
                "performance",
                "optimize",
                "cache",
            ]),
            "intent text names concurrency/performance/optimization concerns",
        );
        add_if(
            &mut score,
            &mut reasons,
            0.25,
            signals.import_has_any(&["tokio", "rayon", "thread", "sync", "mutex", "channel"]),
            "grounded files import concurrency primitives/frameworks",
        );
        if rule_name == "perf-budget-proven" {
            let has_benchmark = signals.validation_has_all(&[&["benchmark", "bench"]]);
            add_if(
                &mut score,
                &mut reasons,
                0.25,
                !has_benchmark,
                "no linked benchmark validation is visible for the performance claim",
            );
        }
    }

    if rule.kind == "performance" {
        add_if(
            &mut score,
            &mut reasons,
            0.20,
            signals.text_has_any(&[
                "optimize",
                "optimization",
                "performance",
                "latency",
                "throughput",
                "cache",
                "startup",
                "size",
            ]),
            "performance-kind rule matches optimization/performance wording",
        );
    }

    if reasons.is_empty() {
        None
    } else {
        Some((score.min(1.0), reasons))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct AppliesWhen {
    #[serde(default)]
    signals: Vec<ApplySignal>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ApplySignal {
    source: String,
    #[serde(default)]
    terms: Vec<String>,
    #[serde(default)]
    groups: Vec<Vec<String>>,
    weight: f64,
    reason: String,
}

fn normalize_applies_when(value: Option<&str>) -> Result<String> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok("{}".to_string());
    };
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|e| anyhow::anyhow!("--applies-when must be valid JSON: {e}"))?;
    if !parsed.is_object() {
        anyhow::bail!("--applies-when must be a JSON object with a `signals` array");
    }
    let mut applies: AppliesWhen = serde_json::from_value(parsed)
        .map_err(|e| anyhow::anyhow!("--applies-when has invalid shape: {e}"))?;
    for signal in &mut applies.signals {
        normalize_apply_signal(signal);
        validate_apply_signal(signal)?;
    }
    if applies.signals.is_empty() {
        return Ok("{}".to_string());
    }
    serde_json::to_string(&applies).map_err(Into::into)
}

fn normalize_apply_signal(signal: &mut ApplySignal) {
    signal.source = signal.source.trim().to_ascii_lowercase();
    signal.terms = signal
        .terms
        .iter()
        .map(|term| term.trim().to_ascii_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    signal.groups = signal
        .groups
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|term| term.trim().to_ascii_lowercase())
                .filter(|term| !term.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .collect();
    signal.reason = signal.reason.trim().to_string();
}

fn validate_apply_signal(signal: &ApplySignal) -> Result<()> {
    match signal.source.as_str() {
        "intent_text" | "path" | "import" => {
            if signal.terms.is_empty() {
                anyhow::bail!("applies_when signal source '{}' requires non-empty terms", signal.source);
            }
        }
        "validation_all" | "missing_validation_all" => {
            if signal.groups.is_empty() || signal.groups.iter().any(Vec::is_empty) {
                anyhow::bail!("applies_when signal source '{}' requires non-empty groups", signal.source);
            }
        }
        other => anyhow::bail!(
            "unknown applies_when signal source '{}'; expected intent_text, path, import, validation_all, or missing_validation_all",
            other
        ),
    }
    if !signal.weight.is_finite() || signal.weight <= 0.0 {
        anyhow::bail!("applies_when signal weight must be a finite positive number");
    }
    if signal.reason.trim().is_empty() {
        anyhow::bail!("applies_when signal reason must be non-empty");
    }
    Ok(())
}

fn score_applies_when(value: &str, signals: &IntentRuleSignals) -> Option<(f64, Vec<String>)> {
    let applies: AppliesWhen = serde_json::from_str(value).ok()?;
    let mut score: f64 = 0.0;
    let mut reasons = Vec::new();
    for signal in &applies.signals {
        let matched = match signal.source.as_str() {
            "intent_text" => signals.text_has_any_owned(&signal.terms),
            "path" => signals.path_has_any_owned(&signal.terms),
            "import" => signals.import_has_any_owned(&signal.terms),
            "validation_all" => signals.validation_has_all_owned(&signal.groups),
            "missing_validation_all" => !signals.validation_has_all_owned(&signal.groups),
            _ => false,
        };
        if matched {
            score += signal.weight;
            reasons.push(signal.reason.clone());
        }
    }
    if reasons.is_empty() {
        None
    } else {
        Some((score.min(1.0), reasons))
    }
}

fn add_if(score: &mut f64, reasons: &mut Vec<String>, weight: f64, condition: bool, reason: &str) {
    if condition {
        *score += weight;
        reasons.push(reason.to_string());
    }
}

fn confidence_label(score: f64) -> &'static str {
    if score >= 0.8 {
        "high"
    } else if score >= 0.6 {
        "medium"
    } else {
        "low"
    }
}

fn group_validations_by_intent(
    snapshot: &crate::db::queries::QuerySnapshot,
) -> std::collections::HashMap<&str, Vec<&crate::types::Validation>> {
    let validation_by_id: std::collections::HashMap<&str, &crate::types::Validation> = snapshot
        .validations
        .iter()
        .map(|validation| (validation.id.as_str(), validation))
        .collect();
    let mut out: std::collections::HashMap<&str, Vec<&crate::types::Validation>> =
        std::collections::HashMap::new();
    for edge in &snapshot.validates {
        if let Some(validation) = validation_by_id.get(edge.validation_id.as_str()).copied() {
            out.entry(edge.intent_id.as_str())
                .or_default()
                .push(validation);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(name: &str) -> &'static [PackRule] {
        PACKS
            .iter()
            .find(|(n, _)| *n == name)
            .expect("pack exists")
            .1
    }
    fn has_rule(rules: &[PackRule], name: &str) -> bool {
        rules.iter().any(|r| r.name == name)
    }

    #[test]
    fn docker_pack_carries_container_creation_standards() {
        let docker = pack("docker");
        for r in [
            "docker-build-proven",
            "docker-multistage-minimal-runtime",
            "docker-cache-friendly-layers",
            "docker-context-pruned",
            "docker-non-root-runtime",
            "docker-no-secrets-in-image",
            "docker-runtime-contract-declared",
        ] {
            assert!(has_rule(docker, r), "docker pack missing rule '{r}'");
        }
        assert_eq!(pack_rule_effort("docker-context-pruned"), "low");
        assert_eq!(pack_rule_effort("docker-non-root-runtime"), "low");
        assert_eq!(pack_rule_effort("docker-no-secrets-in-image"), "low");
    }

    #[test]
    fn iso5055_pack_carries_baseline_rules() {
        let iso = pack("iso5055");
        for r in [
            "iso5055-rel-no-unchecked-failure",
            "iso5055-rel-resource-release",
            "iso5055-rel-boundary-validation",
            "iso5055-sec-no-injection",
            ISO5055_SEC_NO_HARDCODED_SECRETS,
            "iso5055-sec-least-surface",
            "iso5055-perf-bounded-work",
            "iso5055-perf-no-redundant-work",
            "iso5055-main-single-responsibility",
            ISO5055_MAIN_NO_DEAD_OR_DUPLICATE,
        ] {
            assert!(has_rule(iso, r), "iso5055 pack missing rule '{r}'");
        }
        assert_eq!(pack_rule_effort(ISO5055_SEC_NO_HARDCODED_SECRETS), "low");
    }

    #[test]
    fn mobile_pack_carries_lifecycle_rules() {
        let mobile = pack("mobile");
        for r in [
            MOBILE_LIFECYCLE_SAFE_STATE,
            "mobile-offline-behavior-defined",
            "mobile-permission-in-context",
            "mobile-main-thread-clear",
            "mobile-battery-respect",
            "mobile-platform-divergence-explicit",
            "mobile-external-entry-validated",
            "mobile-touch-target-size",
        ] {
            assert!(has_rule(mobile, r), "mobile pack missing rule '{r}'");
        }
    }

    #[test]
    fn service_pack_carries_integration_rules() {
        let service = pack("service");
        for r in [
            "service-contract-artifact",
            "service-idempotent-handlers",
            "service-timeout-retry-explicit",
            "service-compensation-defined",
            "service-auth-at-boundary",
            "service-observable-failures",
            "service-graceful-degradation",
            "service-compatible-evolution",
        ] {
            assert!(has_rule(service, r), "service pack missing rule '{r}'");
        }
    }

    #[test]
    fn security_deep_pack_carries_ai_security_rules() {
        let deep = pack("security-deep");
        for r in [
            "sec-dependency-squatting",
            "sec-rate-limiting",
            "sec-minimal-response",
            "sec-upload-validated",
        ] {
            assert!(has_rule(deep, r), "security-deep pack missing rule '{r}'");
        }
    }

    #[test]
    fn packs_registry_lists_all_seedable_packs() {
        let names = pack_names();
        for n in [
            "iso5055",
            "security-deep",
            "mobile",
            "web-ui",
            "service",
            "data",
            "concurrency",
            "docker",
        ] {
            assert!(names.contains(&n), "pack_names missing '{n}'");
        }
        assert_eq!(names.len(), PACKS.len());
    }

    /// Design-system standards (`design-system standards via QualityRule packs`):
    /// contrast and touch-target sticks ship in the web-ui/mobile packs so
    /// screens GOVERN against them instead of inventing the bar per screen.
    /// a11y, view states, and responsive breakpoints were already present.
    #[test]
    fn ui_packs_carry_the_design_system_standards() {
        let webui = pack("web-ui");
        for r in [
            "webui-accessible-interactive", // a11y (pre-existing)
            "webui-view-states-complete",   // loading/empty/error (pre-existing)
            "webui-responsive-declared",    // breakpoints (pre-existing)
            "webui-color-contrast",         // new
            "webui-touch-target-size",      // new
        ] {
            assert!(
                has_rule(webui, r),
                "web-ui pack missing design-system rule '{r}'"
            );
        }
        assert!(has_rule(pack("mobile"), "mobile-touch-target-size"));
    }

    /// New design-system rules default to mid inspection effort (no special-case
    /// entry needed) and are advisory `warning` severity.
    #[test]
    fn data_pack_carries_data_governance_rules() {
        let data = pack("data");
        for r in [
            "data-migration-reversible",
            "data-validated-at-ingest",
            "data-no-silent-loss",
            "data-pii-handled",
            "data-idempotent-reruns",
            "data-lineage-traceable",
        ] {
            assert!(has_rule(data, r), "data pack missing rule '{r}'");
        }
    }

    #[test]
    fn concurrency_pack_carries_concurrency_rules() {
        let conc = pack("concurrency");
        for r in [
            "conc-sync-discipline",
            "conc-no-lock-across-io",
            "conc-atomic-multi-step",
            "conc-deadlock-ordering",
            "conc-cancellation-safe",
            "conc-bounded-concurrency",
            "perf-budget-proven",
        ] {
            assert!(has_rule(conc, r), "concurrency pack missing rule '{r}'");
        }
    }

    #[test]
    fn docker_applies_signals_are_valid_json() {
        let applies: serde_json::Value =
            serde_json::from_str(DOCKER_APPLIES).expect("docker applies json");
        assert!(applies["signals"].is_array());
        let build: serde_json::Value =
            serde_json::from_str(DOCKER_BUILD_APPLIES).expect("docker build applies json");
        assert!(build["signals"].is_array());
    }

    fn sample_signals() -> IntentRuleSignals {
        let intent = crate::types::Intent {
            id: "i1".into(),
            name: "Docker Startup Latency".into(),
            description: "warm docker image startup".into(),
            criterion: String::new(),
            abstraction_level: "feature".into(),
            domain: "cli".into(),
            layer: String::new(),
            source_refs: vec![],
            status: "proposed".into(),
            aspect: String::new(),
            tags: vec![],
            visibility: String::new(),
            boundary: String::new(),
            lifecycle: "implemented".into(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let grounding = crate::types::Implements {
            id: "g1".into(),
            intent_id: "i1".into(),
            codefile_id: "c1".into(),
            intent_name: intent.name.clone(),
            codefile_path: "src/dockerfile.rs".into(),
            inspection_status: String::new(),
            criterion: String::new(),
            confidence: 0.0,
            evidence: String::new(),
            last_inspected: String::new(),
            inspected_by: String::new(),
            locator: "warm_cache".into(),
            notes: "startup notes".into(),
            created_at: String::new(),
        };
        let codefile = crate::types::CodeFile {
            id: "c1".into(),
            path: "src/Dockerfile".into(),
            language: "dockerfile".into(),
            last_modified: String::new(),
            imports: vec!["src/registry/auth.rs".into()],
            symbols: vec![],
            symbol_facts: vec![],
            content_hash: String::new(),
            extractor_grade: String::new(),
        };
        let validation = crate::types::Validation {
            id: "v1".into(),
            name: "docker smoke".into(),
            description: String::new(),
            validation_type: "test".into(),
            command: "docker build && docker run".into(),
            last_run: String::new(),
            last_result: "passed".into(),
            last_executed_run: String::new(),
            discrimination_status: String::new(),
        };
        IntentRuleSignals::new(&intent, &[&grounding], &[&codefile], &[&validation])
    }

    #[test]
    fn intent_rule_signals_match_text_path_imports_and_validations() {
        let signals = sample_signals();
        assert!(signals.text_has_any(&["docker", "latency"]));
        assert!(!signals.text_has_any(&["missing-term"]));
        assert!(signals.path_has_any(&["dockerfile"]));
        assert!(!signals.path_has_any(&["missing-path"]));
        assert!(signals.import_has_any(&["registry"]));
        assert!(!signals.import_has_any(&["missing-import"]));
        assert!(signals.validation_has_all(&[&["docker build"], &["docker run"]]));
        assert!(!signals.validation_has_all(&[&["docker build"], &["podman run"]]));
        assert!(signals.text_has_any_owned(&["startup".into()]));
        assert!(signals.path_has_any_owned(&["dockerfile".into()]));
        assert!(signals.import_has_any_owned(&["auth".into()]));
        assert!(signals
            .validation_has_all_owned(&[vec!["docker build".into()], vec!["docker run".into()],]));
    }

    #[test]
    fn pack_rule_constructors_set_metadata_defaults() {
        let basic = PackRule::new("demo-rule", "error", "desc", "det", "security");
        assert_eq!(basic.name, "demo-rule");
        assert_eq!(basic.evidence_examples, "");
        assert_eq!(basic.applies_when, "");
        let rich = PackRule::with_evidence(
            "rich-rule",
            "warning",
            "desc",
            "det",
            "architecture",
            r#"{"pass":"ok"}"#,
            r#"[["openapi"]]"#,
        );
        assert!(!rich.evidence_examples.is_empty());
        let scoped = rich.with_applies_when(DOCKER_APPLIES);
        assert_eq!(scoped.applies_when, DOCKER_APPLIES);
    }

    #[test]
    fn new_design_system_rules_are_mid_effort_warnings() {
        for name in [
            "webui-color-contrast",
            "webui-touch-target-size",
            "mobile-touch-target-size",
        ] {
            assert_eq!(pack_rule_effort(name), "mid", "{name} effort");
        }
        let webui = pack("web-ui");
        for r in webui {
            if r.name == "webui-color-contrast" || r.name == "webui-touch-target-size" {
                assert_eq!(r.severity, "warning", "{} severity", r.name);
            }
        }
    }
    #[test]
    fn add_if_conditional_scoring_accumulator() {
        let mut score = 0.0;
        let mut reasons: Vec<String> = Vec::new();
        add_if(&mut score, &mut reasons, 0.5, true, "matched text");
        add_if(&mut score, &mut reasons, 0.3, false, "no match");
        assert!(
            (score - 0.5).abs() < f64::EPSILON,
            "only true condition increments"
        );
        assert_eq!(reasons, vec!["matched text"]);
    }

    #[test]
    fn confidence_label_maps_score_to_tiers() {
        assert_eq!(confidence_label(0.9), "high");
        assert_eq!(confidence_label(0.8), "high");
        assert_eq!(confidence_label(0.75), "medium");
        assert_eq!(confidence_label(0.6), "medium");
        assert_eq!(confidence_label(0.5), "low");
        assert_eq!(confidence_label(0.0), "low");
    }

    #[test]
    fn group_validations_by_intent_buckets_by_intent_id() {
        let v1 = crate::types::Validation {
            id: "v1".into(),
            name: "smoke".into(),
            description: "".into(),
            validation_type: "test".into(),
            command: "cargo test".into(),
            last_run: "".into(),
            last_result: "not_run".into(),
            last_executed_run: "".into(),
            discrimination_status: "".into(),
        };
        let ve = crate::types::ValidatesEdge {
            id: "vv1".into(),
            validation_id: "v1".into(),
            intent_id: "i1".into(),
            validation_name: "smoke".into(),
            intent_name: "i1".into(),
            created_at: "t".into(),
            inspection_status: "current".into(),
            notes: "".into(),
        };
        let snap = crate::db::queries::QuerySnapshot::from_parts(
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![ve],
            vec![v1.clone()],
            vec![],
            vec![],
            None,
        );
        let grouped = group_validations_by_intent(&snap);
        assert_eq!(grouped.get("i1").unwrap().len(), 1);
        assert_eq!(grouped.get("i1").unwrap()[0].id, "v1");
    }

    #[test]
    fn normalize_applies_when_rejects_invalid_json() {
        assert!(normalize_applies_when(Some("{bad")).is_err());
        assert!(normalize_applies_when(Some(r#"{"signals":[]}"#)).is_ok());
        assert!(normalize_applies_when(None).is_ok());
    }

    #[test]
    fn normalize_apply_signal_lowercases_source_and_terms() {
        let json = r#"{"source":"INTENT_TEXT","terms":["Docker","Latency"],"weight":0.72,"reason":"custom rule"}"#;
        let mut signal: ApplySignal = serde_json::from_str(json).unwrap();
        normalize_apply_signal(&mut signal);
        assert_eq!(signal.source, "intent_text");
        assert_eq!(signal.terms, vec!["docker", "latency"]);
    }
}
