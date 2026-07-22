//! Concurrency quota bounding the number of live `GET /sync/v1/events` SSE
//! subscriptions, per SDK key and globally (see issue #111).
//!
//! A compromised key, a reconnect storm or a defective client can otherwise
//! open unbounded long-lived streams and exhaust sockets, memory and
//! broadcast subscribers: the ordinary per-request token bucket
//! ([`crate::rate_limit`]) does not bound resources held for the lifetime of
//! a streaming connection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Configuration for [`SseQuota`].
#[derive(Debug, Clone, Copy)]
pub struct SseQuotaConfig {
    /// Maximum number of concurrent SSE subscriptions across every SDK key.
    ///
    /// Zero means "reject every subscription", NOT "unlimited". This is the
    /// opposite convention from the sibling [`crate::rate_limit::RateLimiter`]
    /// primitive, which expresses "no limit" as an `enabled` flag
    /// (`RateLimiter::disabled()`) rather than as a numeric edge case.
    ///
    /// Must not exceed [`tokio::sync::Semaphore::MAX_PERMITS`]: this value is
    /// handed directly to a `Semaphore::new` in [`SseQuota::new`], which
    /// panics eagerly, at construction time, if it is exceeded. `flapsd`'s
    /// own configuration validation rejects an out-of-range value before it
    /// ever reaches here; a downstream embedder of this library that
    /// constructs an [`SseQuota`] directly is responsible for the same check.
    pub max_global: usize,
    /// Maximum number of concurrent SSE subscriptions for a single SDK key.
    ///
    /// Zero means "reject every subscription", NOT "unlimited"; see
    /// [`Self::max_global`] for the same caveat.
    ///
    /// Must not exceed [`tokio::sync::Semaphore::MAX_PERMITS`]; unlike
    /// [`Self::max_global`], this bound is NOT checked at [`SseQuota::new`]
    /// time. Each SDK key's semaphore is created lazily, on that key's first
    /// [`SseQuota::try_acquire`] call, so an out-of-range value panics lazily,
    /// inside that call, the first time any key is used, rather than eagerly
    /// at construction.
    pub max_per_key: usize,
}

/// Why a subscription attempt was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseQuotaError {
    /// The global concurrency ceiling has been reached.
    GlobalLimitReached,
    /// The per-key concurrency ceiling has been reached for this SDK key.
    PerKeyLimitReached,
}

/// The permits held by one live SSE subscription.
///
/// Move this value into the response stream returned to the client: dropping
/// it releases both the global and per-key permits via [`Drop`], and
/// decrements the active-subscription counter. This is the single mechanism
/// that covers every release path (normal disconnect, client cancellation,
/// server shutdown), because all three reduce to the same event: the stream
/// value is dropped.
#[derive(Debug)]
#[must_use = "dropping this guard immediately releases the subscription slot"]
pub struct SseSubscriptionGuard {
    _global: OwnedSemaphorePermit,
    _per_key: OwnedSemaphorePermit,
    active: Arc<AtomicU64>,
}

