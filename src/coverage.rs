//! Coverage truth — the read-only definition of code ownership gaps.
//!
//! Plane: computed-on-read over registered CodeFiles and asserted grounding.
//! This module owns the predicate shared by diagnostics, maturity, and work
//! routing so those projections cannot disagree about whether a file is owned.
//! One intent may realize in many files (sibling slices). Only `realizes`
//! owns a file; `consumes` / `configures` / `verifies` never close coverage.

use crate::extract::Role;
use crate::model::{EdgeKind, GroundingRole, Node};
use crate::store::Store;
use crate::Result;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct IgnoreRule {
    pub glob: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CoverageScopeSummary {
    /// Every registered CodeFile, before exclusions or observed classification.
    pub total_registered: usize,
    /// Files carrying an ownership obligation (`owned + unowned`).
    pub in_scope: usize,
    pub owned: usize,
    pub unowned_files: Vec<String>,
    pub observed: usize,
    pub excluded_files: Vec<String>,
    /// Excluded registered files grouped by the recorded reason that excluded
    /// them. A file matching multiple rules is attributed to the first rule,
    /// while [`matching_ignore_rules`] still exposes every matching precedent.
    pub exclusions_by_reason: BTreeMap<String, usize>,
}

impl CoverageScopeSummary {
    pub fn unowned(&self) -> usize {
        self.unowned_files.len()
    }

    pub fn excluded(&self) -> usize {
        self.excluded_files.len()
    }
}

pub fn ignore_rules(store: &Store) -> Result<Vec<IgnoreRule>> {
    let Some(raw) = store.get_meta("ignores")? else {
        return Ok(Vec::new());
    };
    let rows: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let glob = row.get("glob")?.as_str()?.to_string();
            let reason = row
                .get("reason")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            Some(IgnoreRule { glob, reason })
        })
        .collect())
}

/// Every recorded exclusion rule matching `path`, in declaration order.
pub fn matching_ignore_rules(store: &Store, path: &str) -> Result<Vec<IgnoreRule>> {
    let mut matches = Vec::new();
    for rule in ignore_rules(store)? {
        if crate::fsglob::matcher([&rule.glob])?.is_match(path) {
            matches.push(rule);
        }
    }
    Ok(matches)
}

/// Whether a CodeFile is registered as observed: monitored upstream code that
/// stays in the sync/surface/contract plane but carries no ownership, coverage,
/// or build obligations. The per-file counterpart of the graph-level observed
/// mode. Asserted at registration (`codefile add --observed`), never touched by
/// sync — derivers write facets, not the body.
pub(crate) fn codefile_observed(n: &Node) -> bool {
    n.body
        .get("observed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Registered CodeFiles with no owning `implements` edge, not matched by a
/// coverage-exclusion glob (`loom ignore`), and not registered as observed.
/// This is the single definition of the coverage gap: the diagnostic, the
/// `realized` maturity gate, and the `coverage` work queue all read it, so they
/// can never disagree. Sorted by name for a stable next-item.
pub(crate) fn unowned_codefiles(store: &Store) -> Result<Vec<Node>> {
    let ignore = crate::fsglob::matcher(store.ignore_globs()?)?;
    let mut unowned = Vec::new();
    for cf in store.codefiles()? {
        if ignore.is_match(&cf.name) {
            continue; // deliberately outside the tracked surface
        }
        if codefile_observed(&cf) {
            continue; // monitored upstream — no ownership obligation
        }
        // A TEST file is never realized by a behavior — it verifies one, and
        // demanding a realizing owner for it would mean `tests/` could only be
        // registered by permanently reddening coverage. That is exactly why
        // 22.8k lines of this repo's evidence backbone stayed outside the graph
        // while coverage reported 67/67 owned.
        if Role::detect(&cf.name) == Role::Test {
            let mut verified = false;
            for e in store.edges_with(Some(EdgeKind::Implements), None, Some(&cf.id))? {
                if !store.edge_superseded(&e.id)?
                    && store.grounding_role(&e.id)? == GroundingRole::Verifies
                {
                    verified = true;
                    break;
                }
            }
            if !verified {
                unowned.push(cf);
            }
            continue;
        }
        if store.realizing_implementers(&cf.id)?.is_empty() {
            unowned.push(cf);
        }
    }
    unowned.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(unowned)
}

/// Names of all CodeFiles currently in the coverage gap.
pub fn unowned_names(store: &Store) -> Result<Vec<String>> {
    Ok(unowned_codefiles(store)?
        .into_iter()
        .map(|n| n.name)
        .collect())
}

/// `(registered, owned, unowned_names, observed)` after coverage exclusions.
/// Ignored files are dropped from every bucket and observed files count only in
/// the `observed` bucket, so `registered == owned + unowned`.
///
/// Counts from [`unowned_codefiles`] rather than re-deriving the rule. It used
/// to carry its own copy, which meant the "single definition of the coverage
/// gap" was two definitions that agreed only by coincidence — and when the test
/// file rule landed in one of them, `loom coverage` and the `covered` rung
/// disagreed about the same files.
pub fn code_ownership_summary(store: &Store) -> Result<(usize, usize, Vec<String>, usize)> {
    let summary = coverage_scope_summary(store)?;
    Ok((
        summary.in_scope,
        summary.owned,
        summary.unowned_files,
        summary.observed,
    ))
}

/// Full coverage denominator, including the files excluded from ownership.
/// This is the status/reporting counterpart to [`unowned_codefiles`]: the gate
/// still evaluates the explicitly tracked surface, while the projection makes
/// the size and rationale of every excluded surface impossible to mistake for
/// behavioral ownership.
pub fn coverage_scope_summary(store: &Store) -> Result<CoverageScopeSummary> {
    let rules = ignore_rules(store)?;
    let compiled: Vec<_> = rules
        .iter()
        .map(|rule| crate::fsglob::matcher([&rule.glob]))
        .collect::<Result<_>>()?;
    let unowned_files: Vec<String> = unowned_codefiles(store)?
        .into_iter()
        .map(|node| node.name)
        .collect();
    let unowned_set: HashSet<&str> = unowned_files.iter().map(String::as_str).collect();
    let codefiles = store.codefiles()?;
    let mut owned = 0usize;
    let mut observed = 0usize;
    let mut excluded_files = Vec::new();
    let mut exclusions_by_reason = BTreeMap::new();

    for codefile in &codefiles {
        if let Some(index) = compiled
            .iter()
            .position(|matcher| matcher.is_match(&codefile.name))
        {
            excluded_files.push(codefile.name.clone());
            let reason = if rules[index].reason.trim().is_empty() {
                "(no reason recorded)"
            } else {
                rules[index].reason.as_str()
            };
            *exclusions_by_reason.entry(reason.to_string()).or_insert(0) += 1;
        } else if codefile_observed(codefile) {
            observed += 1;
        } else if !unowned_set.contains(codefile.name.as_str()) {
            owned += 1;
        }
    }
    excluded_files.sort();

    Ok(CoverageScopeSummary {
        total_registered: codefiles.len(),
        in_scope: owned + unowned_files.len(),
        owned,
        unowned_files,
        observed,
        excluded_files,
        exclusions_by_reason,
    })
}
