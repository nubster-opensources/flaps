//! Application state and the `Store` supertrait.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, RwLock, broadcast};

use flaps_compiler::CompiledRuleset;
use flaps_domain::{EnvironmentKey, ProjectKey};
use flaps_store::repository::{
    AccountRepository, AuditLogRepository, EnvironmentRepository, FlagEnvConfigRepository,
    FlagRepository, ProjectRepository, SdkKeyRepository, SegmentRepository, SessionRepository,
    TransactionalStore,
};

use crate::preauth::budget::{PreAuthBudget, PreAuthBudgetConfig};
use crate::preauth::password_pool::PasswordVerificationPool;
use crate::rate_limit::{RateLimitConfig, RateLimiter};
use crate::sse_quota::{SseQuota, SseQuotaConfig};
use crate::sync::SyncEvent;

/// Bundles every store capability the server requires.
///
/// A blanket impl covers any type implementing all parts, so handlers can bound
/// a single `S: Store` instead of repeating the full list.
pub trait Store:
    ProjectRepository
    + EnvironmentRepository
    + FlagRepository
    + SegmentRepository
    + FlagEnvConfigRepository
    + AuditLogRepository
    + SdkKeyRepository
    + AccountRepository
    + SessionRepository
    + TransactionalStore
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<T> Store for T where
    T: ProjectRepository
        + EnvironmentRepository
        + FlagRepository
        + SegmentRepository
        + FlagEnvConfigRepository
        + AuditLogRepository
        + SdkKeyRepository
        + AccountRepository
        + SessionRepository
        + TransactionalStore
        + Clone
        + Send
        + Sync
        + 'static
{
}

/// Compiled ruleset cache keyed by (project, environment).
pub type CompiledCache = Arc<RwLock<HashMap<(ProjectKey, EnvironmentKey), CompiledRuleset>>>;

/// Default session TTL in seconds (24 hours).
///
/// Shared with `flapsd_lib::config` so the documented default and the value
/// actually applied by [`AppState::new`] cannot drift apart.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 24 * 3600;

/// Default session TTL (24 hours), as a [`Duration`].
const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(DEFAULT_SESSION_TTL_SECS);

/// Default per-key SDK rate limit, in requests per minute.
///
/// Also used as the token-bucket burst capacity: a fresh key can consume its
/// whole per-minute budget immediately, then refills gradually.
pub const DEFAULT_RATE_LIMIT_PER_MINUTE: u32 = 60;

/// Default login rate limiter burst capacity (`POST /login`, keyed by username).
pub const DEFAULT_LOGIN_RATE_LIMIT_CAPACITY: u32 = 5;

/// Default login rate limiter refill rate, in tokens per second
/// (~1 attempt every 10 seconds).
pub const DEFAULT_LOGIN_RATE_LIMIT_REFILL_PER_SECOND: f64 = 0.1;

/// Default process-wide burst capacity for unauthenticated attempts.
///
/// Sized well above any plausible legitimate login burst on a single daemon,
/// and far below what it takes to saturate password verification.
pub const DEFAULT_PREAUTH_GLOBAL_CAPACITY: u32 = 120;

/// Default refill rate of the process-wide pre-authentication budget, in
/// attempts per second.
pub const DEFAULT_PREAUTH_GLOBAL_REFILL_PER_SECOND: f64 = 20.0;

/// Default burst capacity of the per-connection-address budget.
pub const DEFAULT_PREAUTH_PER_CLIENT_CAPACITY: u32 = 20;

/// Default refill rate of the per-connection-address budget, in attempts per
/// second.
pub const DEFAULT_PREAUTH_PER_CLIENT_REFILL_PER_SECOND: f64 = 1.0;

/// Broadcast channel capacity for [`SyncEvent`] notifications.
///
/// A buffer of 256 events covers typical mutation bursts. Slower subscribers
/// skip lagged ticks and re-sync on the next `GET /sync/v1/ruleset`.
const EVENTS_CHANNEL_CAPACITY: usize = 256;

/// Default per-key ceiling for concurrent `GET /sync/v1/events` subscriptions
/// (see issue #111).
///
/// A well-behaved deployment holds a small, stable number of long-lived SSE
/// connections per key (typically one per replica). Five gives headroom for
/// rolling deploys and brief reconnect overlap without letting a compromised
/// key, a reconnect storm, or a defective client open an unbounded number of
/// streams.
pub const DEFAULT_MAX_SSE_SUBSCRIPTIONS_PER_KEY: usize = 5;

