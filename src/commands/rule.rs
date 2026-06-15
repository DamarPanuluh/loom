use anyhow::Result;
use uuid::Uuid;

use crate::cli::RuleCmd;
use crate::db::schema::role;
use crate::db::{ensure_initialized, GraphReadHandle, GraphReadRepository};
use crate::gate;
use crate::output::{fmt_rule_row, Printer};
use crate::types::QualityRule;

/// The ISO 5055 measuring sticks: (name, severity, description, detection_logic).
/// Two-to-three CWE-grounded rules per quality characteristic, written so an
/// LLM holding one against an intent's code knows exactly what to look for.
/// They are sticks, not detectors — verdicts still come from inspection.
const ISO5055_PACK: &[(&str, &str, &str, &str)] = &[
    // Reliability
    ("iso5055-rel-no-unchecked-failure", "error",
     "ISO 5055 Reliability (CWE-252/248/391): every fallible operation's failure path is handled or explicitly propagated — no silently ignored return value, no exception/panic escaping a boundary uncaught.",
     "Inspect the intent's error paths: ignored Results/return codes, unwrap/expect on external input, bare catch-alls, missing error branches at I/O, parse, lock, and network boundaries."),
    ("iso5055-rel-resource-release", "error",
     "ISO 5055 Reliability (CWE-772/404): every acquired resource (file, lock, connection, handle) is released on ALL paths, including error paths.",
     "Look for acquisitions without RAII/defer/finally protection, locks held across I/O or awaits, and early returns that skip cleanup."),
    ("iso5055-rel-boundary-validation", "error",
     "ISO 5055 Reliability (CWE-20): external input (CLI args, file content, env vars, network data) is validated before use; invalid input yields a typed error, never corruption or a crash.",
     "Trace each external input to its first use: is there a validation/parse step with an error path before the value reaches logic or storage?"),
    // Security
    ("iso5055-sec-no-injection", "error",
     "ISO 5055 Security (CWE-89/78/79): untrusted data is never concatenated into SQL/shell/HTML/query strings — parameterize, escape at the boundary, or reject.",
     "Trace untrusted inputs to every interpreter sink (exec/system calls, query strings, format/eval, HTML output) and check the escaping/parameterization at each."),
    ("iso5055-sec-no-hardcoded-secrets", "error",
     "ISO 5055 Security (CWE-798): no credentials, tokens, or keys in source or config committed to the repo; secrets come from the environment or a secret store.",
     "Scan the intent's files for key-like literals, connection strings with passwords, and tokens; check how the code obtains credentials."),
    ("iso5055-sec-least-surface", "error",
     "ISO 5055 Security (CWE-284/732): expose the minimum — no debug/admin paths reachable in production flows, no overly-permissive file modes or defaults.",
     "Enumerate what the intent exposes (endpoints, files written, flags) and check each against who actually needs it."),
    // Performance efficiency
    ("iso5055-perf-bounded-work", "warning",
     "ISO 5055 Performance Efficiency (CWE-834/1050): no unbounded loops/recursion over external-sized data; iteration and queries are bounded, paginated, or capped.",
     "Look for loops over unbounded collections nested in loops (N+1 patterns), recursion without a depth guard, and full scans where a limit exists."),
    ("iso5055-perf-no-redundant-work", "warning",
     "ISO 5055 Performance Efficiency (CWE-1042/1046): no repeated identical I/O, queries, or allocation in hot paths — cache or hoist invariant work out of loops.",
     "Find work inside loops that is invariant across iterations (reads, compiles, allocations) and repeated identical calls that could be batched."),
    // Maintainability
    ("iso5055-main-single-responsibility", "warning",
     "ISO 5055 Maintainability (CWE-1080/1120): each unit (file, function, intent) owns one coherent responsibility; oversized or multi-concern units are split.",
     "Check unit sizes and concern count; cross-check `loom smells` (tangled_file / scattered_intent) for the same intent."),
    ("iso5055-main-no-dead-or-duplicate-code", "warning",
     "ISO 5055 Maintainability (CWE-561/1041): no unreachable or unused code; no copy-pasted logic where one definition should exist.",
     "Look for unused functions/exports, commented-out blocks kept 'just in case', and near-identical logic in sibling files."),
];

