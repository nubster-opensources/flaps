//! Store-level error type and result alias.

/// Errors produced by the persistence layer.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A database driver error.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// A JSON serialization or deserialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    /// A migration failed.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// The requested entity does not exist.
    #[error("entity not found")]
    NotFound,
    /// A uniqueness constraint was violated.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A write referenced a parent entity that does not exist (foreign-key violation).
    #[error("referenced entity does not exist")]
    ForeignKeyViolation,
}

/// Convenience alias for `Result<T, StoreError>`.
pub type StoreResult<T> = Result<T, StoreError>;
