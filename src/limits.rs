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
            scope: "validation command runs; journey CLI steps (default)",
            remedy: "per validation: body key `timeout_seconds`; per journey step: `timeout_secs` in the spec",
        },
        Limit {
            name: "http_timeout_secs",
            value: crate::journey::DEFAULT_HTTP_TIMEOUT_SECS,
            unit: "seconds",
            scope: "journey HTTP steps (default)",
            remedy: "per journey step: `timeout_secs` in the spec",
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
            scope: "graph lock acquisition before a named contention error",
            remedy: "retry after the holder exits; the error names read vs write",
        },
        Limit {
            name: "sqlite_busy_timeout_ms",
            value: crate::store::SQLITE_BUSY_TIMEOUT_MS,
            unit: "milliseconds",
            scope: "SQLite statement-level contention inside the store",
            remedy: "fixed",
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
