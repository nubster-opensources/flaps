//! Compile-as-validation: assemble, compile, and cache compiled rulesets.

use std::collections::HashMap;

use flaps_compiler::{
    CompiledRuleset, FlagConfig, Segments, compile_environment, environments_referencing_segment,
};
use flaps_domain::{
    Environment, EnvironmentKey, Flag, FlagEnvConfig, FlagKey, ProjectKey, Segment, SegmentKey,
};

use crate::{error::ApiError, state::AppState, state::Store, sync::SyncEvent};

// ---------------------------------------------------------------------------
// Change overlay
// ---------------------------------------------------------------------------

/// The pending change applied in memory before compiling, for validation.
pub enum Change<'a> {
    /// An upsert of a flag (new or updated variants/config).
    UpsertFlag(&'a Flag),
    /// A deletion of a flag.
    DeleteFlag(&'a FlagKey),
    /// An upsert of a segment (new or updated match expression).
    UpsertSegment(&'a Segment),
    /// A deletion of a segment.
    DeleteSegment(&'a SegmentKey),
    /// An upsert of a per-environment flag configuration.
    UpsertFlagEnvConfig {
        /// Key of the flag whose config is being upserted.
        flag: &'a FlagKey,
        /// Key of the environment this config belongs to.
        environment: &'a EnvironmentKey,
        /// The new configuration value.
        config: &'a FlagEnvConfig,
    },
    /// A deletion of a per-environment flag configuration.
    DeleteFlagEnvConfig {
        /// Key of the flag whose config is being deleted.
        flag: &'a FlagKey,
        /// Key of the environment this config belongs to.
        environment: &'a EnvironmentKey,
    },
    /// An upsert of an environment (no rules to compile, recompile existing envs).
    UpsertEnvironment(&'a Environment),
    /// A deletion of an environment (removes it from the cache).
    DeleteEnvironment(&'a EnvironmentKey),
    /// An upsert of a project (recompile its existing envs without change).
    UpsertProject,
    /// A deletion of a project (removes all its envs from the cache).
    DeleteProject,
}

// ---------------------------------------------------------------------------
// Assemble helpers
// ---------------------------------------------------------------------------

/// Reads all flags, their per-env configs, and all segments for a project and
/// environment from the store, applies the overlay, then compiles.
async fn compile_env_with_overlay<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    environment: &EnvironmentKey,
    change: &Change<'_>,
) -> Result<CompiledRuleset, ApiError> {
    // Read all flags for the project.
    let mut flags = state
        .store
        .list_flags(project)
        .await
        .map_err(ApiError::from)?;

    // Apply flag-level overlay.
    match change {
        Change::UpsertFlag(flag) => {
            if let Some(pos) = flags.iter().position(|f| f.key == flag.key) {
                flags[pos] = (*flag).clone();
            } else {
                flags.push((*flag).clone());
            }
        }
        Change::DeleteFlag(key) => {
            flags.retain(|f| &f.key != *key);
        }
        _ => {}
    }

    // Read all segments for the project.
    let mut segments = state
        .store
        .list_segments(project)
        .await
        .map_err(ApiError::from)?;

    // Apply segment-level overlay.
    match change {
        Change::UpsertSegment(seg) => {
            if let Some(pos) = segments.iter().position(|s| s.key == seg.key) {
                segments[pos] = (*seg).clone();
            } else {
                segments.push((*seg).clone());
            }
        }
        Change::DeleteSegment(key) => {
            segments.retain(|s| &s.key != *key);
        }
        _ => {}
    }

    // Build the Segments lookup from the (possibly-mutated) segment list.
    let segment_lookup = Segments::new(segments.iter().map(|s| (s.key.clone(), &s.match_expr)));

    // Build FlagConfig slice: flags that have a config in this environment.
    let mut flag_configs: Vec<(Flag, FlagEnvConfig)> = Vec::new();

    for flag in &flags {
        // Determine the config for this (flag, environment), applying the overlay.
        let config = match change {
            Change::UpsertFlagEnvConfig {
                flag: overlay_flag,
                environment: overlay_env,
                config,
            } if *overlay_flag == &flag.key && *overlay_env == environment => {
                Some((*config).clone())
            }
            Change::DeleteFlagEnvConfig {
                flag: overlay_flag,
                environment: overlay_env,
            } if *overlay_flag == &flag.key && *overlay_env == environment => None,
            _ => {
                // Read from store.
                state
                    .store
                    .get_flag_env_config(project, &flag.key, environment)
                    .await
                    .map_err(ApiError::from)?
            }
        };

        if let Some(cfg) = config {
            flag_configs.push((flag.clone(), cfg));
        }
    }

    // Borrow the flag_configs as FlagConfig slices.
    let flag_config_refs: Vec<FlagConfig<'_>> = flag_configs
        .iter()
        .map(|(f, c)| FlagConfig { flag: f, config: c })
        .collect();

    // Get previous compiled ruleset for version monotonicity.
    let cache = state.cache.read().await;
    let previous = cache.get(&(project.clone(), environment.clone()));

    compile_environment(environment, &flag_config_refs, &segment_lookup, previous)
        .map_err(ApiError::Validation)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Returns the environment keys that have a config for `flag_key`.
async fn envs_with_flag_config<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    flag_key: &FlagKey,
) -> Result<Vec<EnvironmentKey>, ApiError> {
    let envs = state
        .store
        .list_environments(project)
        .await
        .map_err(ApiError::from)?;
    let mut affected = Vec::new();
    for env in envs {
        let config = state
            .store
            .get_flag_env_config(project, flag_key, &env.key)
            .await
            .map_err(ApiError::from)?;
        if config.is_some() {
            affected.push(env.key);
        }
    }
    Ok(affected)
}

/// Reads all (env, flag, config) triples for a project and returns the
/// environment keys whose flags reference `segment_key`.
async fn envs_referencing_seg<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    segment_key: &SegmentKey,
) -> Result<Vec<EnvironmentKey>, ApiError> {
    let envs = state
        .store
        .list_environments(project)
        .await
        .map_err(ApiError::from)?;
    let flags = state
        .store
        .list_flags(project)
        .await
        .map_err(ApiError::from)?;

    // Collect (env, flag, config) triples; lifetimes require separate storage.
    let mut all_configs: Vec<(EnvironmentKey, Flag, FlagEnvConfig)> = Vec::new();
    for env in &envs {
        for flag in &flags {
            if let Some(cfg) = state
                .store
                .get_flag_env_config(project, &flag.key, &env.key)
                .await
                .map_err(ApiError::from)?
            {
                all_configs.push((env.key.clone(), flag.clone(), cfg));
            }
        }
    }

    let mut by_env: HashMap<EnvironmentKey, Vec<FlagConfig<'_>>> = HashMap::new();
    for (env_key, flag, config) in &all_configs {
        by_env
            .entry(env_key.clone())
            .or_default()
            .push(FlagConfig { flag, config });
    }

    let affected = environments_referencing_segment(segment_key, &by_env);
    Ok(affected.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns the environments affected by a mutation described by `change`.
///
/// The affected set determines which environments need recompilation.
pub async fn affected_environments<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    change: &Change<'_>,
) -> Result<Vec<EnvironmentKey>, ApiError> {
    match change {
        Change::UpsertEnvironment(env) => Ok(vec![env.key.clone()]),
        Change::DeleteEnvironment(env_key) => Ok(vec![(*env_key).clone()]),
        Change::UpsertFlag(flag) => envs_with_flag_config(state, project, &flag.key).await,
        Change::DeleteFlag(flag_key) => envs_with_flag_config(state, project, flag_key).await,
        Change::UpsertFlagEnvConfig { environment, .. }
        | Change::DeleteFlagEnvConfig { environment, .. } => Ok(vec![(*environment).clone()]),
        Change::UpsertSegment(seg) => envs_referencing_seg(state, project, &seg.key).await,
        Change::DeleteSegment(seg_key) => envs_referencing_seg(state, project, seg_key).await,
        Change::UpsertProject | Change::DeleteProject => {
            let envs = state
                .store
                .list_environments(project)
                .await
                .map_err(ApiError::from)?;
            Ok(envs.into_iter().map(|e| e.key).collect())
        }
    }
}

/// Validates a proposed mutation by compiling the affected environments WITH
/// the change applied in memory, WITHOUT writing to the store.
///
/// Returns the compiled rulesets on success, a `Validation` error otherwise.
/// `DeleteEnvironment` and `DeleteProject` are no-ops (no rulesets to compile).
pub async fn validate_by_compiling<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    change: &Change<'_>,
) -> Result<Vec<CompiledRuleset>, ApiError> {
    // Deletions of environment/project do not involve rule compilation.
    if matches!(change, Change::DeleteEnvironment(_) | Change::DeleteProject) {
        return Ok(Vec::new());
    }

    let envs = affected_environments(state, project, change).await?;

    // For DeleteEnvironment, skip compilation (already handled above).
    let mut rulesets = Vec::new();
    for env_key in &envs {
        // UpsertEnvironment: if env is new, it has no flags yet; compile produces empty ruleset.
        let ruleset = compile_env_with_overlay(state, project, env_key, change).await?;
        rulesets.push(ruleset);
    }

    Ok(rulesets)
}

/// Installs freshly compiled rulesets into the cache (post-commit).
///
/// Each ruleset is inserted into the cache first; then a [`SyncEvent`] is
/// emitted on the broadcast channel. This ordering guarantees that any
/// subscriber receiving the event and immediately calling
/// `GET /sync/v1/ruleset` will observe the new version, never a stale one.
pub async fn install_in_cache<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    rulesets: Vec<CompiledRuleset>,
) {
    let mut cache = state.cache.write().await;
    for ruleset in rulesets {
        let environment = ruleset.environment.clone();
        let version = ruleset.version;
        cache.insert((project.clone(), environment.clone()), ruleset);
        // Emit after insert: ordering invariant documented in `crate::sync`.
        let _ = state.events.send(SyncEvent {
            project: project.clone(),
            environment,
            version,
        });
    }
}

/// Removes all cache entries for a project (used on project deletion).
pub async fn evict_project_from_cache<S: Store>(state: &AppState<S>, project: &ProjectKey) {
    let mut cache = state.cache.write().await;
    cache.retain(|(pk, _), _| pk != project);
}

/// Removes a single (project, environment) entry from the cache.
pub async fn evict_environment_from_cache<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    environment: &EnvironmentKey,
) {
    let mut cache = state.cache.write().await;
    cache.remove(&(project.clone(), environment.clone()));
}