/// Mobile vantage point: lifecycle, offline, permissions, the main thread,
/// battery, platform divergence, externally-triggered entry points.
const MOBILE_PACK: &[(&str, &str, &str, &str)] = &[
    ("mobile-lifecycle-safe-state", "error",
     "Mobile: user-visible state survives backgrounding and process death — nothing critical lives only in memory across a lifecycle boundary.",
     "Trace each screen's state to its save/restore path (saved-state handles, persisted stores). Look for in-flight work assumed to finish after the app is backgrounded without an OS-sanctioned mechanism."),
    ("mobile-offline-behavior-defined", "error",
     "Mobile: every network-dependent feature defines its offline behavior — cached, queued, or an explicit user-facing error. Never an indefinite spinner or a crash.",
     "For each network call reachable from UI: what renders when the request can't start or times out? Look for fetches with no offline/error branch."),
    ("mobile-permission-in-context", "error",
     "Mobile: each platform permission is requested in the context of the feature that needs it, and denial leaves the app functional (degraded, not broken).",
     "List the manifest/Info.plist permissions; trace each to the feature using it, where it's requested, and the denial path."),
    ("mobile-main-thread-clear", "error",
     "Mobile: no blocking I/O, parsing, or heavy compute on the UI thread — frame budget is ~16ms.",
     "Look for synchronous file/DB/network access, large JSON decoding, or image work on the main thread/dispatcher."),
    ("mobile-battery-respect", "warning",
     "Mobile: no unbounded polling, wake locks, or sensor/location subscriptions without lifecycle-bound teardown.",
     "Find timers, location/sensor listeners, and sockets; check each is released when the screen/app stops."),
    ("mobile-platform-divergence-explicit", "warning",
     "Mobile: platform-specific behavior (iOS vs Android, OS-version gates) is isolated and named, not scattered through feature logic as inline conditionals.",
     "Grep platform checks (Platform.OS, Build.VERSION, #available); flag feature files mixing both platforms' branches inline."),
    ("mobile-external-entry-validated", "error",
     "Mobile (CWE-20/939): externally-triggered entry points — deep links, intents/universal links, push payloads — validate their input before navigation or action.",
     "Trace each deep-link/push handler: is the payload parsed and validated with a rejection path before it drives navigation, auth, or writes?"),
];

/// Web-UI vantage point: view states, accessibility, XSS, responsiveness,
/// feedback, client-side trust, URL-recoverable state.
const WEBUI_PACK: &[(&str, &str, &str, &str)] = &[
    ("webui-view-states-complete", "error",
     "Web UI: every data-driven view defines loading, empty, and error states — not just the populated happy state.",
     "For each component that renders fetched data: what shows while pending, when the result is empty, and when the request fails? A missing branch is a violation."),
    ("webui-accessible-interactive", "error",
     "Web UI (WCAG): interactive elements are keyboard-reachable and carry accessible names — real buttons/links, not bare clickable divs; focus is managed on dialogs/route changes.",
     "Look for onClick on non-interactive elements, icon buttons without labels, custom widgets without key handlers, and focus traps/restores on modals."),
    ("webui-no-unescaped-render", "error",
     "Web UI (CWE-79): user-controlled content never reaches innerHTML / dangerouslySetInnerHTML / raw template interpolation without sanitization.",
     "Trace user-originated strings to every raw-HTML sink; check the sanitizer (or its absence) at each."),
    ("webui-no-client-side-trust", "error",
     "Web UI (CWE-602): no secrets in the client bundle, and no authorization decision enforced only in the client — the server re-checks everything the UI hides.",
     "Scan client code/env for key-like literals; for each hidden/disabled privileged control, verify the corresponding server endpoint enforces the same rule."),
    ("webui-feedback-on-action", "warning",
     "Web UI: user actions give immediate feedback — pending/disabled/optimistic states; no silent in-flight gaps or double-submit windows.",
     "For each mutating action: what changes on screen between click and response? Look for submit buttons that stay active mid-flight."),
    ("webui-responsive-declared", "warning",
     "Web UI: layouts define behavior at small and large viewports — breakpoints are deliberate, content never becomes unreachable.",
     "Check key views at narrow widths: fixed widths, overflow without scroll, controls pushed off-canvas with no alternative."),
    ("webui-url-state-recoverable", "warning",
     "Web UI: state needed to recreate a view travels in the URL — refresh, back, and shared links land where the user expects.",
     "For each stateful view: refresh it. If the result differs from what was on screen (lost filters/selection/page), the state isn't URL-recoverable."),
];

