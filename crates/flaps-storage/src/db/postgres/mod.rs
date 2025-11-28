//! PostgreSQL repository implementations.

mod environments;
mod flags;
mod segments;

pub use environments::PostgresEnvironmentRepository;
pub use flags::PostgresFlagRepository;
pub use segments::PostgresSegmentRepository;

use sqlx::{Pool, Postgres};

/// PostgreSQL repositories bundle.
#[derive(Debug, Clone)]
pub struct PostgresRepositories {
    pub flags: PostgresFlagRepository,
    pub segments: PostgresSegmentRepository,
    pub environments: PostgresEnvironmentRepository,
}

impl PostgresRepositories {
    /// Creates a new set of PostgreSQL repositories.
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self {
            flags: PostgresFlagRepository::new(pool.clone()),
            segments: PostgresSegmentRepository::new(pool.clone()),
            environments: PostgresEnvironmentRepository::new(pool),
        }
    }
}
