//! Application state and the `Store` supertrait.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, broadcast};

use flaps_compiler::CompiledRuleset;
use flaps_domain::{EnvironmentKey, ProjectKey};
use flaps_store::repository::{
    AccountRepository, AuditLogRepository, EnvironmentRepository, FlagEnvConfigRepository,
    FlagRepository, ProjectRepository, SdkKeyRepository, SegmentRepository, SessionRepository,
    TransactionalStore,
};

use crate::rate_limit::RateLimiter;
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

/// Broadcast channel capacity for [`SyncEvent`] notifications.
///
/// A buffer of 256 events covers typical mutation bursts. Slower subscribers
/// skip lagged ticks and re-sync on the next `GET /sync/v1/ruleset`.
const EVENTS_CHANNEL_CAPACITY: usize = 256;

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
    /// TTL for newly minted sessions.
    pub session_ttl: Duration,
    /// Broadcast channel sender for ruleset change notifications.
    ///
    /// Emits one [`SyncEvent`] per recompiled ruleset, after it is written to
    /// [`Self::cache`]. Subscribers call [`broadcast::Sender::subscribe`] to
    /// receive events; a send with no active receivers silently discards the
    /// event.
    pub events: broadcast::Sender<SyncEvent>,
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
            session_ttl: DEFAULT_SESSION_TTL,
            events,
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
            session_ttl,
            events,
        }
    }
}
