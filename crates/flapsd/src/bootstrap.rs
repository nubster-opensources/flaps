//! Boot-time helpers: store connection with retry, cache warm-up, admin bootstrap.
//!
//! # Graceful shutdown and SSE streams (AC#4)
//!
//! The daemon uses `axum::serve(...).with_graceful_shutdown(ctrl_c())` to stop
//! accepting new connections when `CTRL+C` is received. Once `main` returns, the
//! `broadcast::Sender<SyncEvent>` held in `AppState` is dropped. Every active SSE
//! handler that holds a `broadcast::Receiver` sees `RecvError::Closed`, terminates
//! its stream, and closes the HTTP response body. This ensures all SSE clients
//! disconnect cleanly without requiring explicit fan-out shutdown logic.

use std::{future::Future, pin::Pin, time::Duration};

use anyhow::{Context as _, Result};
use flaps_server::{
    recompile::recompile_environment,
    state::{AppState, Store},
};
use flaps_store::StoreError;
use tracing::{debug, error, warn};

/// Number of store connection attempts before giving up.
const MAX_CONNECT_ATTEMPTS: u32 = 10;

/// Initial backoff delay between connection attempts.
const BACKOFF_BASE_MS: u64 = 500;

/// Maximum backoff delay, used as the ceiling for exponential growth.
const BACKOFF_MAX_MS: u64 = 30_000;

/// An async closure type that attempts to produce a store, returning a [`Result`].
///
/// The closure returns a pinned, boxed future so it can be called from an async
/// context without blocking the executor. This removes the need for
/// `tokio::task::block_in_place` at the call site, which would panic on a
/// single-threaded runtime.
///
/// Used as a seam in tests to inject a controllable connector without adding a
/// new trait or a dependency on a concrete store type at the call site.
pub type ConnectorFn<S> =
    Box<dyn FnMut() -> Pin<Box<dyn Future<Output = Result<S>> + Send>> + Send>;

/// Attempts to connect to the store using `connector`, retrying with
/// exponential backoff on failure.
///
/// The delay starts at `base_ms` and doubles after each failure, capped at
/// `max_ms`. After `max_attempts` failures the last error is returned.
///
/// # Errors
/// Returns the last connection error when all attempts are exhausted.
async fn connect_store_with_retry_inner<S>(
    mut connector: ConnectorFn<S>,
    base_ms: u64,
    max_ms: u64,
    max_attempts: u32,
) -> Result<S> {
    let mut delay_ms = base_ms;
    let mut last_err = anyhow::anyhow!("no attempts made");

    for attempt in 1..=max_attempts {
        match connector().await {
            Ok(store) => return Ok(store),
            Err(e) => {
                warn!(attempt, max = max_attempts, error = %e, "store connection failed; retrying");
                last_err = e;
                if attempt < max_attempts {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms = (delay_ms * 2).min(max_ms);
                }
            }
        }
    }

    Err(last_err).context("exhausted all store connection attempts")
}

/// Attempts to connect to the store using `connector`, retrying with
/// exponential backoff on failure.
///
/// The delay starts at 500 ms and doubles after each failure,
/// capped at 30 000 ms. After 10 failures the last error is returned.
///
/// # Errors
/// Returns the last connection error when all attempts are exhausted.
pub async fn connect_store_with_retry<S>(connector: ConnectorFn<S>) -> Result<S> {
    connect_store_with_retry_inner(
        connector,
        BACKOFF_BASE_MS,
        BACKOFF_MAX_MS,
        MAX_CONNECT_ATTEMPTS,
    )
    .await
}

