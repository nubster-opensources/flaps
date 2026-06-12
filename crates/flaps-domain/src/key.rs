//! Typed, validated identifiers for all domain aggregates.
//!
//! Every key is a kebab-case newtype with an immutable smart constructor.
//! Pattern: `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// Validates that `value` is a non-empty kebab-case identifier.
///
/// Accepted pattern: `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`
fn validate_kebab(value: &str) -> Result<(), DomainError> {
    if value.is_empty() {
        return Err(DomainError::InvalidKey(value.to_owned()));
    }
    let mut chars = value.chars().peekable();
    // First char must be [a-z]
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return Err(DomainError::InvalidKey(value.to_owned())),
    }
    // Remaining chars: [a-z0-9] or '-' followed immediately by [a-z0-9]
    let mut prev_was_dash = false;
    for c in chars {
        if c == '-' {
            if prev_was_dash {
                return Err(DomainError::InvalidKey(value.to_owned()));
            }
            prev_was_dash = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            prev_was_dash = false;
        } else {
            return Err(DomainError::InvalidKey(value.to_owned()));
        }
    }
    // Must not end with '-'
    if prev_was_dash {
        return Err(DomainError::InvalidKey(value.to_owned()));
    }
    Ok(())
}

macro_rules! define_key {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            /// Creates a new key, validating kebab-case format.
            ///
            /// # Errors
            /// Returns [`DomainError::InvalidKey`] when the value is not valid kebab-case.
            pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let s = value.into();
                validate_kebab(&s)?;
                Ok(Self(s))
            }

            /// Returns the key as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<$name> for String {
            fn from(k: $name) -> String {
                k.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = DomainError;

            fn try_from(s: String) -> Result<Self, DomainError> {
                Self::new(s)
            }
        }
    };
}

define_key!(
    /// Unique identifier for a feature flag within a project.
    FlagKey
);

define_key!(
    /// Unique identifier for a project.
    ProjectKey
);

define_key!(
    /// Unique identifier for an environment within a project.
    EnvironmentKey
);

define_key!(
    /// Unique identifier for a segment within a project.
    SegmentKey
);

define_key!(
    /// Unique identifier for a variant within a flag.
    VariantKey
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_kebab_keys() {
        assert!(FlagKey::new("my-flag").is_ok());
        assert!(FlagKey::new("a").is_ok());
        assert!(FlagKey::new("a-1").is_ok());
        assert!(FlagKey::new("abc").is_ok());
        assert!(FlagKey::new("my-flag-123").is_ok());
    }

    #[test]
    fn rejects_uppercase() {
        assert!(FlagKey::new("My-Flag").is_err());
        assert!(FlagKey::new("MY_FLAG").is_err());
    }

    #[test]
    fn rejects_leading_digit() {
        assert!(FlagKey::new("1abc").is_err());
    }

    #[test]
    fn rejects_double_dash() {
        assert!(FlagKey::new("a--b").is_err());
    }

    #[test]
    fn rejects_trailing_dash() {
        assert!(FlagKey::new("a-").is_err());
    }

    #[test]
    fn rejects_leading_dash() {
        assert!(FlagKey::new("-a").is_err());
    }

    #[test]
    fn rejects_underscore() {
        assert!(FlagKey::new("a_b").is_err());
    }

    #[test]
    fn rejects_empty() {
        assert!(FlagKey::new("").is_err());
    }

    #[test]
    fn serde_round_trip() {
        let key = FlagKey::new("my-flag").unwrap();
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, r#""my-flag""#);
        let back: FlagKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn as_str_returns_inner_value() {
        let key = ProjectKey::new("my-project").unwrap();
        assert_eq!(key.as_str(), "my-project");
    }

    #[test]
    fn all_key_types_validate() {
        assert!(ProjectKey::new("proj-1").is_ok());
        assert!(EnvironmentKey::new("prod").is_ok());
        assert!(SegmentKey::new("beta-users").is_ok());
        assert!(VariantKey::new("enabled").is_ok());
    }

    #[test]
    fn deserialize_rejects_invalid_key_with_space() {
        let result: Result<FlagKey, _> = serde_json::from_str(r#""BAD KEY""#);
        assert!(
            result.is_err(),
            "deserialization must reject keys with spaces"
        );
    }

    #[test]
    fn deserialize_rejects_key_with_underscore() {
        let result: Result<FlagKey, _> = serde_json::from_str(r#""bad_key""#);
        assert!(
            result.is_err(),
            "deserialization must reject keys with underscores"
        );
    }

    #[test]
    fn deserialize_rejects_key_with_uppercase() {
        let result: Result<FlagKey, _> = serde_json::from_str(r#""BadKey""#);
        assert!(
            result.is_err(),
            "deserialization must reject keys with uppercase"
        );
    }

    #[test]
    fn deserialize_accepts_valid_key_round_trip() {
        let key = FlagKey::new("my-flag").unwrap();
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, r#""my-flag""#);
        let back: FlagKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }
}
