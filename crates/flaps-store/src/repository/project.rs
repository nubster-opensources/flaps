//! Repository trait for the Project aggregate.

use std::future::Future;

use flaps_domain::{Project, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Project`] aggregates.
pub trait ProjectRepository: Send + Sync {
    /// Inserts or fully replaces the project identified by its key.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    fn upsert_project(
        &self,
        actor: &str,
        project: &Project,
    ) -> impl Future<Output = StoreResult<()>> + Send;

    /// Returns the project for `key`, or `None` if it does not exist.
    fn get_project(
        &self,
        key: &ProjectKey,
    ) -> impl Future<Output = StoreResult<Option<Project>>> + Send;

    /// Returns all projects in insertion order.
    fn list_projects(&self) -> impl Future<Output = StoreResult<Vec<Project>>> + Send;

    /// Deletes the project identified by `key`. Cascades to all child entities.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the project does not exist this is a no-op and no
    /// audit entry is written.
    fn delete_project(
        &self,
        actor: &str,
        key: &ProjectKey,
    ) -> impl Future<Output = StoreResult<()>> + Send;
}
