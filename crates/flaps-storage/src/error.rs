//! Storage error types.

use thiserror::Error;

/// Errors that can occur in storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Database error from SQLx.
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Entity not found.
    #[error("{entity_type} with {field}={value} not found")]
    NotFound {
        entity_type: &'static str,
        field: &'static str,
        value: String,
    },

    /// Duplicate entity (unique constraint violation).
    #[error("{entity_type} with {field}={value} already exists")]
    Duplicate {
        entity_type: &'static str,
        field: &'static str,
        value: String,
    },

    /// Foreign key constraint violation.
    #[error("Referenced {entity_type} does not exist")]
    ForeignKeyViolation { entity_type: &'static str },

    /// Serialization/deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Redis error.
    #[error("Cache error: {0}")]
    Cache(#[from] redis::RedisError),

    /// Migration error.
    #[error("Migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    /// Configuration error.
    #[error("Configuration error: {0}")]
    Configuration(String),
}

impl StorageError {
    /// Creates a NotFound error.
    pub fn not_found(
        entity_type: &'static str,
        field: &'static str,
        value: impl Into<String>,
    ) -> Self {
        Self::NotFound {
            entity_type,
            field,
            value: value.into(),
        }
    }

    /// Creates a Duplicate error.
    pub fn duplicate(
        entity_type: &'static str,
        field: &'static str,
        value: impl Into<String>,
    ) -> Self {
        Self::Duplicate {
            entity_type,
            field,
            value: value.into(),
        }
    }

    /// Checks if this error is a unique constraint violation.
    pub fn is_unique_violation(&self) -> bool {
        matches!(self, Self::Duplicate { .. })
    }

    /// Checks if this error is a not found error.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }
}

/// Result type for storage operations.
pub type StorageResult<T> = Result<T, StorageError>;
