//! Small, dependency-free statistical utilities.

/// Returns the linearly interpolated `q` quantile of `samples`.
///
/// Samples are sorted in place using [`f64::total_cmp`]. An empty sample
/// returns `0.0`; `q = 0.0` and `q = 1.0` return the minimum and maximum,
/// respectively.
pub(crate) fn quantile(samples: &mut [f64], q: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(f64::total_cmp);
    let pos = q * (samples.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        samples[lo]
    } else {
        samples[lo] + (pos - lo as f64) * (samples[hi] - samples[lo])
    }
}

#[cfg(test)]
mod tests {
    use super::quantile;

    #[test]
    fn empty_sample_returns_zero() {
        assert_eq!(quantile(&mut [], 0.5), 0.0);
    }

    #[test]
    fn single_value_is_every_quantile() {
        let mut sample = [7.5];
        assert_eq!(quantile(&mut sample, 0.25), 7.5);
    }

    #[test]
    fn interpolates_between_adjacent_values() {
        let mut sample = [8.0, 1.0, 4.0, 2.0];
        assert_eq!(quantile(&mut sample, 0.25), 1.75);
    }

    #[test]
    fn bounds_return_minimum_and_maximum() {
        let mut sample = [8.0, 2.0, 4.0];
        assert_eq!(quantile(&mut sample, 0.0), 2.0);
        assert_eq!(quantile(&mut sample, 1.0), 8.0);
    }
}
