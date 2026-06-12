//! Repository trait for the Flag aggregate.

use flaps_domain::{Flag, FlagKey, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Flag`] aggregates scoped to a project.
#[allow(async_fn_in_trait)]
pub trait FlagRepository {
    /// Inserts or fully replaces the flag within `project`.
    async fn upsert_flag(&self, project: &ProjectKey, flag: &Flag) -> StoreResult<()>;

    /// Returns the flag for `key` within `project`, or `None`.
    async fn get_flag(&self, project: &ProjectKey, key: &FlagKey) -> StoreResult<Option<Flag>>;

    /// Returns all flags for `project` in insertion order.
    async fn list_flags(&self, project: &ProjectKey) -> StoreResult<Vec<Flag>>;

    /// Deletes the flag identified by `project` + `key`.
    async fn delete_flag(&self, project: &ProjectKey, key: &FlagKey) -> StoreResult<()>;
}
