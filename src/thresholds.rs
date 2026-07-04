//! Structural finding thresholds — portable repo config for sync's built-in
//! detectors, with `loom calibrate` deriving repo-fitted values.
//!
//! Plane: configuration. Thresholds travel in the export (portable meta, same
//! as layer order and scan adapters), so a repo's tuned gates survive
//! clone/import. An absent key means the shipped defaults; a present key is
//! parsed strictly — a typo must fail loudly, never silently re-default.

use crate::extract::{extract, Role};
use crate::store::Store;
use crate::Result;
use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Meta key carrying the JSON-encoded thresholds (allowlisted in
/// `PORTABLE_META_KEYS`).
pub const THRESHOLDS_META_KEY: &str = "thresholds";

/// Detector gates for sync's built-in structural findings. Every field is a
/// strict `>` bound: a value AT the threshold does not fire. Serde default +
/// deny_unknown_fields: partial configs fill from defaults, misspelled keys
/// fail loudly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Thresholds {
    /// `oversized_file`: lines per file.
    pub max_file_loc: usize,
    /// `complex_symbol`: complexity proxy per callable.
    pub max_symbol_complexity: u32,
    /// `large_symbol`: lines per callable.
    pub max_symbol_loc: usize,
    /// `deep_nesting`: branch-nesting depth per callable.
    pub max_nesting: u32,
    /// `excess_args`: declared arguments per callable (receiver excluded).
    pub max_args: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_file_loc: 600,
            max_symbol_complexity: 20,
            max_symbol_loc: 120,
            max_nesting: 5,
            max_args: 6,
        }
    }
}

/// Read the configured thresholds; absent meta means the shipped defaults.
pub fn load(store: &Store) -> Result<Thresholds> {
    match store.get_meta(THRESHOLDS_META_KEY)? {
        None => Ok(Thresholds::default()),
        Some(raw) => serde_json::from_str(&raw)
            .map_err(|e| anyhow!("invalid '{THRESHOLDS_META_KEY}' config: {e}")),
    }
}

/// Persist thresholds as portable meta.
pub fn save(store: &Store, t: &Thresholds) -> Result<()> {
    store.set_meta(THRESHOLDS_META_KEY, &serde_json::to_string(t)?)
}

/// A calibration proposal: current gates, repo-fitted values, sample sizes.
#[derive(Debug, Clone, Serialize)]
pub struct Calibration {
    pub current: Thresholds,
    pub proposed: Thresholds,
    pub files_sampled: usize,
    pub symbols_sampled: usize,
}

/// Floors below which calibration never tightens a gate — a small clean repo
/// gets strict-but-livable thresholds, not zero-tolerance churn.
const MIN_FILE_LOC: f64 = 200.0;
const MIN_SYMBOL_COMPLEXITY: f64 = 10.0;
const MIN_SYMBOL_LOC: f64 = 60.0;
const MIN_NESTING: f64 = 4.0;
const MIN_ARGS: f64 = 5.0;

/// Quantile of the repo's own distribution proposed as the gate: detectors
/// flag roughly the worst 5% tail of today's code, so calibration baselines
/// the present without flooding triage.
const CALIBRATION_QUANTILE: f64 = 0.95;

/// Derive repo-fitted thresholds from the registered codefiles on disk. Pure
/// read: persisting the proposal is the caller's explicit `--write` decision.
pub fn calibrate(store: &Store, root: &Path) -> Result<Calibration> {
    let current = load(store)?;
    let mut file_locs = Vec::new();
    let mut complexity = Vec::new();
    let mut sym_locs = Vec::new();
    let mut nesting = Vec::new();
    let mut args = Vec::new();
    for cf in store.codefiles()? {
        let Ok(content) = std::fs::read_to_string(root.join(&cf.name)) else {
            continue;
        };
        let ex = extract(&cf.name, &content);
        file_locs.push(ex.loc as f64);
        if ex.role != Role::Source {
            continue;
        }
        for s in &ex.symbols {
            if !matches!(s.kind.as_str(), "function" | "method") {
                continue;
            }
            complexity.push(f64::from(s.complexity));
            sym_locs.push((s.line_end.saturating_sub(s.line_start) + 1) as f64);
            nesting.push(f64::from(s.max_nesting));
            args.push(f64::from(s.arg_count));
        }
    }
    if file_locs.is_empty() {
        bail!(
            "no registered codefile readable from disk — register files (loom codefile add) before calibrating"
        );
    }
    let symbols_sampled = complexity.len();
    let proposed = Thresholds {
        max_file_loc: fitted(
            &mut file_locs,
            MIN_FILE_LOC,
            10.0,
            current.max_file_loc as f64,
        ) as usize,
        max_symbol_complexity: fitted(
            &mut complexity,
            MIN_SYMBOL_COMPLEXITY,
            1.0,
            f64::from(current.max_symbol_complexity),
        ) as u32,
        max_symbol_loc: fitted(
            &mut sym_locs,
            MIN_SYMBOL_LOC,
            10.0,
            current.max_symbol_loc as f64,
        ) as usize,
        max_nesting: fitted(
            &mut nesting,
            MIN_NESTING,
            1.0,
            f64::from(current.max_nesting),
        ) as u32,
        max_args: fitted(&mut args, MIN_ARGS, 1.0, f64::from(current.max_args)) as u32,
    };
    Ok(Calibration {
        current,
        proposed,
        files_sampled: file_locs.len(),
        symbols_sampled,
    })
}