/// Warms up the compiled ruleset cache by compiling every (project, environment)
/// pair found in the store.
///
/// This is a **best-effort** operation: if an environment fails to compile (e.g.
/// because of inconsistent data), the error is logged and that environment is
/// skipped. The daemon still starts; the environment is served as 404 until the
/// data is corrected and the cache is refreshed by a mutation.
///
/// The entire warm-up pass completes **before** the HTTP listener is opened.
pub async fn warm_up_cache<S: Store>(state: &AppState<S>) {
    let projects = match state.store.list_projects().await {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "failed to list projects during cache warm-up; skipping");
            return;
        }
    };

    for project in &projects {
        let environments = match state.store.list_environments(&project.key).await {
            Ok(e) => e,
            Err(e) => {
                error!(project = %project.key.as_str(), error = %e, "failed to list environments; skipping project");
                continue;
            }
        };

        for env in &environments {
            match recompile_environment(state, &project.key, &env.key).await {
                Ok(()) => {
                    debug!(
                        project = %project.key.as_str(),
                        env = %env.key.as_str(),
                        "environment compiled"
                    );
                }
                Err(e) => {
                    error!(
                        project = %project.key.as_str(),
                        env = %env.key.as_str(),
                        error = ?e,
                        "environment failed to compile; serving 404 until fixed"
                    );
                }
            }
        }
    }
}

