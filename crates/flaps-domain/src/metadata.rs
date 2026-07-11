//! Arbitrary scalar metadata attached to a flag or an environment.
//!
//! Mirrors the flagd metadata model at the domain boundary: an ordered map of
//! keys to bare scalar values (boolean, string or number). This type is
//! deliberately independent from `flaps-eval`'s own `MetadataValue`: the
//! domain crate has no dependency on the evaluation model (see the crate-level
//! documentation), so the compiler is the only place that converts between
//! the two.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A single metadata value: a bare boolean, string or number.
///
/// `#[serde(untagged)]` makes this serialize as a bare JSON scalar (`true`,
/// `"owner-team"`, `42`) rather than as a tagged enum object, matching the
/// flagd metadata schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MetadataValue {
    /// A boolean metadata value.
    Bool(bool),
    /// A string metadata value.
    String(String),
    /// A numeric metadata value.
    Number(f64),
}

/// Arbitrary key to scalar metadata attached to a [`crate::Flag`] or a
/// [`crate::Environment`].
///
/// A `BTreeMap` guarantees stable key ordering, which matters for
/// deterministic serialization (see `flaps-compiler`'s determinism guarantee).
pub type Metadata = BTreeMap<String, MetadataValue>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bool_serializes_as_bare_scalar() {
        let json = serde_json::to_string(&MetadataValue::Bool(true)).unwrap();
        assert_eq!(json, "true");
    }

    #[test]
    fn bool_round_trips() {
        let value = MetadataValue::Bool(false);
        let json = serde_json::to_string(&value).unwrap();
        let back: MetadataValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn string_serializes_as_bare_scalar() {
        let json = serde_json::to_string(&MetadataValue::String("team-a".into())).unwrap();
        assert_eq!(json, "\"team-a\"");
    }

    #[test]
    fn string_round_trips() {
        let value = MetadataValue::String("owner-team".into());
        let json = serde_json::to_string(&value).unwrap();
        let back: MetadataValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn number_serializes_as_bare_scalar() {
        let json = serde_json::to_string(&MetadataValue::Number(42.0)).unwrap();
        assert_eq!(json, "42.0");
    }

    #[test]
    fn number_round_trips() {
        let value = MetadataValue::Number(3.5);
        let json = serde_json::to_string(&value).unwrap();
        let back: MetadataValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn metadata_map_round_trips_key_order() {
        let mut metadata: Metadata = Metadata::new();
        metadata.insert("owner".to_owned(), MetadataValue::String("team-a".into()));
        metadata.insert("priority".to_owned(), MetadataValue::Number(3.0));
        metadata.insert("beta".to_owned(), MetadataValue::Bool(false));

        let json = serde_json::to_string(&metadata).unwrap();
        let back: Metadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back, metadata);

        let keys: Vec<&String> = metadata.keys().collect();
        assert_eq!(keys, vec!["beta", "owner", "priority"]);
    }
}
