//! Application state and the `Store` supertrait.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use flaps_compiler::CompiledRuleset;
use flaps_domain::{EnvironmentKey, ProjectKey};
use flaps_store::repository::{
    AccountRepository, AuditLogRepository, EnvironmentRepository, FlagEnvConfigRepository,
    FlagRepository, ProjectRepository, SdkKeyRepository, SegmentRepository, SessionRepository,
    TransactionalStore,
};

use crate::rate_limit::RateLimiter;

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

/// Default session TTL (24 hours).
const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(24 * 3600);

/// Shared application state. Cheap to clone (Arc-backed).
#[derive(Clone)]
pub struct AppState<S: Store> {
    /// The persistence backend.
    pub store: S,
    /// In-memory compiled ruleset cache, refreshed after each mutation.
    pub cache: CompiledCache,
    /// Token-bucket rate limiter for the SDK endpoints.
    pub rate_limiter: Arc<RateLimiter>,
    /// TTL for newly minted sessions.
    pub session_ttl: Duration,
}

impl<S: Store> AppState<S> {
    /// Builds a fresh app state around `store` with default configuration.
    ///
    /// Defaults: rate limiter enabled (60 req/min per key), session TTL 24h.
    #[must_use]
    pub fn new(store: S) -> Self {
        Self {
            store,
            cache: Arc::new(RwLock::new(HashMap::new())),
            rate_limiter: Arc::new(RateLimiter::new(crate::rate_limit::RateLimitConfig {
                enabled: true,
                capacity: 60,
                refill_per_second: 1.0,
            })),
            session_ttl: DEFAULT_SESSION_TTL,
        }
    }

    /// Builds app state with explicit configuration (used in tests and binary).
    #[must_use]
    pub fn with_config(store: S, rate_limiter: Arc<RateLimiter>, session_ttl: Duration) -> Self {
        Self {
            store,
            cache: Arc::new(RwLock::new(HashMap::new())),
            rate_limiter,
            session_ttl,
        }
    }
}
