//! Quality rule packs — pre-authored, enriched `QualityRule` definitions.
//!
//! Plane: data. A pack rule ships with all the guidance fields that make LLM
//! verdicts consistent across sessions: an inspection guide, detection hints
//! (LLM-facing prose), an evidence template, and few-shot pass/fail examples.
//! Seeding upserts these as asserted `QualityRule` nodes with stable ids.
//!
//! The examples are the few-shot calibration channel: a small-context model
//! copies their phrasing and confidence discipline instead of inventing its
//! own inspection protocol per session.

use crate::model::NodeType;
use crate::store::Store;
use crate::Result;
use std::path::Path;

/// A worked verdict example: what a well-phrased criterion/evidence pair looks
/// like for this rule, with an honestly calibrated confidence.
pub struct Example {
    pub criterion: &'static str,
    pub evidence: &'static str,
    pub confidence: f64,
}

/// An enriched pack rule. `patterns` are cheap, language-agnostic regex candidate
/// finders for computed-on-read pre-screening; they are prompt hints only and do
/// not replace adjudicated quality verdicts.
pub struct PackRule {
    pub name: &'static str,
    pub category: &'static str,
    pub severity: &'static str,
    pub effort: &'static str,
    pub description: &'static str,
    pub inspection_guide: &'static str,
    pub detection_hints: &'static [&'static str],
    pub patterns: &'static [&'static str],
    pub evidence_passing: &'static str,
    pub evidence_failing: &'static str,
    pub example_passing: Example,
    pub example_failing: Example,
}

/// Names of all seedable packs (for `loom detect` / help / errors).
pub const PACKS: &[&str] = &[
    "iso5055",
    "service",
    "web-ui",
    "data",
    "concurrency",
    "docker",
];

/// The rules for a named pack, or empty if unknown.
pub fn pack(name: &str) -> &'static [PackRule] {
    match name {
        "iso5055" => ISO5055,
        "service" => SERVICE,
        "web-ui" => WEB_UI,
        "data" => DATA,
        "concurrency" => CONCURRENCY,
        "docker" => DOCKER,
        _ => &[],
    }
}

/// Recommend packs from honest repo  the same detection `loom detect`
/// displays and the compass consumes when the quality rung is unseeded.
/// Always returns at least `iso5055`; additional packs are added only when the
/// repo carries their marker, so a recommendation the seeder rejects is never
/// produced.
pub fn recommended_packs(root: &Path) -> Vec<&'static str> {
    let mut langs: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    let mut markers: Vec<&str> = Vec::new();
    for (marker, label) in [
        ("Cargo.toml", "rust"),
        ("package.json", "node"),
        ("go.mod", "go"),
        ("pyproject.toml", "python"),
        ("Dockerfile", "docker"),
    ] {
        if root.join(marker).exists() {
            markers.push(label);
        }
    }
    count_exts(root, &mut langs, 0);
    let mut recommended: Vec<&str> = ["iso5055"].to_vec();
    if markers.contains(&"docker") {
        recommended.push("docker");
    }
    if markers.contains(&"node") {
        recommended.push("web-ui");
        recommended.push("service");
    }
    if markers.contains(&"rust") || markers.contains(&"go") {
        recommended.push("concurrency");
    }
    if root.join("migrations").is_dir() || langs.contains_key("sql") {
        recommended.push("data");
    }
    recommended
}

fn count_exts(
    dir: &Path,
    langs: &mut std::collections::BTreeMap<&'static str, usize>,
    depth: usize,
) {
    if depth > 6 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            // Recommendations are advisory, so keep the public API infallible while making omissions visible.
            eprintln!(
                "warning: could not inspect '{}' for pack guidance: {err}",
                dir.display()
            );
            return;
        }
    };
    for entry in entries {
        let e = match entry {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!(
                    "warning: could not inspect an entry in '{}' for pack guidance: {err}",
                    dir.display()
                );
                continue;
            }
        };
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            count_exts(&p, langs, depth + 1);
        } else if let Some(ext) = p.extension().and_then(|x| x.to_str()) {
            let label = match ext {
                "rs" => "rust",
                "py" => "python",
                "go" => "go",
                "ts" | "tsx" => "typescript",
                "js" | "jsx" => "javascript",
                "sql" => "sql",
                _ => continue,
            };
            *langs.entry(label).or_insert(0) += 1;
        }
    }
}

