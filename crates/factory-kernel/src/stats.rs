//! Nearest-rank percentile arithmetic (#193, phase 1, F7). Moved here
//! unchanged from `factory_core::scenario`, whose own doc comment already
//! explained why it was `pub` rather than `pub(crate)`: `factory-core`'s
//! `operations.rs` (L4) and `intake.rs`, and the daemon's cost report
//! (`costs.rs`), all need the same rule as `scenario`'s own forecast
//! percentiles -- one formula, not several. Living in the L0 kernel makes
//! that explicit rather than incidental to `scenario` happening to export
//! it; every one of those callers now names `factory_kernel::nearest_rank`
//! directly rather than going through `scenario` at all.

/// Nearest-rank percentile over an already-sorted, non-empty slice's length:
/// `index = round(p * (len - 1))`, clamped into range. Monotone in `p` --
/// `p * (len - 1)` is monotone in `p`, and rounding preserves monotonicity --
/// which is what keeps p10 <= p50 <= p90 a guarantee rather than a
/// coincidence of the data.
pub fn nearest_rank(len: usize, p: f64) -> usize {
    if len == 0 {
        return 0;
    }
    ((p * (len - 1) as f64).round() as usize).min(len - 1)
}

/// Nearest-rank percentile of `values` (any order), `None` when empty. The
/// one place `operations::percentile` (L4) sorts and indexes; `scenario`'s
/// own wrappers keep their own sorting (one has to place `None` completion
/// weeks last) and call [`nearest_rank`] directly instead.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    Some(sorted[nearest_rank(sorted.len(), p)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_is_zero_for_an_empty_length() {
        assert_eq!(nearest_rank(0, 0.5), 0);
    }

    #[test]
    fn nearest_rank_clamps_into_range_and_is_monotone_in_p() {
        let len = 11;
        let mut last = 0;
        for tenth in 0..=10 {
            let p = tenth as f64 / 10.0;
            let rank = nearest_rank(len, p);
            assert!(rank < len);
            assert!(rank >= last, "must not decrease as p grows");
            last = rank;
        }
    }

    #[test]
    fn percentile_is_none_for_empty_input() {
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn percentile_sorts_before_indexing() {
        let values = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&values, 0.0), Some(1.0));
        assert_eq!(percentile(&values, 1.0), Some(5.0));
        assert_eq!(percentile(&values, 0.5), Some(3.0));
    }
}
