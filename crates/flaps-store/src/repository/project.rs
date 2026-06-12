//! Repository trait for the Project aggregate.

use flaps_domain::{Project, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Project`] aggregates.
#[allow(async_fn_in_trait)]
pub trait ProjectRepository {
    /// Inserts or fully replaces the project identified by its key.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    async fn upsert_project(&self, actor: &str, project: &Project) -> StoreResult<()>;

    /// Returns the project for `key`, or `None` if it does not exist.
    async fn get_project(&self, key: &ProjectKey) -> StoreResult<Option<Project>>;

    /// Returns all projects in insertion order.
    async fn list_projects(&self) -> StoreResult<Vec<Project>>;

    /// Deletes the project identified by `key`. Cascades to all child entities.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the project does not exist this is a no-op and no
    /// audit entry is written.
    async fn delete_project(&self, actor: &str, key: &ProjectKey) -> StoreResult<()>;
}
