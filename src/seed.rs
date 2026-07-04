//! The code-domain seed — loom's reference implementation of the engine's seams.
//!
//! Plane: the domain side of the engine/seed boundary. Everything the engine
//! consumes about *code* enters here: tree-sitter extraction and the external
//! scan adapters are the code seed's [`Deriver`]s. The engine (`sync`) never
//! references tree-sitter or file-specific extraction types directly — it loops
//! [`sync_derivers`] and ripples the [`ArtifactChange`]s they report. Adding a
//! second domain would mean a second seed, not a change to the engine.

use crate::deriver::{ArtifactChange, Deriver};
use crate::extract::{extract, Extraction, Role};
use crate::model::{EdgeKind, NodeType, TargetKind, TruthClass};
use crate::store::Store;
use crate::sync::SyncReport;
use crate::thresholds::Thresholds;
use crate::Result;
use std::path::Path;

/// Every registered code-seed deriver. sync orchestrates the ones flagged
/// [`Deriver::runs_on_sync`]; the rest (external adapters) are on-demand.
pub fn derivers() -> Vec<Box<dyn Deriver>> {
    vec![Box::new(StructuralDeriver), Box::new(ScanDeriver)]
}

/// The derivers sync auto-runs each pass (cheap, deterministic, structural).
pub fn sync_derivers() -> Vec<Box<dyn Deriver>> {
    derivers()
        .into_iter()
        .filter(|d| d.runs_on_sync())
        .collect()
}

// ---- structural deriver (tree-sitter extraction) ---------------------------

/// A built-in structural CodeRule and the finding kinds it covers.
struct BuiltinRule {
    key: &'static str,
    name: &'static str,
    category: &'static str,
    description: &'static str,
}

const BUILTIN_RULES: &[BuiltinRule] = &[
    BuiltinRule {
        key: "max-file-size",
        name: "max-file-size",
        category: "size",
        description: "a file should not exceed a maintainable line count",
    },
    BuiltinRule {
        key: "complex-symbol",
        name: "complex-symbol",
        category: "complexity",
        description: "a function should stay below a cognitive complexity threshold",
    },
    BuiltinRule {
        key: "no-panic-marker",
        name: "no-panic-marker",
        category: "safety",
        description: "production source should not panic at a boundary (unwrap/panic!)",
    },
    BuiltinRule {
        key: "large-symbol",
        name: "large-symbol",
        category: "size",
        description: "a function should stay below a maintainable line count",
    },
    BuiltinRule {
        key: "deep-nesting",
        name: "deep-nesting",
        category: "complexity",
        description: "a function should not nest control flow beyond a readable depth",
    },
    BuiltinRule {
        key: "excess-args",
        name: "excess-args",
        category: "design",
        description: "a function should take a bounded number of arguments",
    },
];

/// The tree-sitter extraction deriver: derives a code file's facets (language,
/// role, loc, content hash, symbols, imports) and its structural findings.
pub struct StructuralDeriver;