/// Default global ceiling for concurrent `GET /sync/v1/events` subscriptions,
/// summed across every SDK key (see issue #111).
///
/// Bounds total sockets, memory, and broadcast subscribers held for the
/// lifetime of an SSE connection, independent of the per-key ceiling.
pub const DEFAULT_MAX_SSE_SUBSCRIPTIONS_GLOBAL: usize = 1000;

/// `Retry-After` value (in seconds) sent with a 429 response when an SSE
/// subscription attempt is rejected by [`SseQuota`] (see issue #111).
///
/// Concurrency quotas free up when an existing connection closes, which has
/// no predictable schedule the way a token-bucket refill does; this constant
/// is a documented, conservative retry guidance rather than a computed wait
/// time.
pub const SSE_QUOTA_RETRY_AFTER_SECS: u64 = 30;

/// Shared application state. Cheap to clone (Arc-backed).
#[derive(Clone)]
pub struct AppState<S: Store> {
    /// The persistence backend.
    pub store: S,
    /// In-memory compiled ruleset cache, refreshed after each mutation.
    pub cache: CompiledCache,
    /// Token-bucket rate limiter for the SDK endpoints.
    pub rate_limiter: Arc<RateLimiter>,
    /// Token-bucket rate limiter for `POST /login`, keyed by username.
    ///
    /// Dedicated budget, separate from [`Self::rate_limiter`]: a stricter
    /// throttle here reduces the value of brute-forcing a single account's
    /// password without affecting the SDK read-path budget.
    pub login_rate_limiter: Arc<RateLimiter>,
    /// Layered budget guarding unauthenticated entry points (see issues #133
    /// and #134).
    ///
    /// Distinct from [`Self::login_rate_limiter`], which is the per-identity
    /// layer alone and is kept for the account-level throttle it already
    /// provides.
    pub preauth_budget: Arc<PreAuthBudget>,
    /// Concurrency ceiling on password verification (see issue #133).
    pub password_pool: Arc<PasswordVerificationPool>,
    /// TTL for newly minted sessions.
    pub session_ttl: Duration,
    /// Broadcast channel sender for ruleset change notifications.
    ///
    /// Emits one [`SyncEvent`] per recompiled ruleset, after it is written to
    /// [`Self::cache`]. Subscribers call [`broadcast::Sender::subscribe`] to
    /// receive events; a send with no active receivers silently discards the
    /// event.
    pub events: broadcast::Sender<SyncEvent>,
    /// Concurrency quota bounding live `GET /sync/v1/events` subscriptions,
    /// per SDK key and globally (see issue #111).
    pub sse_quota: Arc<SseQuota>,
    /// Per-project mutation locks, keyed by project.
    ///
    /// See [`Self::lock_project`] for the concurrency contract and the
    /// documented single-writer (single-daemon) assumption.
    mutation_locks: Arc<StdMutex<HashMap<ProjectKey, Arc<AsyncMutex<()>>>>>,
}

