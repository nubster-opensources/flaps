//! Repository trait for the Flag aggregate.

use std::future::Future;

use flaps_domain::{Flag, FlagKey, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`Flag`] aggregates scoped to a project.
pub trait FlagRepository: Send + Sync {
    /// Inserts or fully replaces the flag within `project`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    fn upsert_flag(
        &self,
        actor: &str,
        project: &ProjectKey,
        flag: &Flag,
    ) -> impl Future<Output = StoreResult<()>> + Send;

    /// Returns the flag for `key` within `project`, or `None`.
    fn get_flag(
        &self,
        project: &ProjectKey,
        key: &FlagKey,
    ) -> impl Future<Output = StoreResult<Option<Flag>>> + Send;

    /// Returns all flags for `project` in insertion order.
    fn list_flags(
        &self,
        project: &ProjectKey,
    ) -> impl Future<Output = StoreResult<Vec<Flag>>> + Send;

    /// Deletes the flag identified by `project` + `key`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the flag does not exist this is a no-op and no
    /// audit entry is written.
    fn delete_flag(
        &self,
        actor: &str,
        project: &ProjectKey,
        key: &FlagKey,
    ) -> impl Future<Output = StoreResult<()>> + Send;
}
