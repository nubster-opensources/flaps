//! Bounds how many password verifications run at once, off the async runtime.
//!
//! Argon2 is deliberately expensive, and the anti-enumeration path spends its
//! full cost even for an account that does not exist. Left on the runtime's
//! worker threads and unbounded, that turns a correct defence into an
//! amplifier: an attacker with no valid credential can freeze unrelated
//! requests at will.

use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::preauth::budget::PreAuthRejection;

/// Default number of concurrent verifications, derived from the machine.
///
/// Bounded independently of the SQL connection pool: the two costs are not
/// interchangeable, and sizing one from the other makes the total
/// unpredictable.
fn default_capacity() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get)
}

/// Bounds how many password verifications run at once, off the async runtime.
pub struct PasswordVerificationPool {
    permits: Arc<Semaphore>,
}

impl PasswordVerificationPool {
    /// Builds a pool sized from the available parallelism.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(default_capacity())
    }

    /// Builds a pool with an explicit concurrency ceiling.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(capacity.max(1))),
        }
    }

    /// Acquires a permit if one is free, without queueing.
    ///
    /// # Errors
    /// Returns [`PreAuthRejection::GlobalBudgetExhausted`] immediately when the
    /// pool is saturated, rather than queueing without bound: a queued request
    /// would immobilise exactly the resource being exhausted. Hold the returned
    /// guard across the whole verification; dropping it frees the permit.
    pub fn try_acquire(&self) -> Result<PasswordVerificationPermit, PreAuthRejection> {
        Arc::clone(&self.permits)
            .try_acquire_owned()
            .map(|permit| PasswordVerificationPermit { _permit: permit })
            .map_err(|_| PreAuthRejection::GlobalBudgetExhausted)
    }
}

impl Default for PasswordVerificationPool {
    fn default() -> Self {
        Self::new()
    }
}

/// A held permit bounding one in-flight password verification.
///
/// Keep this guard alive for the entire verification (the store's SQL lookup
/// plus its off-runtime Argon2 computation). Dropping it releases the permit.
/// Bounding the number of live guards is what caps how many Argon2
/// computations run at once, independently of Tokio's blocking pool.
pub struct PasswordVerificationPermit {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_within_capacity_acquires_a_permit() {
        let pool = PasswordVerificationPool::with_capacity(2);
        assert!(pool.try_acquire().is_ok());
    }

    #[test]
    fn saturation_refuses_until_the_held_guard_drops() {
        let pool = PasswordVerificationPool::with_capacity(1);

        let occupant = pool
            .try_acquire()
            .expect("first acquire succeeds within capacity");

        assert_eq!(
            pool.try_acquire().map(|_| ()),
            Err(PreAuthRejection::GlobalBudgetExhausted),
            "a saturated pool must refuse immediately: a waiting request holds \
             exactly the resource the attacker is trying to exhaust"
        );

        drop(occupant);

        assert!(
            pool.try_acquire().is_ok(),
            "dropping the guard must release the permit"
        );
    }

    #[test]
    fn a_permit_is_released_when_the_guard_is_dropped_across_a_loop() {
        let capacity = 3;
        let pool = PasswordVerificationPool::with_capacity(capacity);

        for _ in 0..(capacity * 5) {
            let permit = pool
                .try_acquire()
                .expect("previous guard was dropped, so capacity is free again");
            drop(permit);
        }
    }
}
