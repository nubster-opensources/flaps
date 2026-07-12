//! Core flag aggregate: metadata, type and global variant set.

use serde::{Deserialize, Serialize};

use crate::{
    key::FlagKey,
    metadata::Metadata,
    variant::{ValueType, Variants},
};

/// Classifies the intent of a feature flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagType {
    /// Controls progressive delivery of a new feature.
    Release,
    /// Controls operational behaviour (circuit breakers, kill switches).
    Ops,
    /// A/B or multivariate experiment.
    Experiment,
    /// Fine-grained access control.
    Permission,
}

/// A feature flag with its global metadata and variant declarations.
///
/// Variants are declared once at the flag level and referenced by key in
/// per-environment [`FlagEnvConfig`](crate::flag_env_config::FlagEnvConfig) rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Flag {
    /// Unique identifier within the project.
    pub key: FlagKey,
    /// Human-readable display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Semantic classification of the flag.
    pub flag_type: FlagType,
    /// Declared value type shared by all variants.
    pub value_type: ValueType,
    /// Global variant set; all values must match `value_type`.
    pub variants: Variants,
    /// Arbitrary flag-level metadata, merged into the resolution metadata at
    /// evaluation time (flag entries win over flag-set entries on collision).
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        key::{FlagKey, VariantKey},
        variant::{ValueType, VariantValue, Variants},
    };

    fn make_flag() -> Flag {
        let key = FlagKey::new("my-flag").unwrap();
        let vk_on = VariantKey::new("on").unwrap();
        let vk_off = VariantKey::new("off").unwrap();
        let variants = Variants::new(
            ValueType::Boolean,
            [
                (vk_on, VariantValue::Bool(true)),
                (vk_off, VariantValue::Bool(false)),
            ],
        )
        .unwrap();
        Flag {
            key,
            name: "My Flag".into(),
            description: Some("A test flag".into()),
            flag_type: FlagType::Release,
            value_type: ValueType::Boolean,
            variants,
            metadata: Metadata::new(),
        }
    }

    #[test]
    fn full_construction() {
        let flag = make_flag();
        assert_eq!(flag.key.as_str(), "my-flag");
        assert_eq!(flag.flag_type, FlagType::Release);
        assert_eq!(flag.value_type, ValueType::Boolean);
    }

    #[test]
    fn serde_round_trip_preserves_flag_type_and_value_type() {
        let flag = make_flag();
        let json = serde_json::to_string(&flag).unwrap();
        let back: Flag = serde_json::from_str(&json).unwrap();
        assert_eq!(back.flag_type, flag.flag_type);
        assert_eq!(back.value_type, flag.value_type);
        assert_eq!(back.key, flag.key);
    }

    #[test]
    fn all_flag_types_serialize() {
        for ft in [
            FlagType::Release,
            FlagType::Ops,
            FlagType::Experiment,
            FlagType::Permission,
        ] {
            let json = serde_json::to_string(&ft).unwrap();
            let back: FlagType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, ft);
        }
    }

    #[test]
    fn optional_description_none() {
        let key = FlagKey::new("minimal").unwrap();
        let variants = Variants::new(
            ValueType::Boolean,
            [(VariantKey::new("on").unwrap(), VariantValue::Bool(true))],
        )
        .unwrap();
        let flag = Flag {
            key,
            name: "Minimal".into(),
            description: None,
            flag_type: FlagType::Ops,
            value_type: ValueType::Boolean,
            variants,
            metadata: Metadata::new(),
        };
        assert!(flag.description.is_none());
    }

    #[test]
    fn serde_round_trip_preserves_metadata() {
        let mut flag = make_flag();
        flag.metadata.insert(
            "owner".to_owned(),
            crate::metadata::MetadataValue::String("team-a".into()),
        );
        flag.metadata.insert(
            "priority".to_owned(),
            crate::metadata::MetadataValue::Number(3.0),
        );

        let json = serde_json::to_string(&flag).unwrap();
        let back: Flag = serde_json::from_str(&json).unwrap();
        assert_eq!(back.metadata, flag.metadata);
    }

    #[test]
    fn metadata_absent_from_json_deserializes_to_empty_map() {
        // Proves #[serde(default)] backward compatibility: a Flag persisted
        // before metadata existed must still deserialize successfully.
        let json = serde_json::json!({
            "key": "legacy-flag",
            "name": "Legacy Flag",
            "description": null,
            "flag_type": "release",
            "value_type": "boolean",
            "variants": { "value_type": "boolean", "entries": { "on": { "bool": true } } }
        })
        .to_string();
        let flag: Flag = serde_json::from_str(&json).unwrap();
        assert!(flag.metadata.is_empty());
    }

    #[test]
    fn empty_metadata_is_omitted_from_serialized_json() {
        let flag = make_flag();
        let json = serde_json::to_string(&flag).unwrap();
        assert!(
            !json.contains("\"metadata\""),
            "empty metadata must not be serialized: {json}"
        );
    }
}
