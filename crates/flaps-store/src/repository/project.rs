//! Repository trait for the Project aggregate.

use flaps_domain::{Project, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Project`] aggregates.
#[allow(async_fn_in_trait)]
pub trait ProjectRepository {
    /// Inserts or fully replaces the project identified by its key.
    async fn upsert_project(&self, project: &Project) -> StoreResult<()>;

    /// Returns the project for `key`, or `None` if it does not exist.
    async fn get_project(&self, key: &ProjectKey) -> StoreResult<Option<Project>>;

    /// Returns all projects in insertion order.
    async fn list_projects(&self) -> StoreResult<Vec<Project>>;

    /// Deletes the project identified by `key`. Cascades to all child entities.
    async fn delete_project(&self, key: &ProjectKey) -> StoreResult<()>;
}
