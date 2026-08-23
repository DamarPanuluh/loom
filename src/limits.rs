//! Enforced limits — named at violation time, discoverable in one place.
//!
//! Plane: operability. When the runner kills a step or a write is refused for
//! size, the surfaced error must say WHICH limit fired and at what value —
//! a worker must never bisect for a cause the runner knew. This registry is
//! the single list `loom limits` renders; every entry references the constant
//! its enforcing module uses, so the list cannot drift from enforcement.

/// One enforced limit.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Limit {
    /// Stable machine name, printed at violation time as `name=value`.
    pub name: &'static str,
    pub value: u64,
    pub unit: &'static str,
    /// Where the limit is enforced.
    pub scope: &'static str,
    /// How a worker changes the outcome, or "fixed".
    pub remedy: &'static str,
}

/// Every limit loom enforces, in stable order.
pub fn all() -> Vec<Limit> {
    vec![
        Limit {
            name: "timeout_secs",
            value: crate::runner::DEFAULT_TIMEOUT_SECS,
            unit: "seconds",
            scope: "validation command runs",
            remedy: "per validation: body key `timeout_seconds`",
        },
        Limit {
            name: "observe_timeout_secs",
            value: crate::runner::DEFAULT_OBSERVE_TIMEOUT_SECS,
            unit: "seconds",
            scope: "commands run through `loom observe` and the loom_observe MCP tool",
            remedy: "raise it for this run: `--timeout` / the `timeout` argument",
        },
        Limit {
            name: "max_impact_depth",
            value: crate::callgraph::MAX_IMPACT_DEPTH as u64,
            unit: "call hops",
            scope: "impact walks from `loom impact` and the loom_impact MCP tool",
            remedy: "fixed — a wider walk reports most of the crate, not a blast radius",
        },
        Limit {
            name: "journey_timeout_secs",
            value: crate::journey::DEFAULT_JOURNEY_TIMEOUT_SECONDS,
            unit: "seconds",
            scope: "one journey step, when the spec does not set its own",
            remedy: "per step: spec key `timeout_seconds`",
        },
        Limit {
            name: "max_argv_tokens",
            value: crate::candidate_surface_policy::MAX_ARGV_TOKENS as u64,
            unit: "tokens",
            scope: "argv of a candidate surface operation",
            remedy: "fixed — an argv this long is a script, and belongs in a file the operation invokes",
        },
        Limit {
            name: "max_argv_bytes",
            value: crate::candidate_surface_policy::MAX_ARGV_BYTES as u64,
            unit: "bytes",
            scope: "argv of a candidate surface operation",
            remedy: "fixed — pass large input through a file, not the command line",
        },
        Limit {
            name: "max_operation_nesting",
            value: crate::candidate_surface_policy::MAX_NESTING as u64,
            unit: "levels",
            scope: "nesting depth of a candidate surface operation's structured value",
            remedy: "fixed — flatten the value; deeper shapes cannot be reviewed as one surface",
        },
        Limit {
            name: "max_adversarial_review_frontier",
            value: crate::policy::MAX_ADVERSARIAL_REVIEW_FRONTIER as u64,
            unit: "claims",
            scope: "configured `adversarial_review_frontier` in the evidence policy",
            remedy: "fixed ceiling — set a lower frontier with `loom policy`; 0 disables it",
        },
        Limit {
            name: "validation_excerpt_bytes",
            value: crate::proof::VALIDATION_OUTPUT_EXCERPT_BYTES as u64,
            unit: "bytes",
            scope: "human-facing excerpt of a validation command's output",
            remedy: "fixed — the full hash always covers the whole stream",
        },
        Limit {
            name: "journey_stream_excerpt_bytes",
            value: crate::journey_runtime::STREAM_EXCERPT_BYTES as u64,
            unit: "bytes",
            scope: "captured stdout/stderr retained per journey step",
            remedy: "fixed — assert on the step's output instead of reading it all back",
        },
        Limit {
            name: "journey_failure_diagnostic_bytes",
            value: crate::journey_runtime::FAILURE_DIAGNOSTIC_BYTES as u64,
            unit: "bytes",
            scope: "diagnostic text kept on a failed journey step (head+tail, middle disclosed)",
            remedy: "fixed — both ends are kept, so the runner verdict survives",
        },
        Limit {
            name: "release_diagnostic_bytes",
            value: crate::release::RELEASE_DIAGNOSTIC_BYTES as u64,
            unit: "bytes",
            scope: "diagnostic text kept on a failed release gate (head+tail, middle disclosed)",
            remedy: "fixed — both ends are kept",
        },
        Limit {
            name: "git_output_cap",
            value: crate::checkpoint::GIT_OUTPUT_CAP as u64,
            unit: "bytes",
            scope: "stdout accepted from a git invocation during checkpoint inspection",
            remedy: "fixed — narrow the checkpoint's scope; past this loom refuses rather than truncating silently",
        },
        Limit {
            name: "finding_title_chars",
            value: crate::scan::TITLE_MSG_LIMIT as u64,
            unit: "characters",
            scope: "diagnostic message inside a derived finding's title",
            remedy: "fixed — the full message stays on the finding body",
        },
        Limit {
            name: "max_packet_notes",
            value: crate::workitem::NOTE_CAP as u64,
            unit: "notes",
            scope: "notes carried on one context packet",
            remedy: "fixed — the remainder is counted, not dropped silently",
        },
        Limit {
            name: "scan_timeout_secs",
            value: crate::scan::SCAN_TIMEOUT_SECS,
            unit: "seconds",
            scope: "external diagnostic adapter runs",
            remedy: "fixed — keep adapter output bounded and fast",
        },
        Limit {
            name: "keep_bytes",
            value: crate::subprocess::KEEP_BYTES as u64,
            unit: "bytes",
            scope: "captured stdout/stderr per stream per end (head+tail kept, middle omitted and counted)",
            remedy: "fixed — read the true total in the JSON record",
        },
        Limit {
            name: "excerpt_bytes",
            value: crate::runner::EXCERPT_BYTES as u64,
            unit: "bytes",
            scope: "human-facing stream excerpts on run evidence",
            remedy: "fixed — the full hash always covers the whole stream",
        },
        Limit {
            name: "max_spans",
            value: crate::evidence::MAX_SPANS as u64,
            unit: "citations",
            scope: "distinct file citations per verdict",
            remedy: "reduce evidence to the most decision-relevant citations",
        },
        Limit {
            name: "max_search_lines",
            value: crate::evidence::MAX_SEARCH_LINES as u64,
            unit: "lines",
            scope: "verbatim window search when re-anchoring a moved span",
            remedy: "fixed — the exact-position and symbol-identity checks still run on larger files",
        },
        Limit {
            name: "prescreen_hits",
            value: crate::runner::PRESCREEN_HIT_CAP as u64,
            unit: "hits",
            scope: "quality-rule pattern pre-screen per rule",
            remedy: "narrow the rule's patterns or the intent's grounding set",
        },
        Limit {
            name: "max_guidance_items",
            value: crate::pattern::MAX_GUIDANCE_ITEMS as u64,
            unit: "items",
            scope: "pattern guidance items per packet",
            remedy: "fixed",
        },
        Limit {
            name: "max_guidance_excerpt_bytes",
            value: crate::pattern::MAX_GUIDANCE_EXCERPT_BYTES as u64,
            unit: "bytes",
            scope: "pattern guidance excerpt size",
            remedy: "fixed",
        },
        Limit {
            name: "lock_wait_ms",
            value: crate::store::LOCK_WAIT_BUDGET_MS,
            unit: "milliseconds",
            scope: "exclusive graph lock acquisition before a named contention error",
            remedy: "retry after the holder exits; the error names read vs write",
        },
        Limit {
            name: "read_lock_wait_ms",
            value: crate::store::READ_LOCK_WAIT_BUDGET_MS,
            unit: "milliseconds",
            scope: "shared graph lock acquisition by read-only commands before a named contention error",
            remedy: "wait for the writer to finish; the error names its pid and command",
        },
        Limit {
            name: "sqlite_busy_timeout_ms",
            value: crate::store::SQLITE_BUSY_TIMEOUT_MS,
            unit: "milliseconds",
            scope: "SQLite statement-level contention inside the store",
            remedy: "fixed",
        },
        Limit {
            name: "role_lease_ttl_ms",
            value: crate::rolelease::ROLE_LEASE_TTL_MS,
            unit: "milliseconds",
            scope: "advisory role-lease freshness — a lease not refreshed within this window reads as stale in `loom role list` / session / status",
            remedy: "any loom command run under the claimed role+profile refreshes it; take over an expired lease with `loom role claim <role> --take-stale`",
        },
        Limit {
            name: "git_timeout_secs",
            value: crate::signal::GIT_TIMEOUT_SECS,
            unit: "seconds",
            scope: "git history sampling for the advisory debt signal",
            remedy: "advisory signal — never required work",
        },
        Limit {
            name: "co_change_max_commits",
            value: crate::signal::CO_CHANGE_MAX_COMMITS as u64,
            unit: "commits",
            scope: "git history depth for the advisory co-change signal",
            remedy: "advisory signal — never required work",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_values_nonzero() {
        let limits = all();
        let mut names: Vec<&str> = limits.iter().map(|l| l.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), limits.len(), "duplicate limit names");
        for l in &limits {
            assert!(
                l.value > 0,
                "{}: a zero limit is a ban, not a limit",
                l.name
            );
            assert!(
                !l.scope.is_empty() && !l.remedy.is_empty(),
                "{}: undocumented",
                l.name
            );
        }
    }
}
