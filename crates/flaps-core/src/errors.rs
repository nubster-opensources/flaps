//! Error types for Flaps operations.

use thiserror::Error;

/// Main error type for Flaps operations.
#[derive(Debug, Error)]
pub enum FlapsError {
    /// Flag was not found.
    #[error("Flag not found: {0}")]
    FlagNotFound(String),

    /// Environment was not found.
    #[error("Environment not found: {0}")]
    EnvironmentNotFound(String),

    /// Project was not found.
    #[error("Project not found: {0}")]
    ProjectNotFound(String),

    /// Segment was not found.
    #[error("Segment not found: {0}")]
    SegmentNotFound(String),

    /// Invalid rule configuration.
    #[error("Invalid rule: {0}")]
    InvalidRule(String),

    /// Invalid attribute value.
    #[error("Invalid attribute value: {0}")]
    InvalidAttributeValue(String),

    /// Invalid flag key format.
    #[error("Invalid flag key: {0}")]
    InvalidFlagKey(String),

    /// Duplicate key error.
    #[error("Duplicate key: {0}")]
    DuplicateKey(String),

    /// Validation error.
    #[error("Validation error: {0}")]
    Validation(String),

    /// Storage error.
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Storage-specific errors.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Connection error.
    #[error("Connection error: {0}")]
    Connection(String),

    /// Query error.
    #[error("Query error: {0}")]
    Query(String),

    /// Record not found.
    #[error("Record not found: {0}")]
    NotFound(String),

    /// Conflict (e.g., concurrent modification).
    #[error("Conflict: {0}")]
    Conflict(String),

    /// Transaction error.
    #[error("Transaction error: {0}")]
    Transaction(String),

    /// Migration error.
    #[error("Migration error: {0}")]
    Migration(String),
}

impl FlapsError {
    /// Creates a flag not found error.
    pub fn flag_not_found(key: impl Into<String>) -> Self {
        Self::FlagNotFound(key.into())
    }

    /// Creates an environment not found error.
    pub fn environment_not_found(key: impl Into<String>) -> Self {
        Self::EnvironmentNotFound(key.into())
    }

    /// Creates a project not found error.
    pub fn project_not_found(key: impl Into<String>) -> Self {
        Self::ProjectNotFound(key.into())
    }

    /// Creates a validation error.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    /// Returns true if this is a "not found" error.
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::FlagNotFound(_)
                | Self::EnvironmentNotFound(_)
                | Self::ProjectNotFound(_)
                | Self::SegmentNotFound(_)
                | Self::Storage(StorageError::NotFound(_))
        )
    }

    /// Returns true if this is a conflict error.
    pub fn is_conflict(&self) -> bool {
        matches!(
            self,
            Self::DuplicateKey(_) | Self::Storage(StorageError::Conflict(_))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = FlapsError::flag_not_found("my-flag");
        assert_eq!(err.to_string(), "Flag not found: my-flag");

        let err = FlapsError::validation("Invalid percentage value");
        assert_eq!(err.to_string(), "Validation error: Invalid percentage value");
    }

    #[test]
    fn test_error_classification() {
        assert!(FlapsError::flag_not_found("x").is_not_found());
        assert!(FlapsError::environment_not_found("x").is_not_found());
        assert!(!FlapsError::validation("x").is_not_found());

        assert!(FlapsError::DuplicateKey("x".to_string()).is_conflict());
        assert!(!FlapsError::flag_not_found("x").is_conflict());
    }
}
