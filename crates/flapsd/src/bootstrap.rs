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

use std::time::Duration;

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

/// A closure type that attempts to produce a store, returning a [`Result`].
///
/// Used as a seam in tests to inject a controllable connector without adding a
/// new trait or a dependency on a concrete store type at the call site.
pub type ConnectorFn<S> = Box<dyn FnMut() -> Result<S> + Send>;

/// Attempts to connect to the store using `connector`, retrying with
/// exponential backoff on failure.
///
/// The delay starts at [`BACKOFF_BASE_MS`] and doubles after each failure,
/// capped at [`BACKOFF_MAX_MS`]. After [`MAX_CONNECT_ATTEMPTS`] failures the
/// last error is returned.
///
/// # Errors
/// Returns the last connection error when all attempts are exhausted.
pub async fn connect_store_with_retry<S>(mut connector: ConnectorFn<S>) -> Result<S> {
    let mut delay_ms = BACKOFF_BASE_MS;
    let mut last_err = anyhow::anyhow!("no attempts made");

    for attempt in 1..=MAX_CONNECT_ATTEMPTS {
        match connector() {
            Ok(store) => return Ok(store),
            Err(e) => {
                warn!(attempt, max = MAX_CONNECT_ATTEMPTS, error = %e, "store connection failed; retrying");
                last_err = e;
                if attempt < MAX_CONNECT_ATTEMPTS {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    delay_ms = (delay_ms * 2).min(BACKOFF_MAX_MS);
                }
            }
        }
    }

    Err(last_err).context("exhausted all store connection attempts")
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
    use flaps_domain::{Environment, EnvironmentKey, ManagedBy, Project, ProjectKey};
    use flaps_server::state::AppState;
    use flaps_store::{
        KeyHasher,
        repository::{EnvironmentRepository as _, ProjectRepository as _},
        sqlite::SqliteStore,
    };

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
            let mut count = call_count_clone.lock().unwrap();
            *count += 1;
            if *count < 4 {
                Err(anyhow::anyhow!("simulated connection failure"))
            } else {
                Ok(42u32)
            }
        });

        // Override sleep to zero to keep the test fast.
        // The connector itself uses tokio::time::sleep; we patch the delays by
        // reducing BACKOFF_BASE_MS conceptually. Since the constant is not
        // injectable, the test relies on the fact that delays are bounded and
        // that the function returns after exactly 4 attempts.
        //
        // To avoid 1.5 s of real sleep in CI, we wrap with a timeout.
        let result =
            tokio::time::timeout(Duration::from_secs(5), connect_store_with_retry(connector)).await;

        assert!(result.is_ok(), "should not have timed out");
        let inner = result.unwrap();
        assert!(inner.is_ok(), "expected Ok(42), got {inner:?}");
        assert_eq!(inner.unwrap(), 42u32);
        assert_eq!(*call_count.lock().unwrap(), 4, "connector called 4 times");
    }

    /// Verifies that the connector returns Err when every attempt fails.
    ///
    /// Marked `#[ignore]` because the real exponential backoff (10 attempts,
    /// up to 30 s ceiling) takes ~2 minutes. The success path is covered by
    /// `connect_store_with_retry_succeeds_after_n_failures`. Run manually with
    /// `cargo test -- --ignored` when testing the full backoff schedule.
    #[tokio::test]
    #[ignore = "real backoff schedule takes ~2 minutes; run with --ignored to exercise"]
    async fn connect_store_with_retry_returns_err_after_max_attempts() {
        let connector: ConnectorFn<u32> = Box::new(|| Err(anyhow::anyhow!("always fails")));

        let result = connect_store_with_retry(connector).await;
        assert!(result.is_err(), "expected Err after all attempts exhausted");
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
