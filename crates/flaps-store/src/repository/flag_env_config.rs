//! Repository trait for the [`FlagEnvConfig`](flaps_domain::FlagEnvConfig) aggregate.

use flaps_domain::{EnvironmentKey, FlagEnvConfig, FlagKey, ProjectKey};

use crate::error::StoreResult;

/// Async CRUD operations for [`FlagEnvConfig`] aggregates.
#[allow(async_fn_in_trait)]
pub trait FlagEnvConfigRepository {
    /// Inserts or fully replaces the per-environment flag configuration.
    async fn upsert_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
        config: &FlagEnvConfig,
    ) -> StoreResult<()>;

    /// Returns the config for `(project, flag, environment)`, or `None`.
    async fn get_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> StoreResult<Option<FlagEnvConfig>>;

    /// Deletes the config identified by `(project, flag, environment)`.
    async fn delete_flag_env_config(
        &self,
        project: &ProjectKey,
        flag: &FlagKey,
        environment: &EnvironmentKey,
    ) -> StoreResult<()>;
}