impl<S: Store> AppState<S> {
    /// Builds a fresh app state around `store` with default configuration.
    ///
    /// Defaults: SDK rate limiter enabled (60 req/min per key), login rate
    /// limiter enabled (burst 5, ~1 attempt / 10s per username), session TTL 24h.
    #[must_use]
    pub fn new(store: S) -> Self {
        let (events, _) = broadcast::channel(EVENTS_CHANNEL_CAPACITY);
        Self {
            store,
            cache: Arc::new(RwLock::new(HashMap::new())),
            rate_limiter: Arc::new(RateLimiter::new(crate::rate_limit::RateLimitConfig {
                enabled: true,
                capacity: DEFAULT_RATE_LIMIT_PER_MINUTE,
                refill_per_second: f64::from(DEFAULT_RATE_LIMIT_PER_MINUTE) / 60.0,
            })),
            login_rate_limiter: Arc::new(RateLimiter::new(crate::rate_limit::RateLimitConfig {
                enabled: true,
                capacity: DEFAULT_LOGIN_RATE_LIMIT_CAPACITY,
                refill_per_second: DEFAULT_LOGIN_RATE_LIMIT_REFILL_PER_SECOND,
            })),
            preauth_budget: Arc::new(PreAuthBudget::new(PreAuthBudgetConfig {
                global: RateLimitConfig {
                    enabled: true,
                    capacity: DEFAULT_PREAUTH_GLOBAL_CAPACITY,
                    refill_per_second: DEFAULT_PREAUTH_GLOBAL_REFILL_PER_SECOND,
                },
                per_client: RateLimitConfig {
                    enabled: true,
                    capacity: DEFAULT_PREAUTH_PER_CLIENT_CAPACITY,
                    refill_per_second: DEFAULT_PREAUTH_PER_CLIENT_REFILL_PER_SECOND,
                },
                per_identity: RateLimitConfig {
                    enabled: true,
                    capacity: DEFAULT_LOGIN_RATE_LIMIT_CAPACITY,
                    refill_per_second: DEFAULT_LOGIN_RATE_LIMIT_REFILL_PER_SECOND,
                },
            })),
            password_pool: Arc::new(PasswordVerificationPool::new()),
            session_ttl: DEFAULT_SESSION_TTL,
            events,
            sse_quota: Arc::new(SseQuota::new(SseQuotaConfig {
                max_global: DEFAULT_MAX_SSE_SUBSCRIPTIONS_GLOBAL,
                max_per_key: DEFAULT_MAX_SSE_SUBSCRIPTIONS_PER_KEY,
            })),
            mutation_locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// Builds app state with explicit configuration.
    ///
    /// Used by `flapsd_lib::config::Config` to apply the configured SDK rate
    /// limit and session TTL for both the SQLite and PostgreSQL storage
    /// backends, and by tests that need non-default limiter or TTL values.
    #[must_use]
    pub fn with_config(
        store: S,
        rate_limiter: Arc<RateLimiter>,
        login_rate_limiter: Arc<RateLimiter>,
        session_ttl: Duration,
    ) -> Self {
        let (events, _) = broadcast::channel(EVENTS_CHANNEL_CAPACITY);
        Self {
            store,
            cache: Arc::new(RwLock::new(HashMap::new())),
            rate_limiter,
            login_rate_limiter,
            preauth_budget: Arc::new(PreAuthBudget::new(PreAuthBudgetConfig {
                global: RateLimitConfig {
                    enabled: true,
                    capacity: DEFAULT_PREAUTH_GLOBAL_CAPACITY,
                    refill_per_second: DEFAULT_PREAUTH_GLOBAL_REFILL_PER_SECOND,
                },
                per_client: RateLimitConfig {
                    enabled: true,
                    capacity: DEFAULT_PREAUTH_PER_CLIENT_CAPACITY,
                    refill_per_second: DEFAULT_PREAUTH_PER_CLIENT_REFILL_PER_SECOND,
                },
                per_identity: RateLimitConfig {
                    enabled: true,
                    capacity: DEFAULT_LOGIN_RATE_LIMIT_CAPACITY,
                    refill_per_second: DEFAULT_LOGIN_RATE_LIMIT_REFILL_PER_SECOND,
                },
            })),
            password_pool: Arc::new(PasswordVerificationPool::new()),
            session_ttl,
            events,
            sse_quota: Arc::new(SseQuota::new(SseQuotaConfig {
                max_global: DEFAULT_MAX_SSE_SUBSCRIPTIONS_GLOBAL,
                max_per_key: DEFAULT_MAX_SSE_SUBSCRIPTIONS_PER_KEY,
            })),
            mutation_locks: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    /// Overrides the default SSE subscription quota.
    ///
    /// Used by `flapsd_lib::config::Config` to apply configured concurrency
    /// limits for `GET /sync/v1/events`, and by tests that need non-default
    /// quota values. Kept as a separate builder method (rather than a
    /// [`Self::with_config`] parameter) so existing callers of
    /// [`Self::with_config`] are unaffected.
    #[must_use]
    pub fn with_sse_quota(mut self, sse_quota: Arc<SseQuota>) -> Self {
        self.sse_quota = sse_quota;
        self
    }

    /// Acquires the per-project mutation lock, creating it on first use.
    ///
    /// # Single-writer assumption
    ///
    /// This lock serializes every in-scope mutation (`PUT`/`DELETE` of
    /// project, environment, flag, segment, `flag_env_config`) that targets
    /// the same project **within this process**. It is an in-process
    /// concurrency control, not a distributed one: it does nothing to
    /// coordinate writes issued by a second `flapsd` process against the
    /// same database. `flapsd` is deployed as a single writer per database
    /// today (each daemon owns its own in-memory [`Self::cache`] and
    /// [`Self::events`] broadcast channel, so a second daemon would already
    /// be an independent, uncoordinated cache); a database-level
    /// compare-and-swap is the documented evolution for a future
    /// multi-daemon deployment.
    ///
    /// Callers must hold the returned guard for the entire mutation cycle:
    /// from before reading the resource for the `If-Match` check, through
    /// the store write, through recompiling and installing the affected
    /// rulesets. Holding it across that whole `.await` chain is what makes
    /// the precondition check atomic with the write (#108) and guarantees
    /// the cache is always recompiled from the last committed state (#105).
    pub async fn lock_project(&self, project: &ProjectKey) -> OwnedMutexGuard<()> {
        let project_mutex = {
            // Short, synchronous critical section: no `.await` while holding
            // the registry lock, so a `std::sync::Mutex` is appropriate here
            // (a `tokio::sync::Mutex` would add unneeded async overhead).
            let mut registry = self
                .mutation_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry
                .entry(project.clone())
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        project_mutex.lock_owned().await
    }

    /// Removes `project`'s entry from the mutation-lock registry, if it is
    /// currently unused.
    ///
    /// [`Self::lock_project`] never evicts an entry: the registry is keyed by
    /// the REQUESTED project key, acquired before the parent-existence check,
    /// so a caller that repeatedly mutates a never-created project (or
    /// deletes a project) would otherwise leave one permanent entry per
    /// distinct key ever mentioned. This call lets handlers reclaim the entry
    /// at the two points where it is safe and worthwhile: right before
    /// returning `NotFound` for a missing parent, and after a successful
    /// `delete_project`.
    ///
    /// # Caller contract
    ///
    /// The caller must have already **dropped** the [`OwnedMutexGuard`]
    /// returned by [`Self::lock_project`] for this `project` before calling
    /// this method: it does not accept or drop the guard itself, so it is the
    /// caller's responsibility to end the mutation cycle first.
    ///
    /// # The `strong_count == 1` gate
    ///
    /// Removal is safe only when this registry is the SOLE owner of the
    /// `Arc<AsyncMutex<()>>` for `project` (`Arc::strong_count(&entry) == 1`),
    /// checked while holding the registry's own `std::sync::Mutex` so no
    /// other thread can observe or change the count concurrently with the
    /// decision. If the count is greater than 1, some other in-flight
    /// `lock_project` call has already cloned this same Arc (it is currently
    /// waiting to acquire it, or already holds it) and must keep serializing
    /// against every other mutation for this project through that SAME
    /// mutex. Removing the map entry in that situation would not affect the
    /// task already holding a clone, but it WOULD let a subsequent
    /// `lock_project` call for the same key `or_insert_with` a brand-new,
    /// independent `Arc<AsyncMutex<()>>` -- two different mutexes now
    /// "guarding" the same project key, silently losing mutual exclusion
    /// between them. This is the same unsoundness trap as a naive cache
    /// sweep that evicts an entry a concurrent reader is still mid-use of: a
    /// resource must never be reclaimed out from under a live reference to
    /// it. A false negative here (not removing when removal would in fact
    /// have been safe) is harmless: the entry simply stays in the registry
    /// a little longer, same as pre-fix behavior for that one request.
    pub fn release_project_lock_if_unused(&self, project: &ProjectKey) {
        let mut registry = self
            .mutation_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = registry.get(project) {
            if Arc::strong_count(entry) == 1 {
                registry.remove(project);
            }
        }
    }

    /// Returns the number of entries currently in the mutation-lock registry.
    ///
    /// Not used by request handling; exposed for regression tests (in this
    /// crate and in integration tests under `tests/`) asserting the registry
    /// stays bounded instead of growing once per distinct project key ever
    /// mentioned in a request.
    #[must_use]
    pub fn mutation_lock_registry_len(&self) -> usize {
        self.mutation_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use flaps_store::{KeyHasher, sqlite::SqliteStore};

    use super::AppState;
    use flaps_domain::ProjectKey;

    async fn make_store() -> SqliteStore {
        SqliteStore::in_memory(KeyHasher::new(b"test-pepper-32-bytes-long-enough"))
            .await
            .expect("in-memory store")
    }

    /// Two concurrent `lock_project` calls on the SAME project key must never
    /// hold the critical section at the same time. Uses a `Barrier` to force
    /// genuine overlap of the two tasks (never a timing sleep), and an atomic
    /// high-water mark of concurrent entrants to detect any overlap
    /// deterministically, regardless of scheduling order.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn lock_project_serializes_same_project() {
        let store = make_store().await;
        let state = AppState::new(store);
        let project = ProjectKey::new("proj").unwrap();

        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let concurrent_entrants = std::sync::Arc::new(AtomicU32::new(0));
        let max_concurrent_entrants = std::sync::Arc::new(AtomicU32::new(0));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let state = state.clone();
            let project = project.clone();
            let barrier = barrier.clone();
            let concurrent_entrants = concurrent_entrants.clone();
            let max_concurrent_entrants = max_concurrent_entrants.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                let _guard = state.lock_project(&project).await;

                let current = concurrent_entrants.fetch_add(1, Ordering::SeqCst) + 1;
                max_concurrent_entrants.fetch_max(current, Ordering::SeqCst);
                // Yield cooperatively to give the other task every chance to
                // observe (and prove) an overlap, without a wall-clock sleep.
                tokio::task::yield_now().await;
                concurrent_entrants.fetch_sub(1, Ordering::SeqCst);
            }));
        }

        for handle in handles {
            handle.await.expect("task must not panic");
        }

        assert_eq!(
            max_concurrent_entrants.load(Ordering::SeqCst),
            1,
            "lock_project must serialize mutations against the same project"
        );
    }

    /// Locks for DIFFERENT projects must not exclude one another: the map is
    /// keyed per-project, not a single global mutex.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn lock_project_does_not_serialize_distinct_projects() {
        let store = make_store().await;
        let state = AppState::new(store);
        let project_a = ProjectKey::new("proj-a").unwrap();
        let project_b = ProjectKey::new("proj-b").unwrap();

        // Both guards are held concurrently; if lock_project used one global
        // lock this would deadlock (the second call would never return while
        // the first guard is alive on the same task set), so a bounded join
        // succeeding is itself the proof of independence.
        let guard_a = state.lock_project(&project_a).await;
        let guard_b = state.lock_project(&project_b).await;
        drop(guard_a);
        drop(guard_b);
    }

