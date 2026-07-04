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
    /// `tangled_file`: realizing owners per file (graph-shape gate). Hand-set
    /// only — not part of the `calibrate` fit (owner counts are too small a
    /// distribution for a stable quantile).
    pub max_file_owners: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            max_file_loc: 600,
            max_symbol_complexity: 20,
            max_symbol_loc: 120,
            max_nesting: 5,
            max_args: 6,
            max_file_owners: 2,
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

/// Reset all thresholds by dropping the config, so a later change to the
/// shipped defaults still takes effect (absence = defaults — never a pinned
/// snapshot of today's values).
pub fn clear(store: &Store) -> Result<()> {
    store.remove_meta(THRESHOLDS_META_KEY)
}

/// Canonical gate names — the `config.thresholds` JSON keys, in display order.
pub const GATES: &[&str] = &[
    "max_file_loc",
    "max_symbol_complexity",
    "max_symbol_loc",
    "max_nesting",
    "max_args",
    "max_file_owners",
];

impl Thresholds {
    /// Current `(gate, value)` pairs in [`GATES`] order — for display and JSON.
    pub fn pairs(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("max_file_loc", self.max_file_loc as u64),
            (
                "max_symbol_complexity",
                u64::from(self.max_symbol_complexity),
            ),
            ("max_symbol_loc", self.max_symbol_loc as u64),
            ("max_nesting", u64::from(self.max_nesting)),
            ("max_args", u64::from(self.max_args)),
            ("max_file_owners", self.max_file_owners as u64),
        ]
    }

    /// Set one gate by its canonical name; errors on an unknown gate or a value
    /// too large for the gate's type (user input, so never a silent truncation).
    pub fn set_gate(&mut self, gate: &str, value: u64) -> Result<()> {
        let too_big = || anyhow!("value {value} is too large for gate '{gate}'");
        let as_u32 = || u32::try_from(value).map_err(|_| too_big());
        let as_usize = || usize::try_from(value).map_err(|_| too_big());
        match gate {
            "max_file_loc" => self.max_file_loc = as_usize()?,
            "max_symbol_complexity" => self.max_symbol_complexity = as_u32()?,
            "max_symbol_loc" => self.max_symbol_loc = as_usize()?,
            "max_nesting" => self.max_nesting = as_u32()?,
            "max_args" => self.max_args = as_u32()?,
            "max_file_owners" => self.max_file_owners = as_usize()?,
            other => bail!(
                "unknown threshold gate '{other}' — one of: {}",
                GATES.join(", ")
            ),
        }
        Ok(())
    }

    /// Reset one gate to its shipped default; errors on an unknown gate.
    pub fn reset_gate(&mut self, gate: &str) -> Result<()> {
        let d = Self::default();
        match gate {
            "max_file_loc" => self.max_file_loc = d.max_file_loc,
            "max_symbol_complexity" => self.max_symbol_complexity = d.max_symbol_complexity,
            "max_symbol_loc" => self.max_symbol_loc = d.max_symbol_loc,
            "max_nesting" => self.max_nesting = d.max_nesting,
            "max_args" => self.max_args = d.max_args,
            "max_file_owners" => self.max_file_owners = d.max_file_owners,
            other => bail!(
                "unknown threshold gate '{other}' — one of: {}",
                GATES.join(", ")
            ),
        }
        Ok(())
    }
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
        // Not fitted: owner counts are too small a distribution for a stable
        // quantile. Preserve the operator's setting (or the default) verbatim.
        max_file_owners: current.max_file_owners,
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
        assert_eq!(t.max_file_owners, 2, "default max_file_owners is 2");
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
            max_file_owners: 4,
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
    // Contract 1: set_gate on a default Thresholds sets the named gate to the
    // given value AND leaves every other gate at its default — proving the
    // match arm touched exactly one field, not a neighbor by mistake. Covers all
    // 6 gates (both u32 and usize arms) with distinct non-default values; a
    // flipped match arm (e.g. setting max_nesting when handed "max_args") or a
    // dropped arm reddens the whole-struct equality.
    #[test]
    fn set_gate_sets_one_field_and_leaves_the_rest_at_default() {
        let d = Thresholds::default();

        type Case = (&'static str, u64, fn(&Thresholds) -> Thresholds);
        let cases: &[Case] = &[
            ("max_file_loc", 999, |t| Thresholds {
                max_file_loc: 999,
                ..*t
            }),
            ("max_symbol_complexity", 33, |t| Thresholds {
                max_symbol_complexity: 33,
                ..*t
            }),
            ("max_symbol_loc", 250, |t| Thresholds {
                max_symbol_loc: 250,
                ..*t
            }),
            ("max_nesting", 11, |t| Thresholds {
                max_nesting: 11,
                ..*t
            }),
            ("max_args", 8, |t| Thresholds { max_args: 8, ..*t }),
            ("max_file_owners", 7, |t| Thresholds {
                max_file_owners: 7,
                ..*t
            }),
        ];
        for &(gate, value, mutate) in cases {
            let mut t = Thresholds::default();
            t.set_gate(gate, value)
                .unwrap_or_else(|e| panic!("set_gate({gate}, {value}) should be Ok: {e}"));
            assert_eq!(
                t,
                mutate(&d),
                "only {gate} moves; every other gate keeps its default"
            );
        }
    }

    // Contract 2: an unknown gate name errors AND leaves the struct untouched —
    // a typo must never mutate a field or silently succeed. The error must name
    // the offending gate so the operator can correct it.
    #[test]
    fn set_gate_unknown_gate_errors_and_leaves_struct_unchanged() {
        let mut t = Thresholds::default();
        let before = t;
        let err = t
            .set_gate("max_bogus", 5)
            .expect_err("an unknown gate must error, not silently no-op");
        assert_eq!(t, before, "failed set must not mutate any field");
        let msg = format!("{err}");
        assert!(
            msg.contains("max_bogus"),
            "error names the offending gate: {msg}"
        );
    }

    // Contract 3: the per-type checked conversion rejects a value too large for
    // a u32 gate (no silent truncation), while the SAME value fits a 64-bit usize
    // gate on this target — pinning that the bound is per-type, not a blanket
    // rejection of large values. A blanket `value > u32::MAX` guard reddens the
    // usize Ok branch; dropping the `try_from` reddens the u32 Err branch.
    #[test]
    fn set_gate_overflow_is_per_type_not_blanket() {
        let over_u32 = u64::from(u32::MAX) + 1;

        let mut t = Thresholds::default();
        let before = t;
        t.set_gate("max_args", over_u32)
            .expect_err("u32 gate rejects u32::MAX + 1 (no silent truncation)");
        assert_eq!(t, before, "failed overflow set must not mutate any field");

        let mut t = Thresholds::default();
        t.set_gate("max_file_loc", over_u32)
            .expect("on a 64-bit target usize accepts u32::MAX + 1");
        assert_eq!(
            t.max_file_loc, over_u32 as usize,
            "usize gate stores the large value unchanged"
        );
    }

    // Contract 4: reset_gate restores one gate to its shipped default (not zero,
    // not the current value), and errors on an unknown gate without mutating.
    #[test]
    fn reset_gate_restores_default_and_errors_on_unknown() {
        let d = Thresholds::default();

        let mut t = Thresholds { max_args: 99, ..d };
        t.reset_gate("max_args")
            .expect("reset_gate on a known gate");
        assert_eq!(t.max_args, d.max_args, "reset returns the shipped default");
        assert_eq!(t, d, "after reset the struct equals the full default");

        let mut t = Thresholds {
            max_file_loc: 1,
            ..d
        };
        let before = t;
        let err = t
            .reset_gate("max_bogus")
            .expect_err("reset_gate on an unknown gate must error");
        assert_eq!(t, before, "failed reset must not mutate any field");
        let msg = format!("{err}");
        assert!(
            msg.contains("max_bogus"),
            "error names the offending gate: {msg}"
        );
    }

    // Contract 5: pairs() returns exactly the 6 canonical gates in GATES order,
    // and the values reflect the live struct — a mutated gate shows through.
    // A reordered/shortened/lengthened vec, or a stale value, reddens this.
    #[test]
    fn pairs_match_gates_order_and_reflect_mutations() {
        let d = Thresholds::default();
        let pairs = d.pairs();
        let names: Vec<&str> = pairs.iter().map(|(k, _)| *k).collect();
        assert_eq!(names, GATES, "pairs() emits GATES in order");
        assert_eq!(
            pairs.len(),
            GATES.len(),
            "pairs() emits exactly the 6 gates"
        );

        let mut t = d;
        t.set_gate("max_args", 42).unwrap();
        let pairs = t.pairs();
        let args = pairs
            .iter()
            .find(|(k, _)| *k == "max_args")
            .expect("max_args pair present");
        assert_eq!(args.1, 42, "pairs() reflects the mutated value");
        // the other five pairs still carry their defaults
        for (gate, val) in pairs.iter().filter(|(k, _)| *k != "max_args") {
            let expected = match *gate {
                "max_file_loc" => d.max_file_loc as u64,
                "max_symbol_complexity" => u64::from(d.max_symbol_complexity),
                "max_symbol_loc" => d.max_symbol_loc as u64,
                "max_nesting" => u64::from(d.max_nesting),
                "max_file_owners" => d.max_file_owners as u64,
                _ => panic!("unexpected gate in pairs(): {gate}"),
            };
            assert_eq!(*val, expected, "untouched gate {gate} keeps its default");
        }
    }

    // Contract 6: clear() drops the thresholds meta key so load() falls back to
    // the shipped defaults — a true fallback, not a pinned snapshot of the values
    // that were saved. A clear that wrote `default` instead of removing the key
    // would still pass load()==default today but would pin the values forever;
    // this test asserts the KEY is gone (absent = defaults), not just the values.
    #[test]
    fn clear_drops_meta_key_so_load_falls_back_to_default() {
        let (_tmp, store) = fresh_store();
        let non_default = Thresholds {
            max_args: 17,
            ..Thresholds::default()
        };
        save(&store, &non_default).expect("save persists a non-default config");
        let back = load(&store).expect("load reads the saved config");
        assert_eq!(back, non_default, "save then load round-trips before clear");

        clear(&store).expect("clear drops the thresholds meta key");
        // Directly assert the KEY is gone — a clear that wrote `default` JSON
        // instead of removing the key would still pass load()==default but
        // would pin the values forever (a future default change would not take
        // effect). Absence is the contract; this reddens a "write default"
        // regression that the load()==default assertion alone would miss.
        assert!(
            store
                .get_meta(THRESHOLDS_META_KEY)
                .expect("get_meta reads the thresholds key")
                .is_none(),
            "clear removes the thresholds key (absent = defaults), not writes default JSON"
        );
        let after = load(&store).expect("load after clear yields defaults");
        assert_eq!(
            after,
            Thresholds::default(),
            "clear reverts to the shipped defaults via absence"
        );
    }

    // Contract 7: the equality the CLI relies on to decide clear-vs-save —
    // after reset_gate on the one gate that was away from default, the struct
    // equals Thresholds::default(). A reset_gate that set the field to zero (or
    // left it) instead of the shipped default reddens this.
    #[test]
    fn reset_gate_to_default_equals_full_default() {
        let d = Thresholds::default();
        let mut t = Thresholds {
            max_nesting: 99,
            ..d
        };
        t.reset_gate("max_nesting")
            .expect("reset the one non-default gate");
        assert_eq!(t, d, "after reset the struct equals Thresholds::default()");
    }
}
