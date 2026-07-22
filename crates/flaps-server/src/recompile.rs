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

    // Resolve the environment's own metadata (flag-set level), overlay-aware.
    // This is a single extra read per environment being compiled (not per
    // flag), so it does not introduce an N+1 query pattern: the overlay case
    // (`UpsertEnvironment`) needs no read at all, and every other change kind
    // reads the environment exactly once, same as the flags/segments reads
    // above.
    let environment_metadata = match change {
        Change::UpsertEnvironment(env) if env.key == *environment => env.metadata.clone(),
        _ => state
            .store
            .get_environment(project, environment)
            .await
            .map_err(ApiError::from)?
            .map(|env| env.metadata)
            .unwrap_or_default(),
    };

    // Get previous compiled ruleset for version monotonicity.
    let cache = state.cache.read().await;
    let previous = cache.get(&(project.clone(), environment.clone()));

    compile_environment(
        environment,
        &flag_config_refs,
        &segment_lookup,
        &environment_metadata,
        previous,
    )
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

/// Compiles a single environment and installs it into the cache.
///
/// Reads flags, segments and per-environment flag configurations from the store
/// for the given `(project, environment)` pair, compiles the ruleset, and
/// writes it to the in-memory cache.
///
/// Returns `Err` when the environment cannot be compiled (e.g. a flag config
/// references a segment that does not exist). The cache is left unchanged on
/// failure, so the environment is served as 404 until the data is fixed.
///
/// This function is used during daemon startup to warm up the cache on a
/// per-environment basis (best-effort: a failure in one environment does not
/// prevent the others from loading).
pub async fn recompile_environment<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    environment: &EnvironmentKey,
) -> Result<(), ApiError> {
    let ruleset =
        compile_env_with_overlay(state, project, environment, &Change::UpsertProject).await?;
    install_in_cache(state, project, vec![ruleset]).await;
    Ok(())
}