    /// After the guard is dropped and no other task holds a reference to the
    /// same entry, `release_project_lock_if_unused` must remove it: this is
    /// the registry-bounding half of Fix 2.
    #[tokio::test]
    async fn release_project_lock_if_unused_removes_an_unreferenced_entry() {
        let store = make_store().await;
        let state = AppState::new(store);
        let project = ProjectKey::new("ghost-project").unwrap();

        let guard = state.lock_project(&project).await;
        assert_eq!(state.mutation_lock_registry_len(), 1);

        drop(guard);
        state.release_project_lock_if_unused(&project);

        assert_eq!(
            state.mutation_lock_registry_len(),
            0,
            "the registry entry must be removed once the guard is dropped and no other \
             task references it"
        );
    }

    /// If something else still holds a clone of the registry's
    /// `Arc<AsyncMutex<()>>` for this project -- exactly what a concurrent
    /// `lock_project` call in progress would hold -- `release_project_lock_if_unused`
    /// must NOT remove the entry: doing so would let a later `lock_project`
    /// call install a second, independent mutex for the same key, silently
    /// losing mutual exclusion (the `strong_count == 1` gate). The extra
    /// clone is taken directly from the private registry (this test module
    /// is a child of `state`, so it can see the private field) rather than
    /// via a second spawned task, so the assertion is deterministic instead
    /// of depending on task-scheduling timing.
    #[tokio::test]
    async fn release_project_lock_if_unused_keeps_an_entry_still_referenced_elsewhere() {
        let store = make_store().await;
        let state = AppState::new(store);
        let project = ProjectKey::new("contended-project").unwrap();

        let guard = state.lock_project(&project).await;
        drop(guard);

        // Simulate a concurrent `lock_project` call that has already cloned
        // the Arc out of the registry (and is about to, or currently does,
        // hold the lock through it) but has not returned yet.
        let extra_reference = {
            let registry = state
                .mutation_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry.get(&project).expect("entry must exist").clone()
        };

        state.release_project_lock_if_unused(&project);
        assert_eq!(
            state.mutation_lock_registry_len(),
            1,
            "the entry must survive while `extra_reference` still holds the Arc"
        );

        drop(extra_reference);
        state.release_project_lock_if_unused(&project);
        assert_eq!(
            state.mutation_lock_registry_len(),
            0,
            "once the extra reference is gone too, release must remove the entry"
        );
    }
}