/// Generates a CSPRNG password of at least 24 URL-safe characters (>= 128 bits
/// of entropy from 18 random bytes encoded as 36 hex characters).
///
/// The password is printed to stdout **once** during first-boot by
/// [`bootstrap_admin_once`] and **never** stored in clear text, logged via
/// tracing, or written to any file.
fn generate_password() -> String {
    use argon2::password_hash::rand_core::RngCore as _;
    // 18 bytes -> 36 hex chars -> 144 bits of entropy (> 128-bit requirement).
    let mut bytes = [0u8; 18];
    argon2::password_hash::rand_core::OsRng.fill_bytes(&mut bytes);
    bytes.iter().fold(String::with_capacity(36), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Creates the initial admin account on first boot (idempotent).
///
/// Calls `store.create_account("system", username, &password)` where `password`
/// is freshly generated by CSPRNG. The outcome determines the branch:
///
/// - `Ok(_)`: first boot - the credentials are printed **once** to stdout.
/// - `Err(StoreError::Conflict(_))`: account already exists - nothing is printed.
/// - `Err(other)`: fatal - the error is propagated and the daemon refuses to start.
///
/// The generated password is **never** passed to the tracing framework or
/// written to any persistent store in clear text. The store hashes it with
/// argon2id before persisting.
///
/// # Errors
/// Returns an error when the store reports a failure other than a username conflict.
pub async fn bootstrap_admin_once<S: Store>(store: &S, username: &str) -> Result<()> {
    let password = generate_password();
    match store.create_account("system", username, &password).await {
        Ok(_) => {
            println!("flapsd: created initial admin account");
            println!("  username: {username}");
            println!("  password: {password}");
            println!("  store this now; it will not be shown again");
        }
        Err(StoreError::Conflict(_)) => {
            // Already initialised on a previous boot: do not re-print credentials.
        }
        Err(e) => {
            return Err(anyhow::Error::from(e)).context("failed to bootstrap admin account");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use flaps_domain::{
        Environment, EnvironmentKey, FlagEnvConfig, FlagKey, FlagType, ManagedBy, Project,
        ProjectKey, SdkKeyKind, SegmentKey, ServeTarget, TargetingRule, ValueType, VariantKey,
        VariantValue, Variants,
    };
    use flaps_server::{build_router, state::AppState};
    use flaps_store::{
        KeyHasher, NewSdkKey, SdkKeyScope,
        repository::{
            EnvironmentRepository as _, FlagEnvConfigRepository as _, FlagRepository as _,
            ProjectRepository as _, SdkKeyRepository as _,
        },
        sqlite::SqliteStore,
    };
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use super::*;

    async fn make_store() -> SqliteStore {
        SqliteStore::in_memory(KeyHasher::new(b"test-pepper-32-bytes-minimum-len!"))
            .await
            .expect("in-memory store")
    }

    // -- warm_up_cache --

    #[tokio::test]
    async fn warm_up_cache_compiles_valid_env_and_skips_none() {
        let store = make_store().await;

        // Project 1: one valid environment.
        store
            .upsert_project(
                "test",
                &Project {
                    key: ProjectKey::new("proj-a").unwrap(),
                    name: "A".into(),
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
                &ProjectKey::new("proj-a").unwrap(),
                &Environment {
                    key: EnvironmentKey::new("prod").unwrap(),
                    name: "Prod".into(),
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();

        let state = AppState::new(store);
        // Must not panic, must not return Err.
        warm_up_cache(&state).await;

        let cache = state.cache.read().await;
        assert!(
            cache.contains_key(&(
                ProjectKey::new("proj-a").unwrap(),
                EnvironmentKey::new("prod").unwrap()
            )),
            "valid env should be in cache after warm-up"
        );
    }

    #[tokio::test]
    async fn warm_up_cache_does_not_panic_on_empty_store() {
        let store = make_store().await;
        let state = AppState::new(store);
        // No projects: warm-up should be a no-op.
        warm_up_cache(&state).await;
        let cache = state.cache.read().await;
        assert!(cache.is_empty(), "cache should be empty");
    }

    /// Proves the best-effort contract: a valid environment is compiled into the
    /// cache while a corrupt one (flag config references a missing segment) is
    /// skipped. warm_up_cache must not panic and must not return Err.
    #[tokio::test]
    async fn warm_up_cache_skips_corrupt_env_and_keeps_valid_env() {
        let store = make_store().await;
        let project = ProjectKey::new("mixed").unwrap();

        store
            .upsert_project(
                "test",
                &Project {
                    key: project.clone(),
                    name: "Mixed".into(),
                    description: None,
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();

        // env-good: no flags, compiles to an empty ruleset (success).
        let good_env = EnvironmentKey::new("env-good").unwrap();
        store
            .upsert_environment(
                "test",
                &project,
                &Environment {
                    key: good_env.clone(),
                    name: "Good".into(),
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();

        // env-corrupt: has a FlagEnvConfig referencing a segment that does not
        // exist, which triggers CompileError::UnknownSegment.
        let bad_env = EnvironmentKey::new("env-corrupt").unwrap();
        store
            .upsert_environment(
                "test",
                &project,
                &Environment {
                    key: bad_env.clone(),
                    name: "Corrupt".into(),
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();

        // Seed a flag and a corrupt config (missing segment "ghost").
        let flag_key = FlagKey::new("feat").unwrap();
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
        store
            .upsert_flag(
                "test",
                &project,
                &flaps_domain::Flag {
                    key: flag_key.clone(),
                    name: "feat".into(),
                    description: None,
                    flag_type: FlagType::Release,
                    value_type: ValueType::Boolean,
                    variants,
                },
            )
            .await
            .unwrap();

        let corrupt_config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![SegmentKey::new("ghost").unwrap()],
                serve: ServeTarget::Fixed(vk_on),
            }],
            default_rule: ServeTarget::Fixed(vk_off),
        };
        store
            .upsert_flag_env_config("test", &project, &flag_key, &bad_env, &corrupt_config)
            .await
            .unwrap();

        let state = AppState::new(store);

        // warm_up_cache must not panic and must not propagate the error.
        warm_up_cache(&state).await;

        let cache = state.cache.read().await;

        // The valid environment must be warmed up.
        assert!(
            cache.contains_key(&(project.clone(), good_env)),
            "valid env must be in cache after warm-up"
        );

        // The corrupt environment must be skipped.
        assert!(
            !cache.contains_key(&(project, bad_env)),
            "corrupt env must not be inserted into cache"
        );
    }

    // -- bootstrap_admin_once --

    #[tokio::test]
    async fn bootstrap_admin_once_creates_account_on_first_boot() {
        let store = make_store().await;
        let result = bootstrap_admin_once(&store, "admin").await;
        assert!(result.is_ok(), "first boot should succeed: {result:?}");
        // The account must be verifiable.
        // We cannot recover the password (printed to stdout, not stored), so we
        // verify the account exists by checking that verify_credentials returns
        // None for a wrong password but Some for the right one. Since we cannot
        // know the right password, we at minimum confirm the account was written
        // (create_account would have returned Ok, not Conflict).
        // We do a second call to assert idempotency (Conflict is swallowed).
        let result2 = bootstrap_admin_once(&store, "admin").await;
        assert!(result2.is_ok(), "second call must not error: {result2:?}");
    }

    #[tokio::test]
    async fn bootstrap_admin_once_conflict_is_swallowed() {
        let store = make_store().await;
        // First call creates the account.
        bootstrap_admin_once(&store, "admin").await.unwrap();
        // Second call should silently swallow the Conflict.
        let result = bootstrap_admin_once(&store, "admin").await;
        assert!(result.is_ok(), "Conflict must be swallowed: {result:?}");
    }

    // -- generate_password --

    #[test]
    fn generated_password_meets_length_requirement() {
        // Cannot call generate_password() from outside the module directly
        // because it is private. We verify the property indirectly via the
        // bootstrap path: the password stored on first-boot must satisfy the
        // length constraint. Since we cannot recover it from the store, we
        // call the private function via a dedicated test accessor.
        let pwd = super::generate_password();
        assert!(
            pwd.len() >= 24,
            "password length {} is below 24-char minimum",
            pwd.len()
        );
    }

    #[test]
    fn generated_password_is_url_safe() {
        let pwd = super::generate_password();
        assert!(
            pwd.chars().all(|c| c.is_ascii_alphanumeric()),
            "password must be URL-safe (hex); got {pwd}"
        );
    }

    // -- connect_store_with_retry --

    #[tokio::test]
    async fn connect_store_with_retry_succeeds_after_n_failures() {
        use std::sync::{Arc, Mutex};

        let call_count = Arc::new(Mutex::new(0u32));
        let call_count_clone = Arc::clone(&call_count);

        // Fail the first 3 attempts, succeed on the 4th.
        let connector: ConnectorFn<u32> = Box::new(move || {
            let count_clone = Arc::clone(&call_count_clone);
            Box::pin(async move {
                let mut count = count_clone.lock().unwrap();
                *count += 1;
                if *count < 4 {
                    Err(anyhow::anyhow!("simulated connection failure"))
                } else {
                    Ok(42u32)
                }
            })
        });

        // Use base_ms = 0 so no real sleep occurs; verifies the full retry path
        // in well under a millisecond.
        let result = connect_store_with_retry_inner(connector, 0, 0, 10).await;

        assert!(result.is_ok(), "expected Ok(42), got {result:?}");
        assert_eq!(result.unwrap(), 42u32);
        assert_eq!(*call_count.lock().unwrap(), 4, "connector called 4 times");
    }

    /// Verifies that the connector returns Err when every attempt fails.
    ///
    /// Uses base_ms = 0 via the inner helper so no real sleep occurs; the test
    /// completes in milliseconds and does not need to be ignored.
    #[tokio::test]
    async fn connect_store_with_retry_returns_err_after_max_attempts() {
        let connector: ConnectorFn<u32> =
            Box::new(|| Box::pin(async { Err(anyhow::anyhow!("always fails")) }));

        let result = connect_store_with_retry_inner(connector, 0, 0, 10).await;
        assert!(result.is_err(), "expected Err after all attempts exhausted");
    }

    // -- AC#1: integration boot (warm-up + bootstrap + router serves warmed data) --

    /// Proves AC#1 end-to-end using an in-memory SQLite store:
    ///
    /// 1. Seed a project, environment, flag, SDK key and FlagEnvConfig in the store.
    /// 2. Run warm_up_cache so the compiled ruleset is loaded into the cache.
    /// 3. Run bootstrap_admin_once to create the admin account.
    /// 4. Build the router and exercise the OFREP single-flag evaluation endpoint
    ///    via tower::oneshot with the seeded SDK key.
    /// 5. Assert that the response status is 200 and the returned value comes from
    ///    the compiled ruleset (the flag is disabled, so the response reason is
    ///    DISABLED), proving the cache was consumed on the hot path.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn integration_boot_sqlite_warm_cache_and_router_serve_ofrep() {
        let store = make_store().await;
        let project_key = ProjectKey::new("boot-proj").unwrap();
        let env_key = EnvironmentKey::new("boot-env").unwrap();

        // Seed project.
        store
            .upsert_project(
                "system",
                &Project {
                    key: project_key.clone(),
                    name: "Boot project".into(),
                    description: None,
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();

        // Seed environment.
        store
            .upsert_environment(
                "system",
                &project_key,
                &Environment {
                    key: env_key.clone(),
                    name: "Boot env".into(),
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                },
            )
            .await
            .unwrap();

        // Seed a boolean flag with on/off variants.
        let flag_key = FlagKey::new("boot-flag").unwrap();
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
        store
            .upsert_flag(
                "system",
                &project_key,
                &flaps_domain::Flag {
                    key: flag_key.clone(),
                    name: "boot-flag".into(),
                    description: None,
                    flag_type: FlagType::Release,
                    value_type: ValueType::Boolean,
                    variants,
                },
            )
            .await
            .unwrap();

        // Seed a FlagEnvConfig: disabled flag with a fixed default (off).
        // A disabled flag is the simplest valid config that exercises the
        // full warm-up -> compile -> serve path without needing a targeting rule.
        let flag_env_config = FlagEnvConfig {
            enabled: false,
            rules: vec![],
            default_rule: ServeTarget::Fixed(vk_off),
        };
        store
            .upsert_flag_env_config(
                "system",
                &project_key,
                &flag_key,
                &env_key,
                &flag_env_config,
            )
            .await
            .unwrap();

        // Seed an SDK key so the OFREP endpoint can authenticate.
        let raw_key = "s-boot-integration-test-key-01234";
        let new_key = NewSdkKey {
            scope: SdkKeyScope {
                project_key: project_key.clone(),
                environment_key: env_key.clone(),
            },
            kind: SdkKeyKind::Server,
        };
        store
            .create_sdk_key("system", raw_key, &new_key)
            .await
            .unwrap();

        // Warm up and bootstrap.
        let state = AppState::new(store);
        warm_up_cache(&state).await;
        bootstrap_admin_once(&state.store, "admin").await.unwrap();

        // The cache must contain the compiled ruleset for the seeded env.
        {
            let cache = state.cache.read().await;
            assert!(
                cache.contains_key(&(project_key.clone(), env_key.clone())),
                "cache must contain the compiled ruleset after warm-up"
            );
        }

        // Build the router and exercise the OFREP endpoint.
        let app = build_router(state);
        let req = Request::builder()
            .method("POST")
            .uri(format!("/ofrep/v1/evaluate/flags/{}", flag_key.as_str()))
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {raw_key}"))
            .body(Body::from(r#"{"context":{}}"#))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "OFREP evaluation of a warmed flag must return 200"
        );

        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // A disabled flag yields reason DISABLED and no value: this data comes
        // from the compiled ruleset inserted by warm_up_cache.
        assert_eq!(
            json["reason"].as_str(),
            Some("DISABLED"),
            "disabled flag from warmed cache must return reason DISABLED, got: {json}"
        );
        assert!(
            json["value"].is_null(),
            "disabled flag must omit value, got: {json}"
        );
    }

    // -- AC#4 shutdown: broadcast bus --

    #[tokio::test]
    async fn shutdown_broadcast_sender_drop_closes_receiver() {
        use flaps_server::sync::SyncEvent;
        use tokio::sync::broadcast;

        let (tx, mut rx) = broadcast::channel::<SyncEvent>(16);
        // Drop the sender to simulate the AppState being dropped on shutdown.
        drop(tx);
        // The receiver must observe RecvError::Closed.
        let result = rx.recv().await;
        assert!(
            matches!(
                result,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ),
            "receiver must see Closed when sender is dropped"
        );
    }
}