/// Recompiles `affected` environments directly from the just-committed store
/// state and installs each into the cache (the #105 fix): the cache always
/// ends up reflecting the last committed mutation, never a pre-write
/// snapshot that a differently-ordered concurrent write could make stale.
///
/// # Ordering requirement
///
/// `affected` must be computed with [`affected_environments`] (directly, or
/// via the environment keys on the [`CompiledRuleset`]s returned by
/// [`validate_by_compiling`]) called **before** the store write it follows,
/// not derived by re-querying the store afterwards. Some writes cascade:
/// deleting a flag also deletes its `flag_env_config` rows, which is exactly
/// the evidence a post-write [`affected_environments`] call for
/// [`Change::DeleteFlag`] would need to discover which environments were
/// affected. Querying it after the delete would return an empty set and
/// silently leave those environments' cached rulesets stale.
///
/// Callers must hold the caller's per-project mutation lock
/// ([`AppState::lock_project`](crate::state::AppState::lock_project)) across
/// the write and this call, so no other in-scope mutation for the same
/// project can install a conflicting ruleset in between.
pub async fn recompile_committed<S: Store>(
    state: &AppState<S>,
    project: &ProjectKey,
    affected: &[EnvironmentKey],
) -> Result<(), ApiError> {
    for environment in affected {
        recompile_environment(state, project, environment).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use flaps_domain::{
        Environment, FlagEnvConfig, FlagKey, FlagType, ManagedBy, Project, SegmentKey, ServeTarget,
        TargetingRule, ValueType, VariantKey, VariantValue, Variants,
    };
    use flaps_store::{
        KeyHasher,
        repository::{
            EnvironmentRepository as _, FlagEnvConfigRepository as _, FlagRepository as _,
            ProjectRepository as _,
        },
        sqlite::SqliteStore,
    };

    use super::*;

    async fn make_store() -> SqliteStore {
        SqliteStore::in_memory(KeyHasher::new(b"test-pepper-32-bytes-long-enough"))
            .await
            .expect("in-memory store")
    }

    /// Creates a minimal boolean flag (on/off) in the store for the given project.
    async fn seed_bool_flag(
        store: &SqliteStore,
        project: &ProjectKey,
        flag_key: &str,
    ) -> flaps_domain::Flag {
        let key = FlagKey::new(flag_key).unwrap();
        let vk_on = VariantKey::new("on").unwrap();
        let vk_off = VariantKey::new("off").unwrap();
        let variants = Variants::new(
            ValueType::Boolean,
            [
                (vk_on.clone(), VariantValue::Bool(true)),
                (vk_off.clone(), VariantValue::Bool(false)),
            ],
        )
        .unwrap();
        let flag = flaps_domain::Flag {
            key: key.clone(),
            name: flag_key.to_owned(),
            description: None,
            flag_type: FlagType::Release,
            value_type: ValueType::Boolean,
            variants,
            metadata: flaps_domain::Metadata::new(),
        };
        store.upsert_flag("test", project, &flag).await.unwrap();
        flag
    }

    #[tokio::test]
    async fn recompile_environment_success_populates_cache() {
        let store = make_store().await;
        // Create a project and one environment with no flags.
        store
            .upsert_project(
                "test",
                &Project {
                    key: ProjectKey::new("proj").unwrap(),
                    name: "Proj".into(),
                    description: None,
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();
        store
            .upsert_environment(
                "test",
                &ProjectKey::new("proj").unwrap(),
                &Environment {
                    key: EnvironmentKey::new("prod").unwrap(),
                    name: "Prod".into(),
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                    metadata: flaps_domain::Metadata::new(),
                },
            )
            .await
            .unwrap();

        let state = AppState::new(store);
        let project = ProjectKey::new("proj").unwrap();
        let environment = EnvironmentKey::new("prod").unwrap();

        let result = recompile_environment(&state, &project, &environment).await;

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let cache = state.cache.read().await;
        assert!(
            cache.contains_key(&(project, environment)),
            "cache should contain the compiled ruleset"
        );
    }

    #[tokio::test]
    async fn recompile_environment_error_leaves_cache_unchanged() {
        // Build a genuine corrupt state: a FlagEnvConfig whose targeting rule
        // references a segment that does not exist in the store. When the
        // compiler tries to resolve the segment key it emits CompileError::UnknownSegment
        // which propagates as ApiError::Validation, causing recompile_environment
        // to return Err without touching the cache.
        let store = make_store().await;
        let project = ProjectKey::new("proj-corrupt").unwrap();
        let env_key = EnvironmentKey::new("staging").unwrap();

        // Seed project and environment.
        store
            .upsert_project(
                "test",
                &Project {
                    key: project.clone(),
                    name: "Corrupt project".into(),
                    description: None,
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();
        store
            .upsert_environment(
                "test",
                &project,
                &Environment {
                    key: env_key.clone(),
                    name: "Staging".into(),
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                    metadata: flaps_domain::Metadata::new(),
                },
            )
            .await
            .unwrap();

        // Seed a flag.
        let flag = seed_bool_flag(&store, &project, "my-flag").await;

        // Write a FlagEnvConfig that references a segment that does NOT exist.
        // The segment key "ghost-segment" is never inserted into the store, so
        // list_segments returns an empty vec and compile_environment returns
        // Err(CompileError::UnknownSegment { .. }).
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![SegmentKey::new("ghost-segment").unwrap()],
                serve: ServeTarget::Fixed(VariantKey::new("on").unwrap()),
            }],
            default_rule: ServeTarget::Fixed(VariantKey::new("off").unwrap()),
        };
        store
            .upsert_flag_env_config("test", &project, &flag.key, &env_key, &config)
            .await
            .unwrap();

        let state = AppState::new(store);

        // recompile_environment must fail because "ghost-segment" is absent.
        let result = recompile_environment(&state, &project, &env_key).await;
        assert!(result.is_err(), "expected Err(UnknownSegment), got Ok");

        // The cache must NOT contain an entry for this environment.
        let cache = state.cache.read().await;
        assert!(
            !cache.contains_key(&(project, env_key)),
            "cache must remain empty when compilation fails"
        );
    }

    /// `recompile_committed` recompiles every listed environment directly
    /// from current store content, discarding whatever a stale cache entry
    /// held -- the #105 fix's core promise: the cache always ends up
    /// reflecting the last committed store state, never a pre-write snapshot.
    #[tokio::test]
    async fn recompile_committed_replaces_stale_cache_with_committed_store_state() {
        let store = make_store().await;
        let project = ProjectKey::new("proj").unwrap();
        let env_a = EnvironmentKey::new("env-a").unwrap();
        let env_b = EnvironmentKey::new("env-b").unwrap();

        store
            .upsert_project(
                "test",
                &Project {
                    key: project.clone(),
                    name: "Proj".into(),
                    description: None,
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();
        for env_key in [&env_a, &env_b] {
            store
                .upsert_environment(
                    "test",
                    &project,
                    &Environment {
                        key: env_key.clone(),
                        name: env_key.as_str().to_owned(),
                        external_ref: None,
                        managed_by: ManagedBy::Local,
                        metadata: flaps_domain::Metadata::new(),
                    },
                )
                .await
                .unwrap();
        }
        let flag = seed_bool_flag(&store, &project, "my-flag").await;
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![],
            default_rule: ServeTarget::Fixed(VariantKey::new("on").unwrap()),
        };
        store
            .upsert_flag_env_config("test", &project, &flag.key, &env_a, &config)
            .await
            .unwrap();
        store
            .upsert_flag_env_config("test", &project, &flag.key, &env_b, &config)
            .await
            .unwrap();

        let state = AppState::new(store);

        // Seed a deliberately stale cache entry for env_a: a lower version
        // and a document that does not match current store content.
        install_in_cache(
            &state,
            &project,
            vec![CompiledRuleset {
                environment: env_a.clone(),
                document: "{}".to_owned(),
                content_hash: "stale-hash".to_owned(),
                version: 1,
            }],
        )
        .await;

        let result = recompile_committed(&state, &project, std::slice::from_ref(&env_a)).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");

        let cache = state.cache.read().await;
        let recompiled = cache
            .get(&(project.clone(), env_a.clone()))
            .expect("env_a must be present after recompile_committed");
        assert_ne!(
            recompiled.content_hash, "stale-hash",
            "recompile_committed must replace the stale document with committed store state"
        );
        // env_b was never listed in `affected`, so it must remain absent.
        assert!(
            !cache.contains_key(&(project.clone(), env_b.clone())),
            "recompile_committed must not touch environments outside `affected`"
        );
    }
}