impl Deriver for StructuralDeriver {
    fn name(&self) -> &str {
        "structural"
    }
    fn runs_on_sync(&self) -> bool {
        true
    }
    fn derive(
        &self,
        store: &Store,
        root: &Path,
        report: &mut SyncReport,
    ) -> Result<Vec<ArtifactChange>> {
        let rules = ensure_builtin_rules(store)?;
        let thresholds = crate::thresholds::load(store)?;
        // Findings are wiped and rebuilt deterministically each run (INV-2), so a
        // threshold change re-derives even when file content is unchanged.
        store.wipe_structural_findings()?;
        let codefiles = store.codefiles()?;
        let mut changes = Vec::new();
        for cf in &codefiles {
            report.files_scanned += 1;
            let path = root.join(&cf.name);
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => {
                    report.missing.push(cf.name.clone());
                    // A registered file that is now gone is a deletion: ripple its
                    // dependents ONCE (the `missing_rippled` once-guard), then mark
                    // it, so a still-missing file never re-resets contracts a
                    // driver re-verified against its absence.
                    let had_hash = store
                        .get_facet(&cf.id, TargetKind::Node, "content_hash")?
                        .is_some();
                    let already_rippled = store
                        .get_facet(&cf.id, TargetKind::Node, "missing_rippled")?
                        .is_some();
                    if had_hash || !already_rippled {
                        if had_hash {
                            report.files_deleted += 1;
                        }
                        changes.push(ArtifactChange {
                            artifact_id: cf.id.clone(),
                            cause: format!("registered codefile {} disappeared", cf.name),
                            content: None,
                        });
                        if had_hash {
                            store.clear_facet(&cf.id, TargetKind::Node, "content_hash")?;
                        }
                        store.set_facet(
                            &cf.id,
                            TargetKind::Node,
                            "missing_rippled",
                            "true",
                            TruthClass::Derived,
                        )?;
                    }
                    continue;
                }
            };
            let ex = extract(&cf.name, &content);
            // The file exists (again): a future disappearance is a fresh deletion.
            store.clear_facet(&cf.id, TargetKind::Node, "missing_rippled")?;
            let prior = store.get_facet(&cf.id, TargetKind::Node, "content_hash")?;
            if prior.as_deref() != Some(ex.content_hash.as_str()) {
                write_facets(store, &cf.id, &ex)?;
                // Ripple only on a REAL change (prior hash existed and differs).
                if prior.is_some() {
                    report.files_changed += 1;
                    changes.push(ArtifactChange {
                        artifact_id: cf.id.clone(),
                        cause: format!("content hash of {} changed", cf.name),
                        content: Some(content.clone()),
                    });
                }
            }
            for f in derive_findings(&cf.name, &ex, &thresholds) {
                let rule_id = match rules.get(f.rule) {
                    Some(id) => id,
                    None => continue,
                };
                let det_key = format!("{}:{}:{}", f.kind, cf.name, f.symbol);
                let node = store.add_derived_node(
                    NodeType::Finding,
                    &det_key,
                    &f.title,
                    &f.detail,
                    f.kind,
                    serde_json::json!({ "kind": f.kind, "symbol": f.symbol }),
                )?;
                store.add_derived_edge(EdgeKind::Flags, &node.id, &cf.id)?;
                store.add_derived_edge(EdgeKind::Assesses, &node.id, rule_id)?;
                report.findings += 1;
            }
        }
        Ok(changes)
    }
}

fn ensure_builtin_rules(store: &Store) -> Result<std::collections::HashMap<&'static str, String>> {
    let mut map = std::collections::HashMap::new();
    for r in BUILTIN_RULES {
        let node = store.upsert_builtin_node(
            NodeType::CodeRule,
            r.key,
            r.name,
            r.description,
            serde_json::json!({ "category": r.category }),
        )?;
        map.insert(r.key, node.id);
    }
    Ok(map)
}

fn write_facets(store: &Store, cf_id: &str, ex: &Extraction) -> Result<()> {
    let d = TruthClass::Derived;
    store.set_facet(cf_id, TargetKind::Node, "language", ex.language.as_str(), d)?;
    store.set_facet(cf_id, TargetKind::Node, "role", ex.role.as_str(), d)?;
    store.set_facet(cf_id, TargetKind::Node, "loc", &ex.loc.to_string(), d)?;
    store.set_facet(cf_id, TargetKind::Node, "content_hash", &ex.content_hash, d)?;
    store.set_facet(
        cf_id,
        TargetKind::Node,
        "symbol_count",
        &ex.symbols.len().to_string(),
        d,
    )?;
    let imports = serde_json::to_string(&ex.imports).unwrap_or_else(|_| "[]".into());
    store.set_facet(cf_id, TargetKind::Node, "imports", &imports, d)?;
    Ok(())
}

/// A derived finding descriptor (pre-persistence).
struct FindingDesc {
    kind: &'static str,
    symbol: String,
    title: String,
    detail: String,
    rule: &'static str,
}

