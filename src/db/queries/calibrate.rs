//! Self-calibrating thresholds — derived from a repo's OWN distribution, never a
//! hardcoded magnitude. This is the discipline that lets loom adapt to a repo it
//! never anticipated: a 50-file library and a 50k-file monorepo each get a
//! threshold fitted to THEIR data, because the number is read off the live
//! distribution instead of guessed in advance.
//!
//! The principle: replace every absolute constant ("> 8 owners is a hub") with a
//! statistical OUTLIER of the live distribution ("an outlier in THIS repo's
//! owner-count spread"). When there is no skew, nothing is flagged; when there is,
//! the genuine outliers are — and the derived threshold is DISCLOSED so it stays
//! auditable and overridable, never a hidden magic number.

/// Tukey multipliers as statistical STANCES — universal definitions, NOT
/// repo-specific magnitudes (the legitimate kind of constant). `OUTLIER_K = 1.5` is
/// the textbook outlier fence (used to FLAG a smell — "this file is an ownership /
/// import outlier for this repo"); `FAR_OUTLIER_K = 3.0` is the extreme-outlier
/// fence (used to SUPPRESS — the coupling cap defers only egregious hubs). Because
/// `FAR_OUTLIER_K > OUTLIER_K` over the same distribution, anything the cap defers is
/// always an outlier the flagging smell also catches — the deferral can never escape.
pub const OUTLIER_K: f64 = 1.5;
pub const FAR_OUTLIER_K: f64 = 3.0;

/// Tukey's upper fence — the textbook outlier bound for a distribution: `Q3 + k·IQR`.
/// `k = 1.5` flags outliers; `k = 3.0` flags FAR (extreme) outliers. Self-calibrating
/// and robust to skew: it is quartile-based, so a heavy tail (a few enormous values)
/// does not drag the fence the way a mean/stddev would.
///
/// Returns `None` when there are fewer than 4 values — too few to define quartiles,
/// so no distribution exists and the caller MUST NOT threshold (calibration is
/// impossible → cap nothing, rather than invent a cutoff). Quantiles use linear
/// interpolation (the R-7 / NumPy default) over the sorted values.
pub fn tukey_upper_fence(values: &[usize], k: f64) -> Option<f64> {
    if values.len() < 4 {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let q = |p: f64| -> f64 {
        let idx = p * (v.len() - 1) as f64;
        let lo = idx.floor() as usize;
        let hi = idx.ceil() as usize;
        v[lo] as f64 + (v[hi] as f64 - v[lo] as f64) * (idx - lo as f64)
    };
    let (q1, q3) = (q(0.25), q(0.75));
    Some(q3 + k * (q3 - q1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_points_cannot_calibrate() {
        assert_eq!(tukey_upper_fence(&[1, 2, 3], 3.0), None);
        assert_eq!(tukey_upper_fence(&[], 1.5), None);
    }

    #[test]
    fn a_flat_distribution_flags_nothing() {
        // No skew: Q1 == Q3, IQR == 0, fence == the common value. Nothing exceeds it,
        // so a uniform repo gets NO suppression — the cap fires only on real skew.
        let flat = vec![2usize; 20];
        let fence = tukey_upper_fence(&flat, 3.0).unwrap();
        assert_eq!(fence, 2.0);
        assert!(!flat.iter().any(|&c| c as f64 > fence));
    }

    #[test]
    fn a_skewed_distribution_isolates_the_outlier_cluster() {
        // loom-shaped: a body of small counts + a heavy tail of hub files. The fence
        // lands ABOVE the body and BELOW the hubs — quartile-based, so the giant 79
        // does not inflate it.
        let mut counts = vec![1usize; 64];
        counts.extend(vec![2usize; 20]);
        counts.extend(vec![3usize; 15]);
        counts.extend([11, 12, 13, 30, 34, 79]); // the hub tail
        let fence = tukey_upper_fence(&counts, 3.0).unwrap();
        // Body (<=3) is kept; the hub tail (>=11) is flagged.
        assert!(3.0 <= fence, "fence keeps the body: {fence}");
        assert!(fence < 11.0, "fence flags the hub cluster: {fence}");
        let flagged = counts.iter().filter(|&&c| c as f64 > fence).count();
        assert_eq!(flagged, 6, "exactly the 6 hub files are outliers");
    }

    #[test]
    fn k_3_is_more_conservative_than_k_1_5() {
        // A SPREAD body (non-zero IQR) so the multiplier matters; with a degenerate
        // IQR == 0 both fences collapse to Q3, which is correct but uninteresting.
        let counts: Vec<usize> = (1..=20).collect();
        let outlier = tukey_upper_fence(&counts, 1.5).unwrap();
        let far = tukey_upper_fence(&counts, 3.0).unwrap();
        assert!(
            far > outlier,
            "the far-outlier fence is higher, so it defers fewer: {far} > {outlier}"
        );
    }
}
