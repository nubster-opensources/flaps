//! Repository trait for the Environment aggregate.

use flaps_domain::{Environment, EnvironmentKey, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Environment`] aggregates scoped to a project.
#[allow(async_fn_in_trait)]
pub trait EnvironmentRepository {
    /// Inserts or fully replaces the environment within `project`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    async fn upsert_environment(
        &self,
        actor: &str,
        project: &ProjectKey,
        env: &Environment,
    ) -> StoreResult<()>;

    /// Returns the environment for `key` within `project`, or `None`.
    async fn get_environment(
        &self,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> StoreResult<Option<Environment>>;

    /// Returns all environments for `project` in insertion order.
    async fn list_environments(&self, project: &ProjectKey) -> StoreResult<Vec<Environment>>;

    /// Deletes the environment identified by `project` + `key`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the environment does not exist this is a no-op and
    /// no audit entry is written.
    async fn delete_environment(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &EnvironmentKey,
    ) -> StoreResult<()>;
}
