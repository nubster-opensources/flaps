//! SQLite repository implementations.
//!
//! SQLite is used for:
//! - Local development
//! - On-premise single-node deployments
//! - Testing

mod environments;
mod flags;
mod segments;

pub use environments::SqliteEnvironmentRepository;
pub use flags::SqliteFlagRepository;
pub use segments::SqliteSegmentRepository;

use sqlx::{Pool, Sqlite};

/// SQLite repositories bundle.
#[derive(Debug, Clone)]
pub struct SqliteRepositories {
    pub flags: SqliteFlagRepository,
    pub segments: SqliteSegmentRepository,
    pub environments: SqliteEnvironmentRepository,
}

impl SqliteRepositories {
    /// Creates a new set of SQLite repositories.
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self {
            flags: SqliteFlagRepository::new(pool.clone()),
            segments: SqliteSegmentRepository::new(pool.clone()),
            environments: SqliteEnvironmentRepository::new(pool),
        }
    }
}