/// Service/integration vantage point: contracts, idempotency, timeouts,
/// compensation (sagas), boundary auth, observability, degradation, compat.
const SERVICE_PACK: &[(&str, &str, &str, &str)] = &[
    ("service-contract-artifact", "error",
     "Service: every exposed interface has a committed, versioned contract artifact (schema/IDL/OpenAPI) that consumers can ground against — the seam's single shared truth.",
     "For each endpoint/event/queue the service exposes: where is the contract file, is it in the repo, and does the implementation actually match it?"),
    ("service-idempotent-handlers", "error",
     "Service: handlers for retriable inputs — webhooks, queue messages, payments — are idempotent; replaying the same message yields no duplicate effect.",
     "For each handler: what happens on exact redelivery? Look for inserts without dedup keys, counters without idempotency tokens, side effects before the dedup check."),
    ("service-timeout-retry-explicit", "error",
     "Service (CWE-1088): every outbound call carries an explicit timeout and a bounded retry policy with backoff — no infinite waits, no unbounded retry storms.",
     "Find each HTTP/DB/queue client call: is a timeout set (not the library's infinite default)? Is retry bounded with backoff and jitter?"),
    ("service-compensation-defined", "warning",
     "Service (sagas): multi-step workflows define compensation or abort for partial failure — no half-completed state without a recovery path an operator or the code can take.",
     "For each workflow spanning >1 service or transaction: enumerate the failure point after each step and name the compensating action. A missing one is the violation."),
    ("service-auth-at-boundary", "error",
     "Service (CWE-306/862): every externally reachable endpoint authenticates and authorizes before side effects — including 'internal' endpoints reachable from outside the trust zone.",
     "Enumerate reachable routes; for each, find the auth check and confirm it runs before any write or privileged read."),
    ("service-observable-failures", "warning",
     "Service: failures are logged/metric'd with enough context (ids, cause, upstream) to diagnose without reproducing.",
     "Pick the main failure paths: what exactly lands in logs/metrics? Catch-and-ignore blocks and bare 500s with no context are violations."),
    ("service-graceful-degradation", "warning",
     "Service: a dependency outage degrades the service (fallback, partial answer, fast error) — it never cascades into hangs or crash loops.",
     "For each hard dependency: trace what happens when it's down. Look for unguarded startup dependencies and synchronous calls on the hot path with no circuit/fallback."),
    ("service-compatible-evolution", "error",
     "Service: contract changes are additive or versioned — removing/renaming fields or changing semantics requires a version consumers can pin; old versions get a deprecation path.",
     "Diff the contract's history (or its change discipline): were fields ever removed/renamed in place? Is there a versioning convention at all?"),
];

