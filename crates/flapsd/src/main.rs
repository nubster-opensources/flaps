//! Flaps server daemon entry point.
//!
//! Parses `--config <path>`, initialises structured logging, connects to the
//! store with retry, warms up the compiled ruleset cache, bootstraps the admin
//! account on first boot, then starts the HTTP server with graceful shutdown.
//!
//! All heavy logic lives in [`flapsd_lib::bootstrap`] and [`flapsd_lib::config`]
//! so it can be unit-tested without spawning a real process.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use clap::Parser;
use flaps_server::{
    build_router,
    rate_limit::{RateLimitConfig, RateLimiter},
    state::{AppState, Store},
};
use flaps_store::{KeyHasher, sqlite::SqliteStore};
use tokio::net::TcpListener;

use flapsd_lib::{
    bootstrap::{bootstrap_admin_once, connect_store_with_retry, warm_up_cache},
    config::{Config, read_pepper},
};

/// Command-line arguments for `flapsd`.
#[derive(Debug, Parser)]
#[command(name = "flapsd", about = "Flaps feature flag server daemon")]
struct Args {
    /// Path to the TOML configuration file.
    #[arg(long, default_value = "flapsd.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialise structured logging from the RUST_LOG environment variable.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    run(args.config).await
}

/// Main boot sequence, extracted for testability.
///
/// # Boot order
///
/// 1. Load and validate TOML config.
/// 2. Read the HMAC pepper from `FLAPS_HMAC_PEPPER` (fail-closed if absent).
/// 3. Connect to the store with exponential-backoff retry.
/// 4. Warm up the compiled ruleset cache (best-effort per environment).
/// 5. Bootstrap the admin account on first boot (idempotent).
/// 6. Bind the TCP listener and start serving with graceful shutdown.
///
/// # Errors
/// Any step that fails returns an error; the process exits with a non-zero code.
pub async fn run(config_path: String) -> Result<()> {
    let config = Config::load(&config_path)
        .with_context(|| format!("failed to load config from {config_path:?}"))?;

    log_effective_config(&config);

    let pepper = read_pepper().context("pepper configuration")?;
    let hasher = KeyHasher::new(pepper);

    let url = config.database_url.clone();

    if url.starts_with("sqlite:") {
        let hasher_clone = hasher.clone();
        let url_sqlite = url.clone();
        let store = connect_store_with_retry(Box::new(move || {
            let hasher_inner = hasher_clone.clone();
            let url_inner = url_sqlite.clone();
            Box::pin(async move {
                SqliteStore::connect(&url_inner, hasher_inner)
                    .await
                    .map_err(anyhow::Error::from)
            })
        }))
        .await
        .context("connecting to SQLite store")?;

        let state = build_app_state(store, &config);
        boot(state, config).await
    } else {
        use flaps_store::postgres::PostgresStore;
        let hasher_clone = hasher.clone();
        let url_pg = url.clone();
        let store = connect_store_with_retry(Box::new(move || {
            let hasher_inner = hasher_clone.clone();
            let url_inner = url_pg.clone();
            Box::pin(async move {
                PostgresStore::connect(&url_inner, hasher_inner)
                    .await
                    .map_err(anyhow::Error::from)
            })
        }))
        .await
        .context("connecting to PostgreSQL store")?;

        let state = build_app_state(store, &config);
        boot(state, config).await
    }
}

/// Builds application state from the daemon configuration.
///
/// Applies [`Config::effective_rate_limit_per_minute`] to the SDK rate
/// limiter and [`Config::effective_session_ttl`] to the admin session TTL,
/// for both the SQLite and PostgreSQL storage backends. The login rate
/// limiter is not operator-configurable: it keeps the documented default
/// (see [`flaps_server::state::DEFAULT_LOGIN_RATE_LIMIT_CAPACITY`]).
fn build_app_state<S: Store>(store: S, config: &Config) -> AppState<S> {
    let rate_limit_per_minute = config.effective_rate_limit_per_minute();
    let rate_limiter = Arc::new(RateLimiter::new(RateLimitConfig {
        enabled: true,
        capacity: rate_limit_per_minute,
        refill_per_second: f64::from(rate_limit_per_minute) / 60.0,
    }));
    let login_rate_limiter = Arc::new(RateLimiter::new(RateLimitConfig {
        enabled: true,
        capacity: flaps_server::state::DEFAULT_LOGIN_RATE_LIMIT_CAPACITY,
        refill_per_second: flaps_server::state::DEFAULT_LOGIN_RATE_LIMIT_REFILL_PER_SECOND,
    }));

    AppState::with_config(
        store,
        rate_limiter,
        login_rate_limiter,
        config.effective_session_ttl(),
    )
}

