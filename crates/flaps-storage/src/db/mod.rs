//! Database connection and pool management.

pub mod postgres;
pub mod sqlite;

use sqlx::{Pool, Postgres, Sqlite};
use std::time::Duration;

use crate::error::{StorageError, StorageResult};

/// Database configuration.
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Connection URL (postgres:// or sqlite://).
    pub url: String,
    /// Maximum number of connections in the pool.
    pub max_connections: u32,
    /// Minimum number of connections to keep open.
    pub min_connections: u32,
    /// Connection timeout in seconds.
    pub connect_timeout_secs: u64,
    /// Idle timeout for connections in seconds.
    pub idle_timeout_secs: u64,
    /// Whether to run migrations on startup.
    pub run_migrations: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "sqlite://flaps.db?mode=rwc".to_string(),
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 30,
            idle_timeout_secs: 600,
            run_migrations: true,
        }
    }
}

impl DatabaseConfig {
    /// Creates a new PostgreSQL configuration.
    pub fn postgres(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: 20,
            min_connections: 5,
            ..Default::default()
        }
    }

    /// Creates a new SQLite configuration.
    pub fn sqlite(path: impl Into<String>) -> Self {
        Self {
            url: format!("sqlite://{}?mode=rwc", path.into()),
            max_connections: 5,
            min_connections: 1,
            ..Default::default()
        }
    }

    /// Creates an in-memory SQLite configuration (for testing).
    pub fn sqlite_memory() -> Self {
        Self {
            url: "sqlite::memory:".to_string(),
            max_connections: 1,
            min_connections: 1,
            ..Default::default()
        }
    }

    /// Checks if this is a PostgreSQL configuration.
    pub fn is_postgres(&self) -> bool {
        self.url.starts_with("postgres://") || self.url.starts_with("postgresql://")
    }

    /// Checks if this is a SQLite configuration.
    pub fn is_sqlite(&self) -> bool {
        self.url.starts_with("sqlite://") || self.url.starts_with("sqlite:")
    }
}

/// Database type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseType {
    PostgreSQL,
    SQLite,
}

/// A database connection pool that can be either PostgreSQL or SQLite.
#[derive(Debug, Clone)]
pub enum Database {
    Postgres(Pool<Postgres>),
    Sqlite(Pool<Sqlite>),
}

impl Database {
    /// Creates a new PostgreSQL database connection.
    pub async fn connect_postgres(config: &DatabaseConfig) -> StorageResult<Pool<Postgres>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_secs(config.connect_timeout_secs))
            .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
            .connect(&config.url)
            .await?;

        Ok(pool)
    }

    /// Creates a new SQLite database connection.
    pub async fn connect_sqlite(config: &DatabaseConfig) -> StorageResult<Pool<Sqlite>> {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .min_connections(config.min_connections)
            .acquire_timeout(Duration::from_secs(config.connect_timeout_secs))
            .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
            .connect(&config.url)
            .await?;

        Ok(pool)
    }

    /// Creates a new database connection from configuration.
    pub async fn connect(config: &DatabaseConfig) -> StorageResult<Self> {
        if config.is_postgres() {
            let pool = Self::connect_postgres(config).await?;
            Ok(Self::Postgres(pool))
        } else if config.is_sqlite() {
            let pool = Self::connect_sqlite(config).await?;
            Ok(Self::Sqlite(pool))
        } else {
            Err(StorageError::Configuration(format!(
                "Unsupported database URL: {}",
                config.url
            )))
        }
    }

    /// Returns the database type.
    pub fn db_type(&self) -> DatabaseType {
        match self {
            Self::Postgres(_) => DatabaseType::PostgreSQL,
            Self::Sqlite(_) => DatabaseType::SQLite,
        }
    }

    /// Returns the PostgreSQL pool if this is a PostgreSQL database.
    pub fn postgres(&self) -> Option<&Pool<Postgres>> {
        match self {
            Self::Postgres(pool) => Some(pool),
            _ => None,
        }
    }

    /// Returns the SQLite pool if this is a SQLite database.
    pub fn sqlite(&self) -> Option<&Pool<Sqlite>> {
        match self {
            Self::Sqlite(pool) => Some(pool),
            _ => None,
        }
    }

    /// Closes the database connection pool.
    pub async fn close(&self) {
        match self {
            Self::Postgres(pool) => pool.close().await,
            Self::Sqlite(pool) => pool.close().await,
        }
    }

    /// Checks if the database is healthy.
    pub async fn is_healthy(&self) -> bool {
        match self {
            Self::Postgres(pool) => sqlx::query("SELECT 1").fetch_one(pool).await.is_ok(),
            Self::Sqlite(pool) => sqlx::query("SELECT 1").fetch_one(pool).await.is_ok(),
        }
    }
}

// Note: Migrations are run via `cargo sqlx migrate run` or through the flaps-cli.
// The sqlx::migrate! macro requires compile-time access to migration files,
// which is complex to set up in a workspace. Instead, we provide runtime
// migration support through the Migrator type.
