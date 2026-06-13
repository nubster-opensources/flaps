//! Transactional write seam for atomic multi-mutation sessions.
//!
//! A [`WriteSession`] bundles several mutations into a single database
//! transaction. The transaction is committed by calling [`WriteSession::commit`]
//! and is rolled back automatically when the session is dropped without committing.
//!
//! Each mutation in the session is attributed to the `actor` supplied to
//! [`TransactionalStore::begin`]. Audit entries are appended within the same
//! transaction as the mutation they describe.

use std::future::Future;

use flaps_domain::{
    Environment, EnvironmentKey, Flag, FlagEnvConfig, FlagKey, Project, ProjectKey, Segment,
};

use crate::error::StoreResult;

/// A store that can open a write session spanning multiple mutations atomically.
pub trait TransactionalStore: Send + Sync {
    /// The concrete session type returned by [`begin`](Self::begin).
    type Session<'a>: WriteSession
    where
        Self: 'a;

    /// Begins a transactional write session attributed to `actor`.
    ///
    /// Every mutation performed on the returned [`WriteSession`] is attributed
    /// to `actor` in the audit log. The transaction is committed by calling
    /// [`WriteSession::commit`] and is rolled back automatically on drop.
    fn begin(&self, actor: &str) -> impl Future<Output = StoreResult<Self::Session<'_>>> + Send;
}

/// A set of mutations bound to one database transaction.
///
/// Dropping without calling [`commit`](Self::commit) rolls back the transaction.
#[allow(async_fn_in_trait)]
pub trait WriteSession {
    /// Inserts or fully replaces the project within the transaction.
    async fn upsert_project(&mut self, project: &Project) -> StoreResult<()>;

    /// Inserts or fully replaces the environment within the transaction.
    async fn upsert_environment(
        &mut self,
        project: &ProjectKey,
        env: &Environment,
    ) -> StoreResult<()>;

    /// Inserts or fully replaces the flag within the transaction.
    async fn upsert_flag(&mut self, project: &ProjectKey, flag: &Flag) -> StoreResult<()>;

    /// Inserts or fully replaces the segment within the transaction.
    async fn upsert_segment(&mut self, project: &ProjectKey, segment: &Segment) -> StoreResult<()>;

    /// Inserts or fully replaces the per-environment flag configuration within the transaction.
    async fn upsert_flag_env_config(
        &mut self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
        config: &FlagEnvConfig,
    ) -> StoreResult<()>;

    /// Commits the transaction, consuming the session.
    async fn commit(self) -> StoreResult<()>;
}