/// Derive the built-in structural findings of one file. Symbol-level detectors
/// apply to production callables only (functions/methods in `Role::Source`);
/// every gate is a strict `>` on the configured [`Thresholds`].
fn derive_findings(path: &str, ex: &Extraction, t: &Thresholds) -> Vec<FindingDesc> {
    let mut out = Vec::new();
    // oversized_file — language-agnostic, loc based.
    if ex.loc > t.max_file_loc {
        out.push(FindingDesc {
            kind: "oversized_file",
            symbol: String::new(),
            title: format!("{path} is oversized"),
            detail: format!("{} lines (> {})", ex.loc, t.max_file_loc),
            rule: "max-file-size",
        });
    }
    // Per-callable detectors (source files only).
    if ex.role == Role::Source {
        for s in &ex.symbols {
            if !matches!(s.kind.as_str(), "function" | "method") {
                continue;
            }
            if s.complexity > t.max_symbol_complexity {
                out.push(FindingDesc {
                    kind: "complex_symbol",
                    symbol: s.name.clone(),
                    title: format!("{}::{} is complex", path, s.name),
                    detail: format!(
                        "complexity ~{} (> {})",
                        s.complexity, t.max_symbol_complexity
                    ),
                    rule: "complex-symbol",
                });
            }
            let sym_loc = s.line_end.saturating_sub(s.line_start) + 1;
            if sym_loc > t.max_symbol_loc {
                out.push(FindingDesc {
                    kind: "large_symbol",
                    symbol: s.name.clone(),
                    title: format!("{}::{} is long", path, s.name),
                    detail: format!("{} lines (> {})", sym_loc, t.max_symbol_loc),
                    rule: "large-symbol",
                });
            }
            if s.max_nesting > t.max_nesting {
                out.push(FindingDesc {
                    kind: "deep_nesting",
                    symbol: s.name.clone(),
                    title: format!("{}::{} nests deeply", path, s.name),
                    detail: format!("nesting depth {} (> {})", s.max_nesting, t.max_nesting),
                    rule: "deep-nesting",
                });
            }
            if s.arg_count > t.max_args {
                out.push(FindingDesc {
                    kind: "excess_args",
                    symbol: s.name.clone(),
                    title: format!("{}::{} takes many arguments", path, s.name),
                    detail: format!("{} args (> {})", s.arg_count, t.max_args),
                    rule: "excess-args",
                });
            }
        }
    }
    // panic_marker — production unwrap()/panic! sites (AST-counted; excludes test
    // modules and string/comment text).
    if ex.role == Role::Source && ex.panic_sites > 0 {
        out.push(FindingDesc {
            kind: "panic_marker",
            symbol: String::new(),
            title: format!("{path} panics at a boundary"),
            detail: format!("{} unwrap()/panic! site(s)", ex.panic_sites),
            rule: "no-panic-marker",
        });
    }
    out
}

// ---- scan deriver (external diagnostic adapters) ---------------------------

/// The external-adapter deriver: runs the registered scan commands (linters,
/// type-checkers) whose diagnostics become derived findings. On-demand (linters
/// are expensive), so sync does not auto-run it — `loom scan run` does.
pub struct ScanDeriver;

impl Deriver for ScanDeriver {
    fn name(&self) -> &str {
        "scan"
    }
    fn runs_on_sync(&self) -> bool {
        false
    }
    fn derive(
        &self,
        store: &Store,
        root: &Path,
        report: &mut SyncReport,
    ) -> Result<Vec<ArtifactChange>> {
        let scan = crate::scan::run(store, root, None)?;
        report.findings += scan.new_findings;
        // Diagnostics attach to files by path but do not themselves re-open
        // asserted claims — sync's structural deriver owns content-change ripple.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_and_scan_are_both_registered_derivers() {
        let names: Vec<_> = derivers().iter().map(|d| d.name().to_string()).collect();
        assert!(names.contains(&"structural".to_string()));
        assert!(names.contains(&"scan".to_string()));
    }

    #[test]
    fn sync_runs_structural_but_not_the_external_scan() {
        let sync: Vec<_> = sync_derivers()
            .iter()
            .map(|d| d.name().to_string())
            .collect();
        assert_eq!(sync, vec!["structural".to_string()]);
        // The scan adapter conforms to the deriver contract but is on-demand.
        assert!(!ScanDeriver.runs_on_sync());
        assert!(StructuralDeriver.runs_on_sync());
    }
}
