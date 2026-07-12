//! Repository trait for the [`FlagEnvConfig`] aggregate.

use std::future::Future;

use flaps_domain::{EnvironmentKey, FlagEnvConfig, FlagKey, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`FlagEnvConfig`] aggregates.
pub trait FlagEnvConfigRepository: Send + Sync {
    /// Inserts or fully replaces the per-environment flag configuration.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log.
    fn upsert_flag_env_config(
        &self,
        actor: &str,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
        config: &FlagEnvConfig,
    ) -> impl Future<Output = StoreResult<()>> + Send;

    /// Returns the config for `(project, flag, environment)`, or `None`.
    fn get_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> impl Future<Output = StoreResult<Option<FlagEnvConfig>>> + Send;

    /// Deletes the config identified by `(project, flag, environment)`.
    ///
    /// `actor` identifies the principal performing the mutation; it is recorded
    /// in the audit log. If the config does not exist this is a no-op and no
    /// audit entry is written.
    fn delete_flag_env_config(
        &self,
        actor: &str,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> impl Future<Output = StoreResult<()>> + Send;
}