/// The fitted gate for one metric: the calibration quantile of the sample,
/// rounded up to `step`, clamped to `floor`. No samples (a repo of pure config
/// files has no callables) keeps the current gate — never propose from nothing.
fn fitted(samples: &mut [f64], floor: f64, step: f64, fallback: f64) -> f64 {
    if samples.is_empty() {
        return fallback;
    }
    samples.sort_by(f64::total_cmp);
    let q = crate::signal::quantile(samples, CALIBRATION_QUANTILE);
    ((q / step).ceil() * step).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A unique temp dir that removes itself on drop (same shape as the
    /// integration-test `Tmp` and the store module's own `TmpRoot`).
    struct TmpRoot(PathBuf);

    impl TmpRoot {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let p = std::env::temp_dir().join(format!(
                "loom_thresholds_test_{}_{nanos}_{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&p).unwrap();
            TmpRoot(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TmpRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fresh_store() -> (TmpRoot, Store) {
        let tmp = TmpRoot::new();
        let store = Store::init(tmp.path(), Some("t"), false).unwrap();
        (tmp, store)
    }

    #[test]
    fn load_with_no_meta_returns_default_thresholds() {
        // Contract: an absent thresholds meta key means the shipped defaults —
        // the detector gates a fresh repo sees before anyone calibrates.
        let (_tmp, store) = fresh_store();
        let t = load(&store).expect("load with no meta key yields defaults");
        assert_eq!(
            t,
            Thresholds::default(),
            "absent meta falls back to defaults"
        );
        // Pin the documented default values so a silent change to a shipped
        // default breaks here, not in a downstream test that only counts
        // findings.
        assert_eq!(t.max_file_loc, 600, "default max_file_loc is 600");
        assert_eq!(
            t.max_symbol_complexity, 20,
            "default max_symbol_complexity is 20"
        );
        assert_eq!(t.max_symbol_loc, 120, "default max_symbol_loc is 120");
        assert_eq!(t.max_nesting, 5, "default max_nesting is 5");
        assert_eq!(t.max_args, 6, "default max_args is 6");
    }

    #[test]
    fn save_then_load_round_trips() {
        // Contract: save persists thresholds as portable meta and load reads
        // them back unchanged — the round-trip preserves every gate.
        let (_tmp, store) = fresh_store();
        let t = Thresholds {
            max_file_loc: 333,
            max_symbol_complexity: 7,
            max_symbol_loc: 88,
            max_nesting: 3,
            max_args: 9,
        };
        save(&store, &t).expect("save persists thresholds");
        let back = load(&store).expect("load reads saved thresholds");
        assert_eq!(back, t, "save->load round-trips every field");
    }

    #[test]
    fn partial_json_fills_missing_fields_from_defaults() {
        // Contract: serde(default, deny_unknown_fields) means a partial config
        // fills from defaults — a repo tuning one gate keeps the rest.
        let (_tmp, store) = fresh_store();
        store
            .set_meta(THRESHOLDS_META_KEY, r#"{"max_args":3}"#)
            .unwrap();
        let t = load(&store).expect("partial JSON parses");
        assert_eq!(t.max_args, 3, "the overridden gate takes the given value");
        assert_eq!(t.max_file_loc, 600, "missing gate keeps its default");
        assert_eq!(
            t.max_symbol_complexity, 20,
            "missing gate keeps its default"
        );
        assert_eq!(t.max_symbol_loc, 120, "missing gate keeps its default");
        assert_eq!(t.max_nesting, 5, "missing gate keeps its default");
    }

    #[test]
    fn unknown_field_errors_loudly() {
        // Contract: deny_unknown_fields rejects a typo instead of silently
        // re-defaulting — a misspelled gate must fail loudly, never slip past.
        let (_tmp, store) = fresh_store();
        store
            .set_meta(THRESHOLDS_META_KEY, r#"{"max_argz":3}"#)
            .unwrap();
        let err = load(&store).expect_err("an unknown field must error, not re-default");
        let msg = format!("{err}");
        assert!(
            msg.contains("max_argz") || msg.contains("unknown field"),
            "error surfaces the offending field: {msg}"
        );
    }
}
