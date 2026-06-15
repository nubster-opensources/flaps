//! Flaps server daemon entry point.
//!
//! Parses `--config <path>`, initialises structured logging, connects to the
//! store with retry, warms up the compiled ruleset cache, bootstraps the admin
//! account on first boot, then starts the HTTP server with graceful shutdown.
//!
//! All heavy logic lives in [`flapsd_lib::bootstrap`] and [`flapsd_lib::config`]
//! so it can be unit-tested without spawning a real process.

use anyhow::{Context as _, Result};
use clap::Parser;
use flaps_server::build_router;
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

    let pepper = read_pepper().context("pepper configuration")?;
    let hasher = KeyHasher::new(pepper);

    let url = config.database_url.clone();

    if url.starts_with("sqlite:") {
        let hasher_clone = hasher.clone();
        let url_sqlite = url.clone();
        let store = connect_store_with_retry(Box::new(move || {
            let hasher_inner = hasher_clone.clone();
            let url_inner = url_sqlite.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(SqliteStore::connect(&url_inner, hasher_inner))
                    .map_err(anyhow::Error::from)
            })
        }))
        .await
        .context("connecting to SQLite store")?;

        let state = flaps_server::state::AppState::new(store);
        boot(state, config).await
    } else {
        use flaps_store::postgres::PostgresStore;
        let hasher_clone = hasher.clone();
        let url_pg = url.clone();
        let store = connect_store_with_retry(Box::new(move || {
            let hasher_inner = hasher_clone.clone();
            let url_inner = url_pg.clone();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(PostgresStore::connect(&url_inner, hasher_inner))
                    .map_err(anyhow::Error::from)
            })
        }))
        .await
        .context("connecting to PostgreSQL store")?;

        let state = flaps_server::state::AppState::new(store);
        boot(state, config).await
    }
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

    let bind_addr = config.socket_addr();
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