/// Data vantage point: migrations, ingest validation, loss accounting,
/// PII handling, rerun safety, lineage.
const DATA_PACK: &[(&str, &str, &str, &str)] = &[
    ("data-migration-reversible", "error",
     "Data: schema migrations are ordered and repeatable, with a tested rollback — or an explicitly documented point of no return.",
     "Check the migration set: do down-migrations exist and run? For irreversible ones, is the irreversibility stated where the operator will see it?"),
    ("data-validated-at-ingest", "error",
     "Data (CWE-20): data entering storage is validated at the boundary, and invariants live in the schema (constraints, types, NOT NULL) — not only in application code.",
     "Trace each write path to storage: what rejects bad data? Look for app-side-only checks the schema doesn't enforce, and ingestion that bypasses the validated path."),
    ("data-no-silent-loss", "error",
     "Data: pipelines account for every record — rejects go to a dead-letter/quarantine with a cause, never dropped silently; counts in vs out reconcile.",
     "Find each filter/catch/skip in the pipeline: where do the excluded records go, and is the count surfaced anywhere a human looks?"),
    ("data-pii-handled", "error",
     "Data (CWE-359): personal/sensitive fields are identified, and access, retention, and deletion paths exist — a deletion request can actually be fulfilled.",
     "List fields holding personal data (and copies in logs/derived tables). For each: who can read it, how long it lives, and what a delete actually removes."),
    ("data-idempotent-reruns", "warning",
     "Data: pipeline stages re-run without duplicating or corrupting output — upsert/partition-overwrite semantics, not blind append.",
     "For each stage: run it twice on the same input (mentally or actually). Appends without keys and non-deterministic transforms are violations."),
    ("data-lineage-traceable", "warning",
     "Data: derived datasets name their sources — a consumer can trace a value back to its origin and know when it was computed.",
     "Pick a derived table/report: can you find what produced it, from what inputs, when? Untraceable derived data is the violation."),
];

/// Concurrency & measured-performance vantage point: synchronization
/// discipline, lock hygiene, atomicity, deadlock ordering, cancellation,
/// backpressure — plus the bridge rule that demands hot paths carry a
/// PROVEN budget (a benchmark validation), not a vibe.
const CONCURRENCY_PACK: &[(&str, &str, &str, &str)] = &[
    ("conc-sync-discipline", "error",
     "Concurrency (CWE-362/366): every piece of shared mutable state names its synchronization discipline — a lock, a single-writer thread/actor, atomics, or message passing. No ad-hoc unsynchronized access.",
     "Inventory state reachable from more than one thread/task; for each, name the discipline that guards it. State you cannot name a discipline for is the violation."),
    ("conc-no-lock-across-io", "error",
     "Concurrency (CWE-667): no lock is held across I/O, network calls, or await points — contention windows stay bounded by computation, not by external latency.",
     "Find each lock acquisition; trace what runs before release. File/DB/network access or an .await/blocking call inside the critical section is the violation."),
    ("conc-atomic-multi-step", "error",
     "Concurrency (CWE-362/367): multi-step state transitions (check-then-act, read-modify-write, exists-then-create) are atomic — one lock/transaction span — or explicitly designed to tolerate interleaving.",
     "Find check-then-act sequences on shared state (or storage): can another actor run between the steps? If yes and nothing tolerates that, it's a TOCTOU violation."),
    ("conc-deadlock-ordering", "error",
     "Concurrency (CWE-833): when more than one lock can be held at once, acquisition follows a single documented global order.",
     "List sites holding ≥2 locks; check the acquisition order is consistent everywhere and written down. Two sites taking A→B and B→A is the violation."),
    ("conc-cancellation-safe", "warning",
     "Concurrency: tasks/threads are cancellation-safe — interruption (timeout, shutdown, dropped future) leaves no half-written state and releases resources.",
     "For each spawned task: what happens if it's killed between its side effects? Look for multi-step writes without cleanup/transactions and resources freed only on the happy exit."),
    ("conc-bounded-concurrency", "warning",
     "Concurrency (CWE-400/770): spawns, queues, and in-flight work have explicit limits and backpressure — load sheds or blocks, it never grows unbounded.",
     "Find each spawn/enqueue driven by external input; name its bound (pool size, channel capacity, semaphore). An unbounded channel or per-request spawn with no cap is the violation."),
    ("perf-budget-proven", "error",
     "Measured performance: hot-path intents declare a performance budget in their criterion (e.g. 'p99 < 50ms at 10k entries') AND carry a benchmark validation proving it — fast is a claim, proven-fast is a state.",
     "Cross-check `loom hotspots`: for each high-centrality intent on a hot path, does its criterion state a number, and does a `benchmark`-type validation exist and pass? A budget without a benchmark (or vice versa) is the violation."),
];

