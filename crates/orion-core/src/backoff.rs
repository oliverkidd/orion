//! The one retry pace orion's loops share: a wait that starts at a base
//! and doubles with each failure in a row, up to a cap.

use std::time::Duration;

/// How long to hold off after the `fails`-th failure in a row: `base`
/// after the first (and for `0`), doubling each time after, never past
/// `cap`. Saturates rather than overflowing however many `fails`.
pub fn doubling(base: Duration, cap: Duration, fails: u32) -> Duration {
    let doublings = fails.saturating_sub(1).min(16);
    base.saturating_mul(1 << doublings).min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backoff_starts_at_its_base_and_doubles_to_its_cap() {
        let (base, cap) = (Duration::from_secs(30), Duration::from_secs(600));
        assert_eq!(doubling(base, cap, 0), base);
        assert_eq!(doubling(base, cap, 1), base);
        assert_eq!(doubling(base, cap, 2), Duration::from_secs(60));
        assert_eq!(doubling(base, cap, 3), Duration::from_secs(120));
        assert_eq!(doubling(base, cap, 10), cap, "capped");
        assert_eq!(doubling(base, cap, u32::MAX), cap, "never overflows");
        let huge = Duration::from_secs(u64::MAX / 2);
        assert_eq!(doubling(huge, Duration::MAX, 5), Duration::MAX);
    }
}
