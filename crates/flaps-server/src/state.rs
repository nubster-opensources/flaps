//! Application state and the `Store` supertrait.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use flaps_compiler::CompiledRuleset;
use flaps_domain::{EnvironmentKey, ProjectKey};
use flaps_store::repository::{
    AuditLogRepository, EnvironmentRepository, FlagEnvConfigRepository, FlagRepository,
    ProjectRepository, SegmentRepository, TransactionalStore,
};

/// Bundles every store capability the admin server requires.
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
        + TransactionalStore
        + Clone
        + Send
        + Sync
        + 'static
{
}

/// Compiled ruleset cache keyed by (project, environment).
pub type CompiledCache = Arc<RwLock<HashMap<(ProjectKey, EnvironmentKey), CompiledRuleset>>>;

/// Shared application state. Cheap to clone (Arc-backed).
#[derive(Clone)]
pub struct AppState<S: Store> {
    /// The persistence backend.
    pub store: S,
    /// In-memory compiled ruleset cache, refreshed after each mutation.
    pub cache: CompiledCache,
}

impl<S: Store> AppState<S> {
    /// Builds a fresh app state around `store` with an empty cache.
    #[must_use]
    pub fn new(store: S) -> Self {
        Self {
            store,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}
