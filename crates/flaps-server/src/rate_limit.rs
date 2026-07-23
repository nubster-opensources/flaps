//! Token-bucket rate limiter for SDK endpoints.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::preauth::limiter_key::{LimiterKey, LimiterKeyDeriver};

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

/// Number of buckets sampled per eviction round.
///
/// Eviction picks the fullest bucket out of a small sample instead of sorting
/// the whole table. The cost per round is constant, and eviction quality
/// degrades gracefully rather than making the request path explode.
const EVICTION_SAMPLE_SIZE: usize = 16;

/// Computes the current token level of `bucket` as of `now`, without
/// mutating it.
///
/// Always takes `now` as an explicit parameter rather than reading the wall
/// clock: callers that need a deterministic, testable eviction cost depend on
/// this value never drifting between two calls made at the same instant.
fn tokens_at(bucket: &Bucket, now: Instant, capacity: f64, refill_per_second: f64) -> f64 {
    let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
    (bucket.tokens + elapsed * refill_per_second).min(capacity)
}

/// In-memory token-bucket rate limiter keyed by an arbitrary string (e.g. SDK
/// key prefix).
///
/// The caller-supplied string is never stored. It is derived, under a secret
/// generated at process start, into a fixed-size [`LimiterKey`]: a bucket
/// costs the same whatever the caller sent, and the table cannot be read back
/// as a directory of attempted identifiers.
///
/// When disabled (via [`RateLimiter::disabled`] or `enabled = false`), [`Self::check`]
/// always returns `Ok(())`.
pub struct RateLimiter {
    enabled: bool,
    capacity: f64,
    refill_per_second: f64,
    max_buckets: usize,
    deriver: LimiterKeyDeriver,
    buckets: Mutex<HashMap<LimiterKey, Bucket>>,
    /// Cumulative number of buckets inspected by eviction passes.
    swept_bucket_scans: AtomicU64,
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
            deriver: LimiterKeyDeriver::new(),
            buckets: Mutex::new(HashMap::new()),
            swept_bucket_scans: AtomicU64::new(0),
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

    /// Returns the number of key bytes stored per bucket, for test assertions.
    ///
    /// Constant by construction: the map is keyed by a fixed-size derived key.
    #[cfg(test)]
    pub fn stored_key_bytes(&self) -> usize {
        std::mem::size_of::<LimiterKey>()
    }

    /// Bounds the bucket map. While still above `max_buckets`, evicts the
    /// fullest bucket out of a bounded sample.
    ///
    /// A bucket that has fully refilled sits at `self.capacity`, which is the
    /// maximum value any bucket can reach: it therefore always wins the
    /// fullest-of-sample comparison over an active bucket, so a fully
    /// refilled bucket (indistinguishable from a bucket that does not exist)
    /// already gets first priority for eviction without a dedicated
    /// full-table pass.
    ///
    /// The sample-based round replaces both a full sort of the table and a
    /// full-table retain: either one, run on every call, would itself scale
    /// with the table size. Under a fast flood the table sits just above the
    /// ceiling on nearly every request, so eviction runs on nearly every
    /// request: its cost must not depend on the table size.
    fn sweep(&self, buckets: &mut HashMap<LimiterKey, Bucket>, now: Instant) {
        while buckets.len() > self.max_buckets {
            let mut fullest: Option<(LimiterKey, f64)> = None;
            let mut scanned = 0_u64;

            for (key, bucket) in buckets.iter().take(EVICTION_SAMPLE_SIZE) {
                scanned += 1;
                let tokens = tokens_at(bucket, now, self.capacity, self.refill_per_second);
                if fullest.is_none_or(|(_, best)| tokens > best) {
                    fullest = Some((*key, tokens));
                }
            }
            self.swept_bucket_scans
                .fetch_add(scanned, Ordering::Relaxed);

            match fullest {
                Some((key, _)) => {
                    buckets.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Returns the cumulative number of buckets inspected by eviction passes.
    #[cfg(test)]
    pub fn swept_bucket_scans(&self) -> u64 {
        self.swept_bucket_scans.load(Ordering::Relaxed)
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
        let key = self.deriver.derive(key);
        let bucket = buckets.entry(key).or_insert_with(|| Bucket {
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

        let key = self.deriver.derive(key);
        let bucket = buckets.entry(key).or_insert_with(|| Bucket {
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

    #[test]
    fn eviction_work_per_request_is_bounded_whatever_the_table_size() {
        let limiter = RateLimiter::with_max_buckets(config(10, 0.000_001), 64);
        let t0 = Instant::now();

        // Fill the table well past its ceiling with distinct, freshly debited
        // keys: the refill rate is low enough that none of them is ever full,
        // which is exactly the regime where the old sweep degenerated.
        for index in 0..2_000 {
            let _ = limiter.check_at(&format!("key-{index}"), t0);
        }

        let scans = limiter.swept_bucket_scans();
        let requests = 2_000_u64;
        assert!(
            scans <= requests * 32,
            "eviction must inspect a bounded number of buckets per request, \
             not the whole table: {scans} scans for {requests} requests"
        );
        assert!(limiter.bucket_count() <= 64);
    }

    #[test]
    fn bucket_memory_does_not_grow_with_key_length() {
        let limiter = RateLimiter::new(config(10, 1.0));
        let long_key = "k".repeat(64 * 1024);

        assert!(limiter.check(&long_key).is_ok());
        assert_eq!(limiter.bucket_count(), 1);
        assert_eq!(
            limiter.stored_key_bytes(),
            16,
            "a bucket must cost a fixed number of key bytes whatever the caller sent"
        );
    }
}
