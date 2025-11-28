//! # Flaps Storage
//!
//! Storage abstraction layer for Nubster Flaps.
//!
//! This crate provides traits and implementations for persisting
//! flags, segments, and related data.
//!
//! ## Architecture
//!
//! Flaps integrates with the Nubster ecosystem:
//! - **Workspace API**: Source of truth for projects, tenants, and groups
//! - **Local storage**: Flaps-specific data (flags, segments, environments)
//! - **Cache layer**: Redis for high-performance flag evaluation
//!
//! ## Storage Backends
//!
//! - PostgreSQL (production)
//! - SQLite (development, on-prem single node)
//! - Redis (caching layer)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use flaps_storage::{Database, DatabaseConfig, HttpWorkspaceClient, WorkspaceClientConfig};
//!
//! // Connect to local storage
//! let config = DatabaseConfig::postgres("postgres://localhost/flaps");
//! let db = Database::connect(&config).await?;
//!
//! // Connect to Workspace API
//! let workspace = HttpWorkspaceClient::with_base_url("http://workspace-api:8080")?;
//! ```

pub mod cache;
pub mod db;
pub mod error;
pub mod traits;
pub mod workspace;

// Re-exports
pub use db::{Database, DatabaseConfig, DatabaseType};
pub use error::{StorageError, StorageResult};
pub use traits::*;

// PostgreSQL implementations
pub use db::postgres::PostgresRepositories;

// SQLite implementations
pub use db::sqlite::SqliteRepositories;

// Workspace client
pub use workspace::HttpWorkspaceClient;

// Redis cache
pub use cache::{RedisCacheConfig, RedisFlagCache};
