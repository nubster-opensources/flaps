//! Full-jitter exponential backoff with a local xorshift64 RNG.
//!
//! No external crate dependency: the random source is a minimal xorshift64
//! PRNG seeded from the monotonic clock at construction time. A `Jitter` seam
//! allows tests to inject a deterministic sequence.
//!
//! Algorithm (AWS "full jitter"):
//! `delay = random_uniform(0, min(backoff_max, base * 2^attempt))`

use std::time::Duration;

/// Source of random `u64` values.
///
/// A closure-based seam so tests can inject a deterministic sequence.
pub(crate) type JitterFn = Box<dyn FnMut() -> u64 + Send>;

/// Full-jitter exponential backoff state.
pub(crate) struct Backoff {
    base: Duration,
    max: Duration,
    attempt: u32,
    jitter: JitterFn,
}

impl Backoff {
    /// Creates a new [`Backoff`] with a clock-seeded xorshift64 PRNG.
    pub(crate) fn new(base: Duration, max: Duration) -> Self {
        // Seed from monotonic nanos; non-zero is required for xorshift.
        let seed = {
            let nanos = std::time::Instant::now().elapsed().subsec_nanos();
            // Use a constant fallback if nanos happens to be 0 (extremely rare).
            if nanos == 0 {
                6_364_136_223_846_793_005
            } else {
                u64::from(nanos)
            }
        };
        Self::with_jitter(base, max, xorshift_jitter(seed))
    }

    /// Creates a [`Backoff`] with a custom jitter function (for testing).
    pub(crate) fn with_jitter(base: Duration, max: Duration, jitter: JitterFn) -> Self {
        Self {
            base,
            max,
            attempt: 0,
            jitter,
        }
    }

    /// Returns the next delay using full-jitter and advances the attempt counter.
    ///
    /// `delay = random_uniform(0, ceil)` where `ceil = min(max, base * 2^attempt)`.
    pub(crate) fn next_delay(&mut self) -> Duration {
        // Cap at 2^30 to avoid overflow in the shift.
        let shift = self.attempt.min(30);
        // base_nanos * 2^attempt, saturating to avoid overflow.
        let ceil_nanos = self
            .base
            .as_nanos()
            .saturating_mul(1_u128 << shift)
            .min(self.max.as_nanos());

        let rand_val = (self.jitter)();
        // Map rand_val to [0, ceil_nanos).
        #[allow(clippy::cast_possible_truncation)]
        let delay_nanos = if ceil_nanos == 0 {
            0_u64
        } else {
            // u128 modulo then cast: ceil_nanos fits u64 because max <= Duration::MAX.
            (u128::from(rand_val) % ceil_nanos) as u64
        };

        self.attempt = self.attempt.saturating_add(1);
        Duration::from_nanos(delay_nanos)
    }

    /// Resets the attempt counter after a successful connection.
    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }
}

/// Returns a [`JitterFn`] backed by an xorshift64 PRNG seeded with `seed`.
fn xorshift_jitter(mut state: u64) -> JitterFn {
    Box::new(move || {
        // xorshift64 (Marsaglia 2003).
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic jitter that returns values from a fixed sequence.
    fn seq_jitter(values: Vec<u64>) -> JitterFn {
        let mut iter = values.into_iter().cycle();
        Box::new(move || iter.next().unwrap_or(0))
    }

    #[test]
    fn first_delay_bounded_by_base() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(30);
        // Jitter always returns max u64 -> delay = ceil - 1 (bounded by base on attempt 0).
        let mut b = Backoff::with_jitter(base, max, seq_jitter(vec![u64::MAX]));
        let delay = b.next_delay();
        // ceil = min(max, base * 2^0) = base = 100ms.
        // delay < 100ms (modulo 100_000_000 nanos).
        assert!(delay < base, "first delay must be less than base");
    }

    #[test]
    fn delay_grows_with_attempts() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(30);
        // Use a large fixed value so ceil dominates.
        let mut b = Backoff::with_jitter(base, max, seq_jitter(vec![u64::MAX / 2]));

        let d0 = b.next_delay();
        let d1 = b.next_delay();
        let d2 = b.next_delay();

        // Each subsequent ceil doubles; with a large rand value the delay should grow.
        assert!(d1 >= d0, "delay should grow with attempts");
        assert!(d2 >= d1, "delay should grow with attempts");
    }

    #[test]
    fn delay_bounded_by_max() {
        let base = Duration::from_secs(1);
        let max = Duration::from_secs(5);
        // Always return u64::MAX - 1 so we hit the ceiling.
        let mut b = Backoff::with_jitter(base, max, seq_jitter(vec![u64::MAX - 1]));

        for _ in 0..20 {
            let d = b.next_delay();
            assert!(d < max, "delay must stay below max");
        }
    }

    #[test]
    fn reset_restarts_growth() {
        let base = Duration::from_millis(100);
        let max = Duration::from_secs(30);
        // Use the midpoint so we can observe a decrease after reset.
        let rand_mid: u64 = u64::MAX / 2;
        let mut b = Backoff::with_jitter(base, max, seq_jitter(vec![rand_mid]));

        // Advance several attempts.
        for _ in 0..5 {
            b.next_delay();
        }
        let before_reset = b.attempt;
        assert!(before_reset > 0);

        b.reset();
        assert_eq!(b.attempt, 0, "reset must bring attempt back to 0");
    }

    #[test]
    fn zero_base_returns_zero_delay() {
        let base = Duration::ZERO;
        let max = Duration::from_secs(30);
        let mut b = Backoff::with_jitter(base, max, seq_jitter(vec![12345]));
        let d = b.next_delay();
        assert_eq!(d, Duration::ZERO);
    }

    #[test]
    fn xorshift_jitter_produces_nonzero_values() {
        let mut jitter = xorshift_jitter(42);
        let v1 = jitter();
        let v2 = jitter();
        let v3 = jitter();
        assert_ne!(v1, 0);
        assert_ne!(v2, 0);
        assert_ne!(v3, 0);
        // The sequence must not be constant.
        assert!(v1 != v2 || v2 != v3, "xorshift must produce varying values");
    }
}
