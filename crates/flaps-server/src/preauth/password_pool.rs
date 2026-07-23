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

    /// Runs one verification on a blocking thread, if a permit is free.
    ///
    /// # Errors
    /// Returns [`PreAuthRejection::GlobalBudgetExhausted`] rather than queueing
    /// without bound when the pool is saturated. Refusing immediately keeps
    /// latency bounded under attack; a queued request would immobilise exactly
    /// the resource being exhausted.
    pub async fn run<F, T>(&self, task: F) -> Result<T, PreAuthRejection>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| PreAuthRejection::GlobalBudgetExhausted)?;

        let outcome = tokio::task::spawn_blocking(move || {
            let value = task();
            drop(permit);
            value
        })
        .await;

        outcome.map_err(|_| PreAuthRejection::GlobalBudgetExhausted)
    }
}

impl Default for PasswordVerificationPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn a_task_within_capacity_runs_and_returns_its_value() {
        let pool = PasswordVerificationPool::with_capacity(2);
        assert_eq!(pool.run(|| 42).await, Ok(42));
    }

    #[tokio::test]
    async fn saturation_refuses_instead_of_queueing() {
        let pool = Arc::new(PasswordVerificationPool::with_capacity(1));
        let release = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(tokio::sync::Notify::new());

        let occupant = {
            let pool = Arc::clone(&pool);
            let release = Arc::clone(&release);
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                pool.run(move || {
                    started.notify_one();
                    // Block the single permit until the test releases it.
                    futures_executor_block_on_notified(&release);
                })
                .await
            })
        };

        started.notified().await;

        assert_eq!(
            pool.run(|| ()).await,
            Err(PreAuthRejection::GlobalBudgetExhausted),
            "a saturated pool must refuse immediately: a waiting request holds \
             exactly the resource the attacker is trying to exhaust"
        );

        release.notify_one();
        occupant
            .await
            .expect("occupant task")
            .expect("occupant run");
    }

    #[tokio::test]
    async fn a_permit_is_released_when_the_task_ends() {
        let pool = PasswordVerificationPool::with_capacity(1);
        let calls = Arc::new(AtomicUsize::new(0));

        for _ in 0..5 {
            let calls = Arc::clone(&calls);
            pool.run(move || calls.fetch_add(1, Ordering::Relaxed))
                .await
                .expect("sequential runs stay within capacity");
        }

        assert_eq!(calls.load(Ordering::Relaxed), 5);
    }

    /// Blocks the current blocking thread until `notify` fires.
    fn futures_executor_block_on_notified(notify: &tokio::sync::Notify) {
        #[allow(clippy::expect_used)]
        let handle =
            tokio::runtime::Handle::try_current().expect("test runs inside a tokio runtime");
        handle.block_on(notify.notified());
    }
}
