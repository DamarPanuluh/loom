//! Quality rule packs — pre-authored, enriched `QualityRule` definitions.
//!
//! Plane: data. A pack rule ships with all the guidance fields that make LLM
//! verdicts consistent across sessions: an inspection guide, detection hints
//! (LLM-facing prose), an evidence template, and few-shot pass/fail examples.
//! Seeding upserts these as asserted `QualityRule` nodes with stable ids.

use crate::model::NodeType;
use crate::store::Store;
use crate::Result;

/// An enriched pack rule. `detection_kind` is `llm_judgment` for all current
/// rules (pattern pre-screening + machine `patterns[]` land with the ring-6
/// finding integration).
pub struct PackRule {
    pub name: &'static str,
    pub category: &'static str,
    pub severity: &'static str,
    pub effort: &'static str,
    pub description: &'static str,
    pub inspection_guide: &'static str,
    pub detection_hints: &'static [&'static str],
    pub evidence_passing: &'static str,
    pub evidence_failing: &'static str,
}

/// Names of all seedable packs (for `loom detect` / help / errors).
pub const PACKS: &[&str] = &["iso5055"];

/// The rules for a named pack, or empty if unknown.
pub fn pack(name: &str) -> &'static [PackRule] {
    match name {
        "iso5055" => ISO5055,
        _ => &[],
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
        evidence_passing: "src/<file>:<lines> — failures propagated via ? or handled explicitly",
        evidence_failing: "src/<file>:<line> — <call> discards its failure path",
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
        evidence_passing: "src/<file> — secrets read from env/secret store, none in source",
        evidence_failing: "src/<file>:<line> — literal secret in source",
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
        evidence_passing: "src/<file>:<lines> — parameterized queries / escaped boundaries",
        evidence_failing: "src/<file>:<line> — untrusted input concatenated into a query",
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
        evidence_passing: "src/<file>:<lines> — resources held by RAII guards, released on all paths",
        evidence_failing: "src/<file>:<line> — resource not released on the error path",
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
        evidence_passing: "src/<file> — no dead code; shared logic factored",
        evidence_failing: "src/<file> — dead/duplicated code at <lines>",
    },
];

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
        let hints = serde_json::to_value(r.detection_hints)?;
        let body = serde_json::json!({
            "category": r.category,
            "severity": r.severity,
            "effort": r.effort,
            "pack": pack_name,
            "detection_kind": "llm_judgment",
            "inspection_guide": r.inspection_guide,
            "detection_hints": hints,
            "evidence_template": { "passing": r.evidence_passing, "failing": r.evidence_failing },
        });
        store.upsert_builtin_node(NodeType::QualityRule, r.name, r.name, r.description, body)?;
    }
    Ok(rules.len())
}