const ISO5055: &[PackRule] = &[
    PackRule {
        name: "iso5055-rel-no-unchecked-failure",
        category: "defect",
        severity: "error",
        effort: "mid",
        description: "every fallible operation's failure path is handled or explicitly propagated",
        inspection_guide: "1. Find fallible calls (Result/Option, IO, parsing). 2. Confirm each \
            failure is handled or propagated with `?`. 3. A swallowed error (unwrap_or_default on a \
            real failure, ignored Result) is a violation.",
        detection_hints: &[
            "grep: unwrap(), expect(, let _ =, .ok();",
            "red flag: a Result discarded without handling",
        ],
        patterns: &[
            r#"\bunwrap\(\)"#,
            r#"\bexpect\s*\("#,
            r#"\blet\s+_\s*="#,
        ],
        evidence_passing: "src/<file>:<lines> — failures propagated via ? or handled explicitly",
        evidence_failing: "src/<file>:<line> — <call> discards its failure path",
        example_passing: Example {
            criterion: "every fallible call in the payment path propagates or handles its error",
            evidence: "src/pay.rs:40-92 — all 6 fallible calls use ? or match on Err; no discarded Result",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "every fallible call in the payment path propagates or handles its error",
            evidence: "src/pay.rs:57 — `let _ = charge(card)` discards the charge failure",
            confidence: 0.95,
        },
    },
    PackRule {
        name: "iso5055-sec-no-hardcoded-secrets",
        category: "security",
        severity: "error",
        effort: "low",
        description: "no credentials, tokens, or keys in source or committed config",
        inspection_guide: "1. Scan for literal secrets (API keys, passwords, private keys, \
            connection strings). 2. Confirm secrets come from env or a secret store. 3. A literal \
            credential in source is a violation.",
        detection_hints: &[
            "grep: api_key, secret, password, BEGIN PRIVATE KEY, token =",
            "red flag: a long high-entropy string literal assigned to a secret-like name",
        ],
        patterns: &[
            r#"(?i)(api[_-]?key|secret|password|token)\s*[:=]\s*["'][A-Za-z0-9+/_-]{16,}"#,
            r#"-----BEGIN (RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----"#,
        ],
        evidence_passing: "src/<file> — secrets read from env/secret store, none in source",
        evidence_failing: "src/<file>:<line> — literal secret in source",
        example_passing: Example {
            criterion: "no literal credential exists in the files grounding this intent",
            evidence: "src/auth.rs — all 3 secret-like names read std::env::var; grep for key/token/password literals found none",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "no literal credential exists in the files grounding this intent",
            evidence: "src/auth.rs:12 — `const API_KEY: &str = \"sk-live-…\"` is a literal production key",
            confidence: 0.97,
        },
    },
    PackRule {
        name: "iso5055-sec-no-injection",
        category: "security",
        severity: "error",
        effort: "mid",
        description: "untrusted data is never concatenated into SQL/shell/HTML/query strings",
        inspection_guide: "1. Find SQL/shell/HTML construction. 2. Confirm parameterization or \
            escaping at the boundary. 3. String-concatenated untrusted input is a violation.",
        detection_hints: &[
            "grep: format!(\"SELECT, Command::new, innerHTML, exec(",
            "red flag: user input interpolated into a query/command string",
        ],
        patterns: &[
            r#"(?i)format!\(\s*"(SELECT|INSERT|UPDATE|DELETE)\b"#,
            r#"(?i)\b(SELECT|INSERT|UPDATE|DELETE)\b.*(\+|\$\{|\{[A-Za-z_])"#,
            r#"(?i)\b(exec|system|popen)\s*\([^\n]*(\+|\$\{|format!)"#,
        ],
        evidence_passing: "src/<file>:<lines> — parameterized queries / escaped boundaries",
        evidence_failing: "src/<file>:<line> — untrusted input concatenated into a query",
        example_passing: Example {
            criterion: "all queries touching request data are parameterized",
            evidence: "src/db.rs:88-140 — every query uses params![]; no format! into SQL anywhere in the grounded files",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "all queries touching request data are parameterized",
            evidence: "src/db.rs:104 — `format!(\"SELECT … WHERE name = '{}'\", user_input)` interpolates the request body",
            confidence: 0.95,
        },
    },
    PackRule {
        name: "iso5055-rel-resource-release",
        category: "robustness",
        severity: "error",
        effort: "mid",
        description: "every acquired resource is released on all paths, including errors",
        inspection_guide: "1. Find resource acquisition (files, locks, connections, handles). 2. \
            Confirm release on all paths (RAII/Drop, defer, finally). 3. A leak on the error path \
            is a violation.",
        detection_hints: &[
            "grep: open(, lock(, connect(, acquire(",
            "in Rust, prefer RAII guards; a manual release missing on an early return is a leak",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — resources held by RAII guards, released on all paths",
        evidence_failing: "src/<file>:<line> — resource not released on the error path",
        example_passing: Example {
            criterion: "connections acquired in the import path are released on every exit",
            evidence: "src/import.rs:30-75 — connection wrapped in an RAII guard; both early returns drop it",
            confidence: 0.85,
        },
        example_failing: Example {
            criterion: "connections acquired in the import path are released on every exit",
            evidence: "src/import.rs:52 — early return on parse error skips `conn.close()` at line 71",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "iso5055-main-no-dead-or-duplicate-code",
        category: "defect",
        severity: "warning",
        effort: "low",
        description: "no unreachable or unused code; no copy-pasted logic where one definition belongs",
        inspection_guide: "1. Look for unreachable/unused functions. 2. Look for duplicated logic \
            across files. 3. Significant duplication or dead code is a violation.",
        detection_hints: &[
            "grep: #[allow(dead_code)], duplicated blocks",
            "red flag: the same logic copy-pasted in two places",
        ],
        patterns: &[],
        evidence_passing: "src/<file> — no dead code; shared logic factored",
        evidence_failing: "src/<file> — dead/duplicated code at <lines>",
        example_passing: Example {
            criterion: "the grounded files contain no unreachable functions or duplicated logic",
            evidence: "src/util.rs — every function referenced; the shared parse helper is defined once and imported",
            confidence: 0.8,
        },
        example_failing: Example {
            criterion: "the grounded files contain no unreachable functions or duplicated logic",
            evidence: "src/util.rs:120-160 duplicates src/io.rs:40-80 line-for-line (same retry loop)",
            confidence: 0.9,
        },
    },
];

const SERVICE: &[PackRule] = &[
    PackRule {
        name: "service-auth-at-boundary",
        category: "security",
        severity: "error",
        effort: "mid",
        description: "every externally reachable endpoint authenticates and authorizes before side effects",
        inspection_guide: "1. Find all handlers for this intent's routes. 2. Check each handler \
            verifies authentication AND authorization before any write/delete/mutate. 3. Look for \
            middleware, guards, or explicit checks. 4. Any handler with a side effect before its \
            auth check is a violation.",
        detection_hints: &[
            "grep: require_auth, authenticate, middleware, guard, is_admin",
            "red flag: a handler body that reaches the DB before any auth call",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — all handlers check <auth mechanism> before side effects",
        evidence_failing: "src/<file>:<line> — <handler> mutates at line <N> with no preceding auth check",
        example_passing: Example {
            criterion: "every mutating admin route verifies the admin role before executing",
            evidence: "src/routes/admin.rs:12-80 — require_admin() middleware wraps the router; verified all 4 handlers are inside it",
            confidence: 0.92,
        },
        example_failing: Example {
            criterion: "every mutating admin route verifies the admin role before executing",
            evidence: "src/routes/admin.rs:78 — delete_user() executes at line 82 with no auth call in the handler or its route chain",
            confidence: 0.95,
        },
    },
    PackRule {
        name: "service-input-validated-at-boundary",
        category: "security",
        severity: "error",
        effort: "mid",
        description: "untrusted input is parsed into a typed, validated form at the boundary, not deep inside",
        inspection_guide: "1. Find where request payloads enter. 2. Confirm they are parsed into \
            typed structures with validation (ranges, lengths, enums) at the boundary. 3. Raw \
            strings/maps flowing deep into domain logic before validation is a violation.",
        detection_hints: &[
            "grep: deserialize, validate, parse, from_str near handlers",
            "red flag: a handler passing req.body fields straight into domain calls",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — payloads deserialized into validated types at the handler",
        evidence_failing: "src/<file>:<line> — raw request field reaches <domain call> unvalidated",
        example_passing: Example {
            criterion: "the transfer endpoint validates amount and account ids before domain logic",
            evidence: "src/routes/transfer.rs:20-45 — TransferReq derives Deserialize + Validate (amount > 0, ids uuid); handler rejects on error before service call",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "the transfer endpoint validates amount and account ids before domain logic",
            evidence: "src/routes/transfer.rs:31 — body[\"amount\"] as raw string is passed into ledger::transfer with no validation",
            confidence: 0.93,
        },
    },
    PackRule {
        name: "service-outbound-timeouts",
        category: "robustness",
        severity: "error",
        effort: "low",
        description: "every outbound network call has an explicit timeout; retries are bounded",
        inspection_guide: "1. Find outbound HTTP/gRPC/DB/queue calls. 2. Confirm each client or \
            call site sets an explicit timeout. 3. If retried, confirm retry counts/backoff are \
            bounded. 4. A default-infinite client is a violation.",
        detection_hints: &[
            "grep: Client::new, timeout, connect_timeout, retry",
            "red flag: reqwest/hyper client built without .timeout(...)",
        ],
        patterns: &[
            r#"\bClient::new\(\)"#,
            r#"\breqwest::get\s*\("#,
            r#"(?i)\bnew\s+(HttpClient|Client)\s*\("#,
        ],
        evidence_passing: "src/<file>:<lines> — client configured with <timeout>; retries bounded at <n>",
        evidence_failing: "src/<file>:<line> — outbound call through a client with no timeout",
        example_passing: Example {
            criterion: "all outbound calls in the webhook dispatcher time out",
            evidence: "src/webhook.rs:15 — shared Client built with .timeout(10s); it is the only client used in the file",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "all outbound calls in the webhook dispatcher time out",
            evidence: "src/webhook.rs:15 — Client::new() with no timeout; a slow receiver hangs the dispatch loop",
            confidence: 0.93,
        },
    },
    PackRule {
        name: "service-mutations-idempotent-or-guarded",
        category: "robustness",
        severity: "warning",
        effort: "high",
        description: "a retried or duplicated mutation request cannot double-apply",
        inspection_guide: "1. Find externally triggerable mutations (payments, sends, inserts). \
            2. Check for an idempotency mechanism: idempotency key, uniqueness constraint, \
            compare-and-set, or transactional dedup. 3. A mutation that applies twice when the \
            caller retries is a violation.",
        detection_hints: &[
            "grep: idempotency, ON CONFLICT, unique, upsert, request_id",
            "red flag: an INSERT triggered by a webhook/retryable request with no dedup key",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — mutation deduplicated via <mechanism>",
        evidence_failing: "src/<file>:<line> — retrying this request re-applies the mutation",
        example_passing: Example {
            criterion: "a retried payment webhook cannot create a second charge record",
            evidence: "src/webhook.rs:44 + migrations/007.sql — charges.event_id UNIQUE; insert uses ON CONFLICT DO NOTHING",
            confidence: 0.88,
        },
        example_failing: Example {
            criterion: "a retried payment webhook cannot create a second charge record",
            evidence: "src/webhook.rs:44 — plain INSERT keyed by autoincrement id; provider retries produce duplicate charges",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "service-errors-structured-no-leak",
        category: "security",
        severity: "warning",
        effort: "low",
        description: "error responses are structured and never leak internals (stack traces, SQL, paths)",
        inspection_guide: "1. Find the error-to-response mapping. 2. Confirm external responses \
            use structured codes/messages. 3. Confirm internal detail (backtraces, SQL errors, \
            file paths) is logged, not returned. 4. A raw internal error in a response body is a \
            violation.",
        detection_hints: &[
            "grep: map_err, into_response, {:?} inside a response body",
            "red flag: format!(\"{e:?}\") returned to the caller",
        ],
        patterns: &[
            r#"format!\(\s*"\{[^"}]*:\?\}""#,
            r#"(?i)(response|body|reply|json)[^\n]*(stack trace|traceback|backtrace)"#,
        ],
        evidence_passing: "src/<file>:<lines> — errors mapped to structured responses; internals only logged",
        evidence_failing: "src/<file>:<line> — response body contains the raw internal error",
        example_passing: Example {
            criterion: "API errors return stable codes without internal detail",
            evidence: "src/error.rs:20-70 — ApiError maps every variant to {code, message}; Debug output goes to tracing only",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "API errors return stable codes without internal detail",
            evidence: "src/routes/user.rs:66 — `(500, format!(\"{e:?}\"))` returns the SQL error text to the client",
            confidence: 0.94,
        },
    },
];

const WEB_UI: &[PackRule] = &[
    PackRule {
        name: "webui-async-states-handled",
        category: "defect",
        severity: "error",
        effort: "mid",
        description: "every async data view renders loading, error, AND empty states, not only success",
        inspection_guide: "1. Find components/views that fetch data. 2. For each, check the \
            render path for loading, error, and empty branches. 3. A view that renders nothing or \
            stale content on failure is a violation.",
        detection_hints: &[
            "grep: isLoading, error, useQuery, fetch, suspense",
            "red flag: a component that destructures {data} and renders data.map(...) with no guards",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — loading/error/empty branches all rendered",
        evidence_failing: "src/<file>:<line> — <view> has no <missing state> branch",
        example_passing: Example {
            criterion: "the orders list renders all four fetch states",
            evidence: "src/OrdersList.tsx:18-60 — spinner on isLoading, retryable alert on error, explicit empty copy, list on data",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "the orders list renders all four fetch states",
            evidence: "src/OrdersList.tsx:24 — renders data.map(...) directly; a failed fetch leaves a blank region with no message",
            confidence: 0.92,
        },
    },
    PackRule {
        name: "webui-user-errors-visible",
        category: "defect",
        severity: "error",
        effort: "low",
        description: "a failed user action always surfaces a visible, actionable message",
        inspection_guide: "1. Find user-triggered mutations (submit, save, delete). 2. Trace the \
            failure path to the UI. 3. console.error or a swallowed catch with no visible feedback \
            is a violation.",
        detection_hints: &[
            "grep: catch, onError, toast, alert, setError",
            "red flag: .catch(console.error) as the whole failure handling",
        ],
        patterns: &[
            r#"\.catch\(\s*console\.error\s*\)"#,
            r#"catch\s*\([^)]*\)\s*\{\s*console\.error\s*\("#,
        ],
        evidence_passing: "src/<file>:<lines> — failures reach the user via <mechanism>",
        evidence_failing: "src/<file>:<line> — <action> failure is logged but never shown",
        example_passing: Example {
            criterion: "a failed profile save is visible to the user",
            evidence: "src/Profile.tsx:71 — onError sets a form-level alert with the retry affordance",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "a failed profile save is visible to the user",
            evidence: "src/Profile.tsx:71 — save().catch(console.error); the form stays silent on failure",
            confidence: 0.94,
        },
    },
    PackRule {
        name: "webui-no-client-secrets",
        category: "security",
        severity: "error",
        effort: "low",
        description: "no privileged keys or secrets ship in client-delivered code; the server mediates privileged calls",
        inspection_guide: "1. Scan client source and build config for keys/tokens. 2. Distinguish \
            public keys (fine) from privileged ones. 3. A privileged secret reachable in the \
            bundle is a violation.",
        detection_hints: &[
            "grep: apiKey, secret, token, PRIVATE, process.env in client code",
            "red flag: a server-scope credential in NEXT_PUBLIC_/VITE_ env exposure",
        ],
        patterns: &[
            r#"(?i)(api[_-]?key|secret|password|token)\s*[:=]\s*["'][A-Za-z0-9+/_-]{16,}"#,
            r#"\b(NEXT_PUBLIC|VITE|PUBLIC)_[A-Z0-9_]*(SECRET|TOKEN|KEY)\b"#,
        ],
        evidence_passing: "src/<files> — only public identifiers in client code; privileged calls proxied via <server path>",
        evidence_failing: "src/<file>:<line> — privileged <secret kind> shipped to the client",
        example_passing: Example {
            criterion: "the client bundle contains no privileged credentials",
            evidence: "web/src — grep for key/token/secret found only the public analytics id; payments go through /api/pay server route",
            confidence: 0.88,
        },
        example_failing: Example {
            criterion: "the client bundle contains no privileged credentials",
            evidence: "web/src/pay.ts:9 — VITE_STRIPE_SECRET_KEY (server-scope sk_ key) referenced in client code",
            confidence: 0.96,
        },
    },
    PackRule {
        name: "webui-state-recoverable",
        category: "robustness",
        severity: "warning",
        effort: "mid",
        description: "reload and deep links reconstruct view state; no dead-end screens that only in-memory state can reach",
        inspection_guide: "1. Identify stateful views (filters, wizards, selected items). 2. Check \
            whether the defining state lives in the URL or storage. 3. A view that breaks or blanks \
            on reload is a violation.",
        detection_hints: &[
            "grep: useSearchParams, router.query, localStorage, history.push",
            "red flag: a multi-step wizard whose step lives only in useState",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — defining state serialized to <URL/storage>; reload verified",
        evidence_failing: "src/<file>:<line> — reload of <view> loses <state> and dead-ends",
        example_passing: Example {
            criterion: "the search results view survives reload with filters intact",
            evidence: "src/Search.tsx:30 — filters serialize to query params; opening the copied URL reproduces the view",
            confidence: 0.85,
        },
        example_failing: Example {
            criterion: "the search results view survives reload with filters intact",
            evidence: "src/Search.tsx:30 — filters live in useState only; reload resets to the unfiltered view and the shared URL is meaningless",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "webui-interaction-latency-guarded",
        category: "performance",
        severity: "warning",
        effort: "mid",
        description: "expensive work stays off the interaction path: inputs debounced, renders not synchronously blocked",
        inspection_guide: "1. Find handlers on high-frequency events (typing, scroll, drag). 2. \
            Check for debounce/throttle/deferral of expensive work. 3. Synchronous heavy \
            computation or a network call per keystroke is a violation.",
        detection_hints: &[
            "grep: onChange, onScroll, debounce, throttle, useMemo, useDeferredValue",
            "red flag: a fetch fired inside onChange with no debounce",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — <event> work deferred via <mechanism>",
        evidence_failing: "src/<file>:<line> — <expensive work> runs synchronously per <event>",
        example_passing: Example {
            criterion: "typing in the search box does not fire a request per keystroke",
            evidence: "src/SearchBox.tsx:22 — query debounced 300ms before the fetch; verified one request per pause in the network log",
            confidence: 0.88,
        },
        example_failing: Example {
            criterion: "typing in the search box does not fire a request per keystroke",
            evidence: "src/SearchBox.tsx:22 — fetch inside onChange, no debounce; 12 requests for a 12-char query",
            confidence: 0.92,
        },
    },
];

const DATA: &[PackRule] = &[
    PackRule {
        name: "data-migrations-versioned",
        category: "robustness",
        severity: "error",
        effort: "low",
        description: "schema changes happen through versioned, ordered migrations, never manual drift",
        inspection_guide: "1. Find the migrations directory/tooling. 2. Confirm every schema \
            object referenced by code has a creating migration. 3. Schema referenced in code but \
            created by hand (no migration) is a violation.",
        detection_hints: &[
            "grep: migrations/, CREATE TABLE, schema_version, refinery/sqlx migrate",
            "red flag: code querying a table no migration creates",
        ],
        patterns: &[],
        evidence_passing: "migrations/ — every table/index used by <files> has an ordered migration",
        evidence_failing: "src/<file>:<line> — <object> queried but no migration creates it",
        example_passing: Example {
            criterion: "every table the billing module touches is migration-managed",
            evidence: "migrations/001-014 create charges, invoices, plans; grep found no other tables referenced in src/billing/",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "every table the billing module touches is migration-managed",
            evidence: "src/billing/report.rs:33 queries `charge_summaries`; no migration creates it (hand-made in prod)",
            confidence: 0.92,
        },
    },
    PackRule {
        name: "data-multi-step-transactional",
        category: "defect",
        severity: "error",
        effort: "high",
        description: "multi-write mutations are atomic (one transaction) or explicitly compensated; partial states are unreachable",
        inspection_guide: "1. Find mutations that write more than one row/table/system. 2. Confirm \
            a transaction wraps them, or a compensation/outbox handles partial failure. 3. Two \
            sequential writes where a crash between them leaves inconsistent state is a violation.",
        detection_hints: &[
            "grep: begin, transaction, commit, outbox, saga",
            "red flag: two .execute() calls in sequence with no tx for one logical change",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — <mutation> wrapped in one transaction / compensated via <mechanism>",
        evidence_failing: "src/<file>:<lines> — crash between line <A> and <B> leaves <inconsistency>",
        example_passing: Example {
            criterion: "account transfer debits and credits atomically",
            evidence: "src/ledger.rs:50-72 — both updates inside one BEGIN..COMMIT; early error rolls back",
            confidence: 0.92,
        },
        example_failing: Example {
            criterion: "account transfer debits and credits atomically",
            evidence: "src/ledger.rs:50-72 — debit commits at 58, credit at 70; a crash at 60 loses money with no compensation",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "data-invariants-in-schema",
        category: "defect",
        severity: "warning",
        effort: "mid",
        description: "critical data invariants (uniqueness, references, non-null) are enforced by schema constraints, not only application code",
        inspection_guide: "1. List the invariants the code assumes (unique emails, valid foreign \
            refs, required fields). 2. Check the schema declares them (UNIQUE, FK, NOT NULL, \
            CHECK). 3. An invariant enforced only by an app-level `if` is a violation.",
        detection_hints: &[
            "grep migrations for: UNIQUE, REFERENCES, NOT NULL, CHECK",
            "red flag: code checking uniqueness with a SELECT before INSERT (racy) instead of a constraint",
        ],
        patterns: &[],
        evidence_passing: "migrations/<file> — <invariant> enforced by <constraint>",
        evidence_failing: "src/<file>:<line> — <invariant> checked only in code; schema allows the violation",
        example_passing: Example {
            criterion: "user email uniqueness is guaranteed under concurrency",
            evidence: "migrations/003.sql — users.email UNIQUE; insert path handles the conflict error",
            confidence: 0.92,
        },
        example_failing: Example {
            criterion: "user email uniqueness is guaranteed under concurrency",
            evidence: "src/users.rs:41 — SELECT-then-INSERT uniqueness check; no UNIQUE constraint, so two concurrent signups both pass",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "data-hot-path-query-shape",
        category: "performance",
        severity: "warning",
        effort: "mid",
        description: "hot paths avoid N+1 query loops and unbounded scans; frequent lookups are indexed",
        inspection_guide: "1. Identify request-frequency paths that query. 2. Look for queries in \
            loops and full scans over unbounded tables. 3. Check indices exist for frequent \
            predicates. 4. An N+1 loop or an unindexed hot predicate is a violation.",
        detection_hints: &[
            "grep: for … query, SELECT inside a loop body, LIKE '%…%'",
            "red flag: fetching a list then querying per element",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — batched query / indexed predicate on the hot path",
        evidence_failing: "src/<file>:<lines> — query inside the loop over <collection> (N+1)",
        example_passing: Example {
            criterion: "the dashboard loads its rows in a constant number of queries",
            evidence: "src/dashboard.rs:30-55 — one JOIN fetches orders+users; migrations/009 adds the (user_id, created_at) index it uses",
            confidence: 0.85,
        },
        example_failing: Example {
            criterion: "the dashboard loads its rows in a constant number of queries",
            evidence: "src/dashboard.rs:38 — per-order user lookup inside the loop; 200 orders = 201 queries",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "data-pii-deliberate",
        category: "security",
        severity: "error",
        effort: "mid",
        description: "personal data is stored and logged only where deliberately decided; never in logs by accident",
        inspection_guide: "1. Identify PII fields (emails, names, addresses, tokens). 2. Trace \
            where they are written: tables, logs, traces, analytics. 3. PII in log lines or debug \
            dumps of whole records is a violation.",
        detection_hints: &[
            "grep: log/info/debug lines containing email, user, {:?} of user structs",
            "red flag: tracing::info!(?user) dumping a record with PII fields",
        ],
        patterns: &[
            r#"(?i)\b(log|logger|tracing|console)\b[^\n]*(email|phone|address|ssn|token)"#,
            r#"(?i)\b(info|debug|trace|warn|error)!?\s*\([^\n]*(email|phone|address|ssn|token)"#,
        ],
        evidence_passing: "src/<files> — PII confined to <tables>; log sites print ids only",
        evidence_failing: "src/<file>:<line> — <PII field> written to logs",
        example_passing: Example {
            criterion: "no PII reaches the log stream",
            evidence: "src/ — all 9 log sites touching user data print user.id only; the User Debug impl redacts email",
            confidence: 0.87,
        },
        example_failing: Example {
            criterion: "no PII reaches the log stream",
            evidence: "src/signup.rs:27 — info!(\"new signup {}\", user.email) writes the address to app logs",
            confidence: 0.93,
        },
    },
];

const CONCURRENCY: &[PackRule] = &[
    PackRule {
        name: "conc-shared-state-guarded",
        category: "defect",
        severity: "error",
        effort: "high",
        description: "shared mutable state is reached only through synchronization or ownership transfer",
        inspection_guide: "1. Find state reachable from more than one thread/task. 2. Confirm \
            access goes through a lock, atomic, channel, or single-owner task. 3. In unsafe/FFI \
            code, check aliasing by hand. 4. Unsynchronized shared mutation is a violation.",
        detection_hints: &[
            "grep: static mut, Arc<Mutex, RwLock, AtomicUsize, unsafe",
            "red flag: Arc<SomeStruct> with interior mutation through UnsafeCell-ish patterns",
        ],
        patterns: &[
            r#"\bstatic\s+mut\b"#,
            r#"\bUnsafeCell\b"#,
        ],
        evidence_passing: "src/<file>:<lines> — shared state behind <primitive>; no unsynchronized path",
        evidence_failing: "src/<file>:<line> — <state> mutated from <contexts> without synchronization",
        example_passing: Example {
            criterion: "the connection counter is race-free across worker tasks",
            evidence: "src/server.rs:19 — AtomicUsize with fetch_add; no other mutation path exists",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "the connection counter is race-free across worker tasks",
            evidence: "src/server.rs:19 — `static mut COUNT` incremented from every accept task; a data race by construction",
            confidence: 0.95,
        },
    },
    PackRule {
        name: "conc-no-lock-across-blocking",
        category: "robustness",
        severity: "error",
        effort: "mid",
        description: "no lock is held across an await point or blocking I/O call",
        inspection_guide: "1. Find lock acquisitions. 2. Check the guard's live range for .await \
            or blocking calls. 3. A std::sync lock guard alive across .await, or any lock held \
            during long blocking I/O, is a violation.",
        detection_hints: &[
            "grep: .lock(, .await in the same fn body",
            "red flag: let g = m.lock(); …; something.await while g is alive",
        ],
        patterns: &[
            r#"\.lock\s*\([^\n]*\.await\b"#,
            r#"\.await\b[^\n]*\.lock\s*\("#,
        ],
        evidence_passing: "src/<file>:<lines> — guards dropped before await/blocking calls",
        evidence_failing: "src/<file>:<lines> — guard from line <A> alive across the await at line <B>",
        example_passing: Example {
            criterion: "the cache lock never spans an await",
            evidence: "src/cache.rs:44-60 — value cloned inside a block scope; the guard drops before the fetch().await",
            confidence: 0.88,
        },
        example_failing: Example {
            criterion: "the cache lock never spans an await",
            evidence: "src/cache.rs:47 — MutexGuard from line 45 is alive across fetch().await at line 47; contention stalls the executor",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "conc-bounded-queues",
        category: "robustness",
        severity: "warning",
        effort: "mid",
        description: "channels and queues are bounded (or their unboundedness is a recorded decision); producers experience backpressure",
        inspection_guide: "1. Find channel/queue construction. 2. Check bounds. 3. For each \
            unbounded one, look for a recorded justification and a reason the producer cannot \
            outrun the consumer. 4. An unbounded queue fed by external input is a violation.",
        detection_hints: &[
            "grep: unbounded_channel, channel(, VecDeque, spawn per request",
            "red flag: mpsc::unbounded_channel receiving network-driven events",
        ],
        patterns: &[
            r#"\bunbounded_(channel|queue)\b"#,
            r#"\bmpsc::unbounded\b"#,
        ],
        evidence_passing: "src/<file>:<line> — channel bounded at <n> / unbounded with recorded rationale",
        evidence_failing: "src/<file>:<line> — unbounded queue fed by <external source>",
        example_passing: Example {
            criterion: "the event pipeline applies backpressure to producers",
            evidence: "src/pipeline.rs:12 — mpsc::channel(1024); send .await blocks producers when full",
            confidence: 0.88,
        },
        example_failing: Example {
            criterion: "the event pipeline applies backpressure to producers",
            evidence: "src/pipeline.rs:12 — unbounded_channel fed by every websocket message; a slow consumer grows memory without limit",
            confidence: 0.9,
        },
    },
    PackRule {
        name: "conc-cancellation-safe",
        category: "defect",
        severity: "warning",
        effort: "high",
        description: "cancelling a task cannot leave persistent state half-written",
        inspection_guide: "1. Find tasks that can be cancelled (select!, timeouts, dropped \
            futures, shutdown). 2. Check persistent multi-step writes inside them. 3. A write \
            sequence that a cancellation can cut in half, with no transaction/cleanup, is a \
            violation.",
        detection_hints: &[
            "grep: select!, timeout(, abort(, drop of JoinHandle",
            "red flag: a select! arm racing a multi-write future against shutdown",
        ],
        patterns: &[],
        evidence_passing: "src/<file>:<lines> — cancellable writes are atomic / guarded by <mechanism>",
        evidence_failing: "src/<file>:<lines> — cancellation between <A> and <B> leaves <partial state>",
        example_passing: Example {
            criterion: "shutdown cannot leave a half-applied checkpoint",
            evidence: "src/checkpoint.rs:30-58 — write to temp file + atomic rename; cancellation before rename leaves the old checkpoint intact",
            confidence: 0.87,
        },
        example_failing: Example {
            criterion: "shutdown cannot leave a half-applied checkpoint",
            evidence: "src/checkpoint.rs:30-58 — select! races shutdown against a two-file write; cancel after the first write corrupts the pair",
            confidence: 0.88,
        },
    },
    PackRule {
        name: "conc-lock-order",
        category: "defect",
        severity: "error",
        effort: "high",
        description: "when multiple locks are taken, every path acquires them in one consistent order",
        inspection_guide: "1. Find functions holding more than one lock. 2. Extract each path's \
            acquisition order. 3. Two paths acquiring the same pair in opposite orders is a \
            deadlock and a violation. 4. Check the order is documented where the locks live.",
        detection_hints: &[
            "grep: two .lock() calls in one scope",
            "red flag: fn a() locks X then Y; fn b() locks Y then X",
        ],
        patterns: &[],
        evidence_passing: "src/<files> — all multi-lock paths follow the order <X before Y>, documented at <site>",
        evidence_failing: "src/<file>:<lines> — <path A> locks X→Y while <path B> locks Y→X",
        example_passing: Example {
            criterion: "accounts and audit locks are never acquired in conflicting order",
            evidence: "src/bank.rs — both multi-lock paths (transfer:40, audit:88) take accounts before audit; order documented at bank.rs:12",
            confidence: 0.86,
        },
        example_failing: Example {
            criterion: "accounts and audit locks are never acquired in conflicting order",
            evidence: "src/bank.rs — transfer:40 locks accounts→audit; report:88 locks audit→accounts; concurrent calls deadlock",
            confidence: 0.9,
        },
    },
];

const DOCKER: &[PackRule] = &[
    PackRule {
        name: "docker-non-root",
        category: "security",
        severity: "error",
        effort: "low",
        description: "the final image runs as a non-root user",
        inspection_guide: "1. Open the Dockerfile's final stage. 2. Confirm a USER directive \
            switches to a non-root user before the entrypoint. 3. No USER (implicit root) is a \
            violation.",
        detection_hints: &[
            "grep Dockerfile: USER, adduser, useradd",
            "red flag: no USER directive after the last FROM",
        ],
        patterns: &[],
        evidence_passing: "Dockerfile:<line> — USER <name> set in the final stage",
        evidence_failing: "Dockerfile — final stage has no USER directive; container runs as root",
        example_passing: Example {
            criterion: "the runtime container is non-root",
            evidence: "Dockerfile:24 — `USER app` after copying the binary; entrypoint runs as uid 1000",
            confidence: 0.95,
        },
        example_failing: Example {
            criterion: "the runtime container is non-root",
            evidence: "Dockerfile — no USER directive in the final stage; process runs as root",
            confidence: 0.95,
        },
    },
    PackRule {
        name: "docker-pinned-bases",
        category: "robustness",
        severity: "warning",
        effort: "low",
        description: "base images are pinned to an exact version (tag, ideally digest), never a floating latest",
        inspection_guide: "1. Read every FROM line. 2. Confirm an exact version tag or digest. 3. \
            `latest` or a bare image name is a violation.",
        detection_hints: &[
            "grep Dockerfile: FROM",
            "red flag: FROM node:latest or FROM debian with no tag",
        ],
        patterns: &[
            r#"(?i)^\s*FROM\s+[^:@\s]+\s*$"#,
            r#"(?i)^\s*FROM\s+[^@\s]+:latest\b"#,
        ],
        evidence_passing: "Dockerfile:<lines> — all FROM lines pinned to <versions>",
        evidence_failing: "Dockerfile:<line> — FROM <image> floats on latest",
        example_passing: Example {
            criterion: "builds are reproducible across pulls",
            evidence: "Dockerfile:1,14 — rust:1.79.0 and debian:bookworm-20240612-slim, both exact tags",
            confidence: 0.95,
        },
        example_failing: Example {
            criterion: "builds are reproducible across pulls",
            evidence: "Dockerfile:1 — FROM rust:latest; the toolchain changes under the build without a commit",
            confidence: 0.95,
        },
    },
    PackRule {
        name: "docker-no-secrets-in-layers",
        category: "security",
        severity: "error",
        effort: "low",
        description: "no secret enters any image layer: not via COPY, ENV, ARG, or a deleted-later file",
        inspection_guide: "1. Check COPY/ADD lines for credential files. 2. Check ENV/ARG for \
            secret values (ARG persists in history). 3. A secret copied then deleted still lives \
            in the layer — violation. 4. Confirm runtime secrets come from the orchestrator.",
        detection_hints: &[
            "grep Dockerfile: ENV, ARG, COPY with .env/keys/pem",
            "red flag: ARG API_KEY used at build time",
        ],
        patterns: &[
            r#"(?i)^\s*(ENV|ARG)\s+.*(API[_-]?KEY|SECRET|TOKEN|PASSWORD)"#,
            r#"(?i)^\s*(COPY|ADD)\s+.*(\.env|id_rsa|\.pem|secret|token)"#,
        ],
        evidence_passing: "Dockerfile — no secret-bearing COPY/ENV/ARG; runtime secrets injected by <mechanism>",
        evidence_failing: "Dockerfile:<line> — <secret> baked into a layer",
        example_passing: Example {
            criterion: "image history contains no credentials",
            evidence: "Dockerfile — COPY limited to target/release/app; env vars set only in compose with runtime injection",
            confidence: 0.9,
        },
        example_failing: Example {
            criterion: "image history contains no credentials",
            evidence: "Dockerfile:8 — COPY .env /app/.env bakes production credentials into the layer",
            confidence: 0.95,
        },
    },
    PackRule {
        name: "docker-minimal-final-stage",
        category: "maintainability",
        severity: "warning",
        effort: "low",
        description: "a multi-stage build keeps compilers, package managers, and sources out of the final image",
        inspection_guide: "1. Confirm the build uses stages. 2. Confirm the final stage copies \
            only runtime artifacts. 3. A final image containing the toolchain or source tree is a \
            violation.",
        detection_hints: &[
            "grep Dockerfile: FROM … AS, COPY --from",
            "red flag: single-stage build shipping cargo/npm and the src tree",
        ],
        patterns: &[],
        evidence_passing: "Dockerfile — final stage copies <artifacts> only from the build stage",
        evidence_failing: "Dockerfile — final image ships <toolchain/sources>",
        example_passing: Example {
            criterion: "the shipped image contains only the runtime artifact",
            evidence: "Dockerfile:14-25 — slim final stage; COPY --from=build target/release/app plus certs, nothing else",
            confidence: 0.92,
        },
        example_failing: Example {
            criterion: "the shipped image contains only the runtime artifact",
            evidence: "Dockerfile — single stage: rustc, cargo registry, and src/ all present in the shipped image",
            confidence: 0.92,
        },
    },
];

/// The canonical seeded body for one pack rule — the single source shared by
/// `seed` (which writes it) and the pack-drift smell (which compares a stored
/// rule against it after a loom upgrade).
pub fn rule_body(pack_name: &str, r: &PackRule) -> serde_json::Value {
    let detection_kind = if r.patterns.is_empty() {
        "llm_judgment"
    } else {
        "pattern"
    };
    serde_json::json!({
        "category": r.category,
        "severity": r.severity,
        "effort": r.effort,
        "pack": pack_name,
        "detection_kind": detection_kind,
        "inspection_guide": r.inspection_guide,
        "detection_hints": r.detection_hints,
        "patterns": r.patterns,
        "evidence_template": { "passing": r.evidence_passing, "failing": r.evidence_failing },
        "passing_example": {
            "criterion": r.example_passing.criterion,
            "evidence": r.example_passing.evidence,
            "confidence": r.example_passing.confidence,
        },
        "failing_example": {
            "criterion": r.example_failing.criterion,
            "evidence": r.example_failing.evidence,
            "confidence": r.example_failing.confidence,
        },
    })
}

/// Seed a pack's rules as asserted `QualityRule` nodes. Idempotent.
pub fn seed(store: &Store, pack_name: &str) -> Result<usize> {
    let rules = pack(pack_name);
    if rules.is_empty() {
        anyhow::bail!(
            "unknown pack '{pack_name}'; available: {}",
            PACKS.join(", ")
        );
    }
    for r in rules {
        let body = rule_body(pack_name, r);
        store.upsert_builtin_node(NodeType::QualityRule, r.name, r.name, r.description, body)?;
    }
    Ok(rules.len())
}

#[cfg(test)]
mod tests {
    use super::{pack, PACKS};
    use regex::Regex;

    #[test]
    fn pack_patterns_are_valid_regexes() {
        for pack_name in PACKS {
            for rule in pack(pack_name) {
                for pattern in rule.patterns {
                    Regex::new(pattern).unwrap_or_else(|err| {
                        panic!(
                            "invalid regex pattern for rule {}: {pattern}: {err}",
                            rule.name
                        )
                    });
                }
            }
        }
    }
}
