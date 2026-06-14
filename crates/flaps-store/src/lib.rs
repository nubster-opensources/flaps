//! Persistence layer for the Flaps feature flag platform.
//!
//! This crate persists the **editable source model** defined by `flaps-domain`:
//! projects, environments, feature flags, segments, per-environment flag
//! configurations, SDK keys, and local admin accounts.
//!
//! # Backends
//!
//! Two concrete implementations are provided:
//!
//! - [`sqlite::SqliteStore`]: SQLite pool with embedded migrations. Suitable for
//!   single-node deployments and in-memory testing.
//! - [`postgres::PostgresStore`]: PostgreSQL pool with embedded migrations.
//!   Suitable for production multi-instance deployments.
//!
//! # Usage
//!
//! ```rust,no_run
//! # async fn example() -> flaps_store::StoreResult<()> {
//! use flaps_store::{sqlite::SqliteStore, KeyHasher};
//! use flaps_store::repository::ProjectRepository;
//!
//! let store = SqliteStore::in_memory(KeyHasher::new(b"my-pepper")).await?;
//! let projects = store.list_projects().await?;
//! # Ok(())
//! # }
//! ```

mod clock;

pub mod account;
pub mod audit;
pub mod error;
pub mod hash;
pub mod postgres;
pub mod repository;
pub mod sdk_key;
pub mod sqlite;

pub use account::{AccountRecord, NewSession};
pub use audit::AuditRecord;
pub use error::{StoreError, StoreResult};
pub use hash::KeyHasher;
pub use sdk_key::{NewSdkKey, SdkKeyRecord, SdkKeyScope};