/// Logs the effective, non-secret configuration values at startup.
///
/// Deliberately omits `database_url` (may embed PostgreSQL credentials) and
/// never touches the HMAC pepper, which is read separately from the
/// environment and is never stored on [`Config`].
fn log_effective_config(config: &Config) {
    tracing::info!(
        admin_username = %config.admin_username,
        rate_limit_per_minute = config.effective_rate_limit_per_minute(),
        session_ttl_secs = config.effective_session_ttl().as_secs(),
        "effective flapsd configuration"
    );
}

/// Completes the boot sequence once a store is connected.
async fn boot<S: flaps_server::state::Store>(
    state: flaps_server::state::AppState<S>,
    config: Config,
) -> Result<()> {
    warm_up_cache(&state).await;

    bootstrap_admin_once(&state.store, &config.admin_username)
        .await
        .context("bootstrapping admin account")?;

    let bind_addr = config.socket_addr().context("resolving bind address")?;
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("binding TCP listener on {bind_addr}"))?;

    tracing::info!(%bind_addr, "flapsd listening");

    axum::serve(listener, build_router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("received shutdown signal; draining connections");
        })
        .await
        .context("HTTP server error")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use flaps_domain::{Environment, EnvironmentKey, ManagedBy, Project, ProjectKey, SdkKeyKind};
    use flaps_store::{
        KeyHasher, NewSdkKey, SdkKeyScope,
        repository::{EnvironmentRepository as _, ProjectRepository as _, SdkKeyRepository as _},
        sqlite::SqliteStore,
    };
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    fn base_config(rate_limit_per_minute: Option<u32>, session_ttl_secs: Option<u64>) -> Config {
        Config {
            database_url: "sqlite://flaps.db".to_owned(),
            bind_addr: "127.0.0.1:8080".to_owned(),
            admin_username: "admin".to_owned(),
            rate_limit_per_minute,
            session_ttl_secs,
        }
    }

    async fn make_store() -> SqliteStore {
        SqliteStore::in_memory(KeyHasher::new(b"test-pepper-32-bytes-minimum-len!"))
            .await
            .expect("in-memory store")
    }

    /// Seeds a project, an environment and an SDK key, returning the raw key.
    async fn seed_sdk_key(store: &SqliteStore) -> String {
        let project_key = ProjectKey::new("proj").unwrap();
        let env_key = EnvironmentKey::new("env").unwrap();

        store
            .upsert_project(
                "system",
                &Project {
                    key: project_key.clone(),
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
                "system",
                &project_key,
                &Environment {
                    key: env_key.clone(),
                    name: "Env".into(),
                    external_ref: None,
                    managed_by: ManagedBy::Local,
                    metadata: flaps_domain::Metadata::new(),
                },
            )
            .await
            .unwrap();

        let raw_key = "s-flapsd-config-integration-test-key";
        store
            .create_sdk_key(
                "system",
                raw_key,
                &NewSdkKey {
                    scope: SdkKeyScope {
                        project_key,
                        environment_key: env_key,
                    },
                    kind: SdkKeyKind::Server,
                },
            )
            .await
            .unwrap();

        raw_key.to_owned()
    }

    // -- build_app_state: rate_limit_per_minute --

    /// Proves `rate_limit_per_minute` configures the SDK rate limiter through
    /// the actual router (AC: "rate_limit_per_minute configures the SDK rate
    /// limiter", "Integration tests prove non-default values are applied
    /// through the running router").
    #[tokio::test]
    async fn rate_limit_per_minute_is_enforced_through_router() {
        let store = make_store().await;
        let sdk_key = seed_sdk_key(&store).await;

        let config = base_config(Some(2), None);
        let state = build_app_state(store, &config);
        let app = build_router(state);

        let whoami_request = || {
            Request::builder()
                .method("GET")
                .uri("/sdk/whoami")
                .header("Authorization", format!("Bearer {sdk_key}"))
                .body(Body::empty())
                .unwrap()
        };

        for attempt in 1..=2 {
            let resp = app.clone().oneshot(whoami_request()).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "request {attempt} must be within the configured burst of 2"
            );
        }

        let resp = app.clone().oneshot(whoami_request()).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "request 3 must be throttled by the configured rate_limit_per_minute = 2"
        );
        assert!(
            resp.headers().contains_key("retry-after"),
            "429 must include a Retry-After header"
        );
    }

    /// Proves an omitted `rate_limit_per_minute` retains the documented
    /// default (60/minute): two quick requests must not be throttled.
    #[tokio::test]
    async fn omitted_rate_limit_retains_default_through_router() {
        let store = make_store().await;
        let sdk_key = seed_sdk_key(&store).await;

        let config = base_config(None, None);
        let state = build_app_state(store, &config);
        let app = build_router(state);

        for attempt in 1..=2 {
            let req = Request::builder()
                .method("GET")
                .uri("/sdk/whoami")
                .header("Authorization", format!("Bearer {sdk_key}"))
                .body(Body::empty())
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "request {attempt} must succeed under the default 60/minute limit"
            );
        }
    }

    // -- build_app_state: session_ttl_secs --

    /// Proves `session_ttl_secs` controls newly minted admin session
    /// expiration through the actual router: a session minted with a 1-second
    /// TTL must be rejected once it has expired.
    #[tokio::test]
    async fn session_ttl_secs_is_enforced_through_router() {
        let store = make_store().await;
        flaps_server::bootstrap_admin(&store, "admin", "admin-password")
            .await
            .expect("bootstrap admin");

        let config = base_config(None, Some(2));
        let state = build_app_state(store, &config);
        let app = build_router(state);

        let login_body = serde_json::json!({"username": "admin", "password": "admin-password"});
        let login_req = Request::builder()
            .method("POST")
            .uri("/login")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&login_body).unwrap()))
            .unwrap();
        let login_resp = app.clone().oneshot(login_req).await.unwrap();
        assert_eq!(login_resp.status(), StatusCode::OK, "login must succeed");
        let bytes = login_resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let token = json["token"].as_str().unwrap().to_owned();

        // Session valid immediately after login.
        let list_req = Request::builder()
            .method("GET")
            .uri("/projects")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(list_req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "session must be valid immediately after login"
        );

        // Wait past the configured 2-second TTL. The store truncates session
        // timestamps to whole seconds, so the margin must clear a full extra
        // second to stay deterministic under loaded CI.
        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;

        let list_req = Request::builder()
            .method("GET")
            .uri("/projects")
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(list_req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "session minted with a 2-second TTL must be expired after 2.2 seconds"
        );
    }

    // -- log_effective_config --

    /// A `Vec<u8>`-backed writer usable as a `tracing_subscriber` sink in tests.
    #[derive(Clone, Default)]
    struct CapturingWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for CapturingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CapturingWriter {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Proves the startup log line exposes the effective, non-secret
    /// configuration values (AC: "Startup logs expose effective non-secret
    /// configuration values"), and never leaks the database URL (which may
    /// embed PostgreSQL credentials).
    #[test]
    fn log_effective_config_exposes_values_without_secrets() {
        let writer = CapturingWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .finish();

        let config = Config {
            database_url: "postgres://admin:super-secret-password@db/flaps".to_owned(),
            bind_addr: "127.0.0.1:8080".to_owned(),
            admin_username: "admin".to_owned(),
            rate_limit_per_minute: Some(5),
            session_ttl_secs: Some(120),
        };

        tracing::subscriber::with_default(subscriber, || {
            log_effective_config(&config);
        });

        let output = String::from_utf8(writer.0.lock().unwrap().clone()).unwrap();
        assert!(
            output.contains("rate_limit_per_minute") && output.contains('5'),
            "log must expose the effective rate_limit_per_minute, got: {output}"
        );
        assert!(
            output.contains("session_ttl_secs") && output.contains("120"),
            "log must expose the effective session_ttl_secs, got: {output}"
        );
        assert!(
            !output.contains("super-secret-password"),
            "log must never expose the database_url credentials, got: {output}"
        );
    }
}