impl Drop for SseSubscriptionGuard {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Bounds the number of concurrently open SSE subscriptions, per SDK key and
/// globally.
///
/// ## Lock ordering
///
/// Acquisition always takes the GLOBAL semaphore permit first, then the
/// PER-KEY permit, a fixed order that makes the quota deadlock-free: no two
/// callers can ever wait on each other's semaphores in opposite order. If the
/// per-key acquisition fails after the global one already succeeded, the
/// global permit is dropped (released) before the error is returned, so a
/// rejected attempt never holds on to a global slot.
///
/// ## Non-blocking
///
/// [`Self::try_acquire`] uses `try_acquire_owned`: an over-quota request is
/// rejected immediately rather than queued. Queueing would hold the incoming
/// HTTP request open while it waits for a streaming slot, which is exactly
/// the unbounded-resource problem this quota exists to prevent.
pub struct SseQuota {
    global: Arc<Semaphore>,
    // No eviction: entries are only ever created AFTER authentication
    // succeeds (see `try_acquire`), so cardinality is bounded by the number
    // of distinct SDK keys that actually authenticated over the process
    // lifetime, not by attacker-controlled input. This is unlike
    // `rate_limit.rs`'s `MAX_BUCKETS` sweep, which guards an unauthenticated
    // enumeration path (issue #75) and therefore does need a cap. Growth here
    // is monotonic but judged negligible: a real deployment authenticates a
    // small, roughly fixed set of SDK keys.
    //
    // IMPORTANT for whoever revisits this: a naive "remove entries with an
    // idle semaphore" sweep would be UNSOUND. A concurrent `try_acquire` can
    // already hold an `Arc` clone of a key's semaphore in the window between
    // the map lookup and `try_acquire_owned` completing; if a sweep removed
    // the map entry during that window, the NEXT `try_acquire` for the same
    // key would `or_insert_with` a fresh semaphore at full capacity,
    // admitting up to `2 * max_per_key` concurrent subscriptions for that
    // key. Any future eviction must hold the map lock while checking
    // `Arc::strong_count(sem) == 1` (no in-flight acquire holds a clone)
    // before removing an entry.
    per_key: Mutex<HashMap<String, Arc<Semaphore>>>,
    max_global: usize,
    max_per_key: usize,
    active_subscriptions: Arc<AtomicU64>,
    rejected_subscriptions: AtomicU64,
}

impl SseQuota {
    /// Builds a quota from the given configuration.
    ///
    /// # Panics
    /// Panics if `config.max_global` exceeds
    /// [`tokio::sync::Semaphore::MAX_PERMITS`] (the underlying
    /// `Semaphore::new` call panics). `config.max_per_key` is not validated
    /// here: it is only checked, lazily, the first time it is used to build a
    /// per-key semaphore inside [`Self::try_acquire`], so an out-of-range
    /// `max_per_key` does NOT panic at construction time.
    #[must_use]
    pub fn new(config: SseQuotaConfig) -> Self {
        Self {
            global: Arc::new(Semaphore::new(config.max_global)),
            per_key: Mutex::new(HashMap::new()),
            max_global: config.max_global,
            max_per_key: config.max_per_key,
            active_subscriptions: Arc::new(AtomicU64::new(0)),
            rejected_subscriptions: AtomicU64::new(0),
        }
    }

    /// Attempts to acquire one subscription slot for `key`.
    ///
    /// # Errors
    /// Returns [`SseQuotaError`] immediately, without queueing, when either
    /// the global or the per-key ceiling is currently exhausted.
    pub fn try_acquire(&self, key: &str) -> Result<SseSubscriptionGuard, SseQuotaError> {
        // GLOBAL first, then PER-KEY: see the struct-level lock-ordering doc.
        let Ok(global_permit) = Arc::clone(&self.global).try_acquire_owned() else {
            self.rejected_subscriptions.fetch_add(1, Ordering::Relaxed);
            return Err(SseQuotaError::GlobalLimitReached);
        };

        let key_semaphore = {
            let mut per_key = self
                .per_key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(
                per_key
                    .entry(key.to_owned())
                    .or_insert_with(|| Arc::new(Semaphore::new(self.max_per_key))),
            )
        };

        let Ok(per_key_permit) = key_semaphore.try_acquire_owned() else {
            // `global_permit` drops here (end of scope), releasing it
            // immediately: a rejected attempt never leaks a global slot.
            self.rejected_subscriptions.fetch_add(1, Ordering::Relaxed);
            return Err(SseQuotaError::PerKeyLimitReached);
        };

        self.active_subscriptions.fetch_add(1, Ordering::Relaxed);
        Ok(SseSubscriptionGuard {
            _global: global_permit,
            _per_key: per_key_permit,
            active: Arc::clone(&self.active_subscriptions),
        })
    }

    /// Returns the current number of live SSE subscriptions, across every
    /// SDK key (the counter behind the GLOBAL ceiling, [`Self::max_global`]).
    #[must_use]
    pub fn active_subscriptions(&self) -> u64 {
        self.active_subscriptions.load(Ordering::Relaxed)
    }

    /// Returns the total number of subscription attempts rejected since
    /// construction (global or per-key), for metrics.
    #[must_use]
    pub fn rejected_subscriptions(&self) -> u64 {
        self.rejected_subscriptions.load(Ordering::Relaxed)
    }

    /// Returns the configured global concurrency ceiling.
    ///
    /// Lets callers on the rejection path (see `flaps_server::sync`) log a
    /// [`SseQuotaError::GlobalLimitReached`] rejection as "N/limit" rather
    /// than a bare active count with nothing to compare it against.
    #[must_use]
    pub fn max_global(&self) -> usize {
        self.max_global
    }

    /// Returns the configured per-key concurrency ceiling.
    ///
    /// Lets callers on the rejection path (see `flaps_server::sync`) log a
    /// [`SseQuotaError::PerKeyLimitReached`] rejection as "N/limit" rather
    /// than a bare active count with nothing to compare it against.
    #[must_use]
    pub fn max_per_key(&self) -> usize {
        self.max_per_key
    }

