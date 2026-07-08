//! Token-bucket rate limiter for SDK endpoints.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

/// Configuration for the rate limiter.
#[derive(Debug, Clone, Copy)]
pub struct RateLimitConfig {
    /// Whether rate limiting is active.
    pub enabled: bool,
    /// Maximum number of tokens in a bucket (burst capacity).
    pub capacity: u32,
    /// Number of tokens refilled per second.
    pub refill_per_second: f64,
}

/// Per-key token bucket state.
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Hard ceiling on the number of live per-key buckets, to bound memory against
/// an unauthenticated key-enumeration flood (see issue #75). Not exposed in
/// `RateLimitConfig`: a sane default that avoids growing the public surface.
const MAX_BUCKETS: usize = 100_000;

/// In-memory token-bucket rate limiter keyed by an arbitrary string (e.g. SDK key prefix).
///
/// When disabled (via [`RateLimiter::disabled`] or `enabled = false`), [`Self::check`]
/// always returns `Ok(())`.
pub struct RateLimiter {
    enabled: bool,
    capacity: f64,
    refill_per_second: f64,
    max_buckets: usize,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    /// Builds a rate limiter from the given configuration.
    #[must_use]
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            enabled: config.enabled,
            capacity: f64::from(config.capacity),
            refill_per_second: config.refill_per_second,
            max_buckets: MAX_BUCKETS,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Builds a disabled rate limiter. [`Self::check`] always returns `Ok(())`.
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(RateLimitConfig {
            enabled: false,
            capacity: u32::MAX,
            refill_per_second: f64::MAX / 2.0,
        })
    }

    /// Builds a rate limiter with an injectable bucket ceiling, for deterministic
    /// eviction tests (see issue #75).
    #[cfg(test)]
    #[must_use]
    pub fn with_max_buckets(config: RateLimitConfig, max_buckets: usize) -> Self {
        Self {
            max_buckets,
            ..Self::new(config)
        }
    }

    /// Returns the current number of live buckets, for test assertions.
    #[cfg(test)]
    pub fn bucket_count(&self) -> usize {
        #[allow(clippy::expect_used)]
        self.buckets
            .lock()
            .expect("rate limiter mutex should not be poisoned")
            .len()
    }

    /// Bounds the bucket map. First removes every bucket that has fully refilled
    /// (indistinguishable from a bucket that does not exist), then, if still
    /// above `max_buckets`, evicts the buckets closest to full (the least
    /// active ones) until back under the ceiling.
    ///
    /// Reads token levels as of `now` without mutating surviving buckets: the
    /// normal refill on their next `check` already accounts for elapsed time,
    /// so mutating here would double-count it.
    fn sweep(&self, buckets: &mut HashMap<String, Bucket>, now: Instant) {
        let capacity = self.capacity;
        let refill_per_second = self.refill_per_second;
        let tokens_now = |bucket: &Bucket| {
            let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
            (bucket.tokens + elapsed * refill_per_second).min(capacity)
        };

        buckets.retain(|_, bucket| tokens_now(bucket) < capacity);

        if buckets.len() > self.max_buckets {
            let mut by_tokens: Vec<(String, f64)> = buckets
                .iter()
                .map(|(key, bucket)| (key.clone(), tokens_now(bucket)))
                .collect();
            // Highest tokens_now first: those buckets are the least active
            // (closest to full) and are evicted first.
            by_tokens.sort_by(|a, b| b.1.total_cmp(&a.1));

            let excess = buckets.len() - self.max_buckets;
            for (key, _) in by_tokens.into_iter().take(excess) {
                buckets.remove(&key);
            }
        }
    }

    /// Checks and consumes one token for `key`.
    ///
    /// Returns `Ok(())` if a token was available, or `Err(retry_after_seconds)`
    /// if the bucket is empty.
    ///
    /// # Errors
    /// Returns the estimated wait time in seconds until the next token is available.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        if !self.enabled {
            return Ok(());
        }

        #[allow(clippy::expect_used)]
        let mut buckets = self
            .buckets
            .lock()
            .expect("rate limiter mutex should not be poisoned");

        let now = Instant::now();
        let bucket = buckets.entry(key.to_owned()).or_insert_with(|| Bucket {
            tokens: self.capacity,
            last_refill: now,
        });

        // Refill tokens based on elapsed time.
        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_second).min(self.capacity);
        bucket.last_refill = now;

        let result = if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            // Seconds until the next token is available.
            let wait = (1.0 - bucket.tokens) / self.refill_per_second;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Err(wait.ceil() as u64)
        };

        if buckets.len() > self.max_buckets {
            self.sweep(&mut buckets, now);
        }

        result
    }

    /// Checks using an injectable `now` instant (for deterministic tests).
    ///
    /// This is a test-only helper exposed here so that unit tests do not need
    /// to depend on wall-clock time.
    #[cfg(test)]
    pub fn check_at(&self, key: &str, now: Instant) -> Result<(), u64> {
        if !self.enabled {
            return Ok(());
        }

        #[allow(clippy::expect_used)]
        let mut buckets = self
            .buckets
            .lock()
            .expect("rate limiter mutex should not be poisoned");

        let bucket = buckets.entry(key.to_owned()).or_insert_with(|| Bucket {
            tokens: self.capacity,
            last_refill: now,
        });

        let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_second).min(self.capacity);
        bucket.last_refill = now;

        let result = if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let wait = (1.0 - bucket.tokens) / self.refill_per_second;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Err(wait.ceil() as u64)
        };

        if buckets.len() > self.max_buckets {
            self.sweep(&mut buckets, now);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn config(capacity: u32, refill_per_second: f64) -> RateLimitConfig {
        RateLimitConfig {
            enabled: true,
            capacity,
            refill_per_second,
        }
    }

    #[test]
    fn sweep_evicts_refilled_buckets() {
        let limiter = RateLimiter::with_max_buckets(config(1, 1.0), 2);
        let t0 = Instant::now();

        assert!(limiter.check_at("alice", t0).is_ok());
        assert!(limiter.check_at("bob", t0).is_ok());

        // Both buckets are fully refilled by t0 + 10s (capacity 1, refill 1/s).
        let t1 = t0 + Duration::from_secs(10);
        assert!(limiter.check_at("carol", t1).is_ok());

        assert!(limiter.bucket_count() <= 2);
    }

    #[test]
    fn hard_cap_evicts_least_active_when_all_active() {
        let limiter = RateLimiter::with_max_buckets(config(10, 1.0), 2);
        let t0 = Instant::now();

        // All three keys are checked at the same instant, so all buckets
        // remain active (tokens < capacity, none fully refilled).
        assert!(limiter.check_at("alice", t0).is_ok());
        assert!(limiter.check_at("bob", t0).is_ok());
        assert!(limiter.check_at("carol", t0).is_ok());

        assert!(limiter.bucket_count() <= 2);
    }

    #[test]
    fn eviction_preserves_rate_limit_semantics() {
        let limiter = RateLimiter::with_max_buckets(config(1, 1.0), 2);
        let t0 = Instant::now();

        assert!(limiter.check_at("alice", t0).is_ok());
        assert!(limiter.check_at("bob", t0).is_ok());

        // Fully refills alice and bob, then evicts them via the sweep.
        let t1 = t0 + Duration::from_secs(10);
        assert!(limiter.check_at("carol", t1).is_ok());

        // Alice was evicted while fully refilled: rechecking her behaves
        // like a brand-new bucket, allowed `capacity` (1) times.
        assert!(limiter.check_at("alice", t1).is_ok());
        assert_eq!(limiter.check_at("alice", t1), Err(1));
    }

    #[test]
    fn no_sweep_below_cap() {
        let limiter = RateLimiter::with_max_buckets(config(10, 1.0), 10);
        let t0 = Instant::now();

        assert!(limiter.check_at("alice", t0).is_ok());
        assert!(limiter.check_at("bob", t0).is_ok());
        assert!(limiter.check_at("carol", t0).is_ok());

        assert_eq!(limiter.bucket_count(), 3);
    }
}