/// All seedable packs, by name. `iso5055` is the baseline (applies to any code);
/// the rest are repo-kind vantage points — `loom detect` recommends which fit.
type PackRule = (&'static str, &'static str, &'static str, &'static str);
type Pack = (&'static str, &'static [PackRule]);

const PACKS: &[Pack] = &[
    ("iso5055", ISO5055_PACK),
    ("mobile", MOBILE_PACK),
    ("web-ui", WEBUI_PACK),
    ("service", SERVICE_PACK),
    ("data", DATA_PACK),
    ("concurrency", CONCURRENCY_PACK),
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
        "iso5055-sec-no-hardcoded-secrets" | "iso5055-main-no-dead-or-duplicate-code" => "low",
        // Deep semantic reading.
        "conc-atomic-multi-step"
        | "conc-deadlock-ordering"
        | "conc-cancellation-safe"
        | "service-compensation-defined"
        | "service-idempotent-handlers"
        | "mobile-lifecycle-safe-state"
        | "data-pii-handled" => "high",
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
        RuleCmd::Check { intent_id } => {
            let db = GraphReadHandle::open(&cwd)?;
            run_check_with_db(&db, intent_id, printer)
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
            effort,
        } => {
            gate::acting_in_lane("add a quality rule", &[role::QUALITY], None)?;
            severity
                .parse::<crate::types::Severity>()
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            if let Some(e) = &effort {
                if !matches!(e.as_str(), "low" | "mid" | "high") {
                    anyhow::bail!("--effort must be low, mid, or high (a statement about the inspection WORK, not about models).");
                }
            }
            let id = Uuid::new_v4().to_string();
            let rule = QualityRule {
                id: id.clone(),
                name: name.clone(),
                description,
                detection_logic: String::new(),
                inspection_effort: effort.unwrap_or_default(),
                severity,
            };
            store.insert_rule(&rule)?;

            if printer.json {
                printer.print_json(&rule);
            } else {
                println!("✓ Rule '{}' created  (id: {})", name, id);
            }
        }

        RuleCmd::Seed { pack } => {
            gate::acting_in_lane("seed a rule pack", &[role::QUALITY], None)?;
            let Some((_, rules)) = PACKS.iter().find(|(n, _)| *n == pack) else {
                anyhow::bail!(
                    "Unknown pack '{}'. Available: {} — `loom detect` recommends which fit this repo.",
                    pack,
                    pack_names().join(", ")
                );
            };
            let existing: std::collections::HashSet<String> =
                store.list_rules()?.into_iter().map(|r| r.name).collect();
            let mut created: Vec<QualityRule> = Vec::new();
            let mut skipped = 0usize;
            for (name, severity, description, detection) in *rules {
                if existing.contains(*name) {
                    skipped += 1;
                    continue;
                }
                let rule = QualityRule {
                    id: Uuid::new_v4().to_string(),
                    name: (*name).to_string(),
                    description: (*description).to_string(),
                    detection_logic: (*detection).to_string(),
                    inspection_effort: pack_rule_effort(name).to_string(),
                    severity: (*severity).to_string(),
                };
                store.insert_rule(&rule)?;
                created.push(rule);
            }
            if printer.json {
                printer.print_json(&serde_json::json!({
                    "status": "ok", "pack": pack,
                    "created": created, "skipped_existing": skipped,
                    "next": "loom next --mode quality now serves every coded intent these rules were never held against; one command resolves each — loom rule verdict.",
                }));
            } else {
                println!(
                    "✓ Seeded pack '{}': {} rule(s) created, {} already present.",
                    pack,
                    created.len(),
                    skipped
                );
                for r in &created {
                    println!("  + [{}] {}", r.severity, r.name);
                }
                println!("  → `loom next --mode quality` now serves every coded intent these were never held against;");
                println!("    one command resolves each: `loom rule verdict` (independent = measured, doesn't apply;");
                println!("    a verdict at component altitude covers its descendants).");
            }
        }

        RuleCmd::Apply {
            rule_id,
            intent_id,
            criterion,
        } => {
            gate::acting_in_lane("apply a quality rule", &[role::QUALITY], None)?;
            let rule_id = store.resolve_rule(&rule_id)?;
            let intent_id = resolve_intent_with_db(&store, &intent_id)?;
            let now = chrono::Utc::now().to_rfc3339();
            let crit = criterion.as_deref().unwrap_or("");
            if !crit.is_empty() {
                gate::require_substantive(
                    "criterion",
                    crit,
                    "what compliance looks like for this rule on this intent",
                )?;
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
                println!("✓ GOVERNS edge created  (id: {})", edge_id);
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
        } => {
            let by = gate::acting_in_lane(
                "record a GOVERNS verdict",
                &[role::QUALITY],
                inspected_by.as_deref(),
            )?;
            let rule_id = store.resolve_rule(&rule_id)?;
            let intent_id = resolve_intent_with_db(&store, &intent_id)?;
            if status != "passing" && status != "failing" && status != "independent" {
                anyhow::bail!(
                    "Invalid --status '{}'. A verdict is passing, failing, or independent.",
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
                    "why this rule does not apply to this intent"
                } else {
                    "what was actually found in the code during inspection"
                },
            )?;
            let evidence = gate::compose_evidence(&evidence_locator, &evidence)?;
            gate::require_confidence(confidence)?;

            let now = chrono::Utc::now().to_rfc3339();
            let mut found = store.update_governs_verdict(
                &rule_id, &intent_id, &status, &criterion, &evidence, confidence, &by, &now,
            )?;
            let mut edge_created = false;
            if !found {
                store.insert_governs(&rule_id, &intent_id, &criterion, &now)?;
                found = store.update_governs_verdict(
                    &rule_id, &intent_id, &status, &criterion, &evidence, confidence, &by, &now,
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
                    "flag the intent (`loom intent mark {} --lifecycle needs_change --reason \"…\"`) or fix and re-verdict.",
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
        RuleCmd::Check { intent_id } => run_check_with_db(&store, intent_id, printer)?,
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

fn resolve_intent_with_db(db: &dyn GraphReadRepository, key: &str) -> Result<String> {
    let intents = db.list_intents(None, None)?;
    if intents.iter().any(|intent| intent.id == key) {
        return Ok(key.to_string());
    }
    let kl = key.to_lowercase();
    let exact: Vec<_> = intents
        .iter()
        .filter(|intent| intent.name.to_lowercase() == kl)
        .collect();
    if exact.len() == 1 {
        return Ok(exact[0].id.clone());
    }
    if exact.len() > 1 {
        anyhow::bail!(
            "Intent name '{}' is not unique ({} intents carry it) — use the id. `loom intent list` to see them.",
            key,
            exact.len()
        );
    }
    let subs: Vec<_> = intents
        .iter()
        .filter(|intent| intent.name.to_lowercase().contains(&kl))
        .collect();
    match subs.len() {
        1 => Ok(subs[0].id.clone()),
        0 => anyhow::bail!(
            "No intent matches '{}' (by id, exact name, or name fragment). Run `loom intent list`.",
            key
        ),
        _ => {
            let total = subs.len();
            let shown = subs
                .iter()
                .take(10)
                .map(|intent| format!("'{}'", intent.name))
                .collect::<Vec<_>>()
                .join(", ");
            if total > 10 {
                anyhow::bail!(
                    "'{}' is ambiguous — it matches: {} … +{} more — narrow the fragment or `loom find \"{}\"`.",
                    key,
                    shown,
                    total - 10,
                    key
                );
            }
            anyhow::bail!(
                "'{}' is ambiguous — it matches: {}. Narrow the fragment or use an id.",
                key,
                shown
            )
        }
    }
}