    /// Returns the current number of live SSE subscriptions for `key` alone
    /// (the counter behind the PER-KEY ceiling, [`Self::max_per_key`]), as
    /// opposed to [`Self::active_subscriptions`], which is global.
    ///
    /// Takes the per-key map lock, released before returning. This is meant
    /// for the rejection path, which is cold: re-taking the lock here (on
    /// top of the one already taken and released inside
    /// [`Self::try_acquire`]) is cheap, and this method never holds the
    /// per-key lock and a semaphore permit acquisition at the same time.
    /// A key that never called [`Self::try_acquire`] reports zero rather
    /// than creating an entry.
    #[must_use]
    pub fn active_subscriptions_for_key(&self, key: &str) -> u64 {
        let per_key = self
            .per_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        per_key.get(key).map_or(0, |semaphore| {
            let held = self
                .max_per_key
                .saturating_sub(semaphore.available_permits());
            u64::try_from(held).unwrap_or(u64::MAX)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(max_global: usize, max_per_key: usize) -> SseQuotaConfig {
        SseQuotaConfig {
            max_global,
            max_per_key,
        }
    }

    #[test]
    fn try_acquire_allows_up_to_per_key_limit() {
        let quota = SseQuota::new(config(10, 2));
        let g1 = quota.try_acquire("key-a");
        let g2 = quota.try_acquire("key-a");
        assert!(g1.is_ok());
        assert!(g2.is_ok());
        assert_eq!(quota.active_subscriptions(), 2);
    }

    #[test]
    fn nth_plus_one_subscription_for_key_is_rejected() {
        let quota = SseQuota::new(config(10, 2));
        let _g1 = quota.try_acquire("key-a").expect("first must succeed");
        let _g2 = quota.try_acquire("key-a").expect("second must succeed");

        let err = quota
            .try_acquire("key-a")
            .expect_err("third subscription for the same key must be rejected");
        assert_eq!(err, SseQuotaError::PerKeyLimitReached);
        assert_eq!(quota.active_subscriptions(), 2);
        assert_eq!(quota.rejected_subscriptions(), 1);
    }

    #[test]
    fn per_key_limits_are_independent_across_keys() {
        let quota = SseQuota::new(config(10, 1));
        let _a = quota.try_acquire("key-a").expect("key-a must succeed");
        let _b = quota.try_acquire("key-b").expect("key-b must succeed");
        assert_eq!(quota.active_subscriptions(), 2);
    }

    #[test]
    fn global_cap_rejects_across_keys() {
        // Per-key ceiling is generous; only the global ceiling should bind.
        let quota = SseQuota::new(config(2, 10));
        let _a = quota.try_acquire("key-a").expect("first must succeed");
        let _b = quota.try_acquire("key-b").expect("second must succeed");

        let err = quota
            .try_acquire("key-c")
            .expect_err("global ceiling reached: third key must be rejected");
        assert_eq!(err, SseQuotaError::GlobalLimitReached);
        assert_eq!(quota.rejected_subscriptions(), 1);
    }

    #[test]
    fn dropping_a_permit_frees_the_per_key_slot_for_a_new_subscription() {
        let quota = SseQuota::new(config(10, 1));
        let g1 = quota.try_acquire("key-a").expect("first must succeed");
        quota
            .try_acquire("key-a")
            .expect_err("second must be rejected while the first is held");

        drop(g1);

        let g2 = quota
            .try_acquire("key-a")
            .expect("slot must be free again after the first guard was dropped");
        assert_eq!(quota.active_subscriptions(), 1);
        drop(g2);
        assert_eq!(quota.active_subscriptions(), 0);
    }

    #[test]
    fn dropping_a_permit_frees_the_global_slot_for_a_new_subscription() {
        let quota = SseQuota::new(config(1, 10));
        let g1 = quota.try_acquire("key-a").expect("first must succeed");
        quota
            .try_acquire("key-b")
            .expect_err("global ceiling reached: second key must be rejected");

        drop(g1);

        let g2 = quota
            .try_acquire("key-b")
            .expect("global slot must be free again after the first guard was dropped");
        drop(g2);
    }

    /// Proves the lock-ordering contract from the struct doc: when the GLOBAL
    /// permit is acquired but the PER-KEY acquisition then fails, the GLOBAL
    /// permit must be released immediately rather than leaked. With
    /// `max_global = 2` and `max_per_key = 1`, a second attempt for the same
    /// already-full key must fail on the PER-KEY check while leaving exactly
    /// one GLOBAL slot free for a different key.
    #[test]
    fn global_permit_is_released_when_per_key_acquisition_fails() {
        let quota = SseQuota::new(config(2, 1));
        let g1 = quota.try_acquire("key-a").expect("first must succeed");

        let err = quota
            .try_acquire("key-a")
            .expect_err("key-a per-key ceiling (1) is already reached");
        assert_eq!(err, SseQuotaError::PerKeyLimitReached);

        // If the GLOBAL permit taken during the failed attempt above had
        // leaked, this would now fail with GlobalLimitReached instead of
        // succeeding.
        let g2 = quota
            .try_acquire("key-b")
            .expect("the GLOBAL permit from the failed attempt must not have leaked");

        assert_eq!(quota.active_subscriptions(), 2);
        drop(g1);
        drop(g2);
    }

    #[test]
    fn zero_capacity_rejects_immediately_without_queueing() {
        let quota = SseQuota::new(config(0, 0));
        let err = quota
            .try_acquire("key-a")
            .expect_err("zero global capacity must reject immediately");
        assert_eq!(err, SseQuotaError::GlobalLimitReached);
    }

    #[test]
    fn rejected_subscriptions_counts_both_global_and_per_key_reasons() {
        let quota = SseQuota::new(config(2, 1));
        let _a1 = quota.try_acquire("key-a").expect("first must succeed");

        // Rejected on the PER-KEY check (global still has one slot left).
        let per_key_err = quota
            .try_acquire("key-a")
            .expect_err("key-a per-key ceiling already reached");
        assert_eq!(per_key_err, SseQuotaError::PerKeyLimitReached);

        let _b1 = quota
            .try_acquire("key-b")
            .expect("second key must succeed, consuming the last global slot");

        // Rejected on the GLOBAL check.
        let global_err = quota
            .try_acquire("key-c")
            .expect_err("global ceiling already reached");
        assert_eq!(global_err, SseQuotaError::GlobalLimitReached);

        assert_eq!(
            quota.rejected_subscriptions(),
            2,
            "both rejection reasons must be counted"
        );
    }

    /// A single guard drop is the mechanism behind every release path: normal
    /// client disconnect, client-initiated cancellation, and server shutdown
    /// all end with the response stream (and therefore this guard) being
    /// dropped. This test proves that mechanism directly.
    #[test]
    fn guard_drop_releases_permits_covering_disconnect_cancellation_and_shutdown() {
        let quota = SseQuota::new(config(1, 1));
        let guard = quota.try_acquire("key-a").expect("must succeed");
        assert_eq!(quota.active_subscriptions(), 1);

        drop(guard);

        assert_eq!(quota.active_subscriptions(), 0);
        assert!(
            quota.try_acquire("key-a").is_ok(),
            "slot must be usable again after the guard is dropped"
        );
    }

    /// The rejection log (see `flaps_server::sync`) needs the configured
    /// ceilings, not just the live counters, so that a `PerKeyLimitReached`
    /// rejection can report "N/limit" instead of a bare count with no limit
    /// to compare it against.
    #[test]
    fn max_global_and_max_per_key_expose_the_configured_ceilings() {
        let quota = SseQuota::new(config(42, 7));
        assert_eq!(quota.max_global(), 42);
        assert_eq!(quota.max_per_key(), 7);
    }

    /// `active_subscriptions()` is the GLOBAL counter; the rejection log
    /// needs the PER-KEY count for a `PerKeyLimitReached` rejection, which is
    /// generally not the same number.
    #[test]
    fn active_subscriptions_for_key_reflects_only_that_key() {
        let quota = SseQuota::new(config(10, 5));
        let _a1 = quota
            .try_acquire("key-a")
            .expect("first key-a must succeed");
        let _a2 = quota
            .try_acquire("key-a")
            .expect("second key-a must succeed");
        let _b1 = quota
            .try_acquire("key-b")
            .expect("first key-b must succeed");

        assert_eq!(quota.active_subscriptions_for_key("key-a"), 2);
        assert_eq!(quota.active_subscriptions_for_key("key-b"), 1);
        assert_eq!(
            quota.active_subscriptions_for_key("key-never-seen"),
            0,
            "a key that never acquired must report zero, not panic or create an entry"
        );
    }
}
