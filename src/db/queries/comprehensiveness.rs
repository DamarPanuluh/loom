//! Comprehensiveness — the COVERAGE half of "production ready" (the QUALITY half
//! is `fully_proven`). Does the intent graph CAPTURE everything the code should
//! do? Comprehensiveness can't be a pure count of what's IN the graph; loom ships
//! a CANONICAL rubric of dimensions and the LLM instantiates it per repo. Two of
//! the dimensions are MECHANICALLY ENUMERABLE — anchored to the real code surface
//! so a thin checklist can't under-enumerate — and gate `fully_proven` (G7/G8):
//!
//! - **entrypoint coverage**: every externally-public symbol is grounded /
//!   accepted / adjudicated (reuses symbol_accountability — its `required` is the
//!   honest denominator).
//! - **boundary coverage**: every codefile that statically signals an OUTBOUND
//!   external dependency has an owning intent that declares a boundary.

use crate::db::queries::stats::CoverageAxis;
use crate::db::queries::symbol_accountability::SymbolAccountabilityReport;

/// Entrypoint coverage from an already-computed symbol-accountability report:
/// `required` public symbols minus the still-`actionable_gaps`. Denominator-honest
/// — a public symbol can't leave the denominator by being un-owned.
pub fn entrypoint_coverage(report: &SymbolAccountabilityReport) -> CoverageAxis {
    let required = report.summary.required as i64;
    let gaps = report.summary.actionable_gaps as i64;
    CoverageAxis {
        covered: (required - gaps).max(0),
        total: required,
    }
}

// NOTE: boundary coverage (the other mechanical comprehensiveness dimension)
// needs the RAW external-import surface (reqwest / net/http / …). The graph only
// persists imports RESOLVED to internal file paths — external names are dropped —
// so boundary can't be computed from the snapshot. It lives in `loom complete`'s
// disk scan (re-reading source for import lines), where file I/O is acceptable;
// it is deliberately NOT a hot-path badge gate on data the snapshot lacks.
