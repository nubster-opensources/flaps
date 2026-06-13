//! Repository trait for the Environment aggregate.

use std::future::Future;

use flaps_domain::{Environment, EnvironmentKey, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Environment`] aggregates scoped to a project.
pub trait EnvironmentRepository: Send + Sync {
    /// Inserts or fully replaces the environment within `project`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    fn upsert_environment(
        &self,
        actor: &str,
        project: &ProjectKey,
        env: &Environment,
    ) -> impl Future<Output = StoreResult<()>> + Send;

    /// Returns the environment for `key` within `project`, or `None`.
    fn get_environment(
        &self,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> impl Future<Output = StoreResult<Option<Environment>>> + Send;

    /// Returns all environments for `project` in insertion order.
    fn list_environments(
        &self,
        project: &ProjectKey,
    ) -> impl Future<Output = StoreResult<Vec<Environment>>> + Send;

    /// Deletes the environment identified by `project` + `key`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the environment does not exist this is a no-op and
    /// no audit entry is written.
    fn delete_environment(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> impl Future<Output = StoreResult<()>> + Send;
}
