//! Variant values and the validated variant map attached to a flag.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{error::DomainError, key::VariantKey};

/// The scalar or structured type of a flag's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    /// Boolean on/off flag.
    Boolean,
    /// String-valued flag.
    String,
    /// Numeric (f64) flag.
    Number,
    /// Arbitrary JSON object or array flag.
    Object,
}

/// A concrete value carried by a variant.
///
/// The active arm must match the flag's [`ValueType`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariantValue {
    /// Boolean variant value.
    Bool(bool),
    /// String variant value.
    String(String),
    /// Numeric variant value.
    Number(f64),
    /// Structured JSON variant value.
    Json(serde_json::Value),
}

impl VariantValue {
    /// Returns `true` when this value is compatible with `value_type`.
    #[must_use]
    pub fn matches_type(&self, value_type: ValueType) -> bool {
        matches!(
            (self, value_type),
            (VariantValue::Bool(_), ValueType::Boolean)
                | (VariantValue::String(_), ValueType::String)
                | (VariantValue::Number(_), ValueType::Number)
                | (VariantValue::Json(_), ValueType::Object)
        )
    }
}

/// Private deserialization helper for [`Variants`].
///
/// Mirrors the serialized shape of [`Variants`] so that `TryFrom` can
/// route raw JSON through the validating constructor.
#[derive(Deserialize)]
struct VariantsRepr {
    value_type: ValueType,
    entries: HashMap<VariantKey, VariantValue>,
}

/// The validated set of variants declared globally on a flag.
///
/// All entries must carry values whose type matches the flag's [`ValueType`].
/// The set is immutable once constructed.
///
/// Deserialization is routed through [`Variants::new`] so that invariants
/// (non-empty, type-homogeneous) are enforced even when deserializing from JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(try_from = "VariantsRepr")]
pub struct Variants {
    value_type: ValueType,
    entries: HashMap<VariantKey, VariantValue>,
}

impl<'de> serde::Deserialize<'de> for Variants {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = VariantsRepr::deserialize(deserializer)?;
        Variants::try_from(repr).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<VariantsRepr> for Variants {
    type Error = DomainError;

    fn try_from(r: VariantsRepr) -> Result<Self, DomainError> {
        Variants::new(r.value_type, r.entries)
    }
}

impl Variants {
    /// Constructs a [`Variants`] map, validating that:
    /// - `entries` is non-empty.
    /// - Every value matches `value_type`.
    ///
    /// # Errors
    /// - [`DomainError::EmptyVariants`] when `entries` is empty.
    /// - [`DomainError::VariantTypeMismatch`] when a value's type does not match.
    pub fn new(
        value_type: ValueType,
        entries: impl IntoIterator<Item = (VariantKey, VariantValue)>,
    ) -> Result<Self, DomainError> {
        let entries: HashMap<VariantKey, VariantValue> = entries.into_iter().collect();
        if entries.is_empty() {
            return Err(DomainError::EmptyVariants);
        }
        for (key, value) in &entries {
            if !value.matches_type(value_type) {
                return Err(DomainError::VariantTypeMismatch {
                    variant: key.as_str().to_owned(),
                    value_type,
                });
            }
        }
        Ok(Self {
            value_type,
            entries,
        })
    }

    /// Returns the declared value type for all variants in this set.
    #[must_use]
    pub fn value_type(&self) -> ValueType {
        self.value_type
    }

    /// Returns the value for `key`, or `None` if not present.
    #[must_use]
    pub fn get(&self, key: &VariantKey) -> Option<&VariantValue> {
        self.entries.get(key)
    }

    /// Returns `true` when `key` is present in this variant set.
    #[must_use]
    pub fn contains(&self, key: &VariantKey) -> bool {
        self.entries.contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::VariantKey;

    fn vk(s: &str) -> VariantKey {
        VariantKey::new(s).unwrap()
    }

    #[test]
    fn new_ok_all_types_match() {
        let v = Variants::new(
            ValueType::Boolean,
            [
                (vk("on"), VariantValue::Bool(true)),
                (vk("off"), VariantValue::Bool(false)),
            ],
        );
        assert!(v.is_ok());
    }

    #[test]
    fn new_err_type_mismatch() {
        let v = Variants::new(
            ValueType::Boolean,
            [(vk("bad"), VariantValue::String("x".into()))],
        );
        assert!(matches!(v, Err(DomainError::VariantTypeMismatch { .. })));
    }

    #[test]
    fn new_err_empty_variants() {
        let v = Variants::new(ValueType::Boolean, []);
        assert!(matches!(v, Err(DomainError::EmptyVariants)));
    }

    #[test]
    fn get_and_contains() {
        let key = vk("on");
        let variants = Variants::new(
            ValueType::Boolean,
            [(key.clone(), VariantValue::Bool(true))],
        )
        .unwrap();
        assert!(variants.contains(&key));
        assert_eq!(variants.get(&key), Some(&VariantValue::Bool(true)));
        assert!(!variants.contains(&vk("off")));
    }

    #[test]
    fn serde_round_trip() {
        let variants = Variants::new(
            ValueType::String,
            [(vk("a"), VariantValue::String("hello".into()))],
        )
        .unwrap();
        let json = serde_json::to_string(&variants).unwrap();
        let back: Variants = serde_json::from_str(&json).unwrap();
        assert_eq!(back, variants);
    }

    #[test]
    fn number_variants_accepted() {
        let v = Variants::new(ValueType::Number, [(vk("x"), VariantValue::Number(1.5))]);
        assert!(v.is_ok());
    }

    #[test]
    fn object_variants_accepted() {
        let v = Variants::new(
            ValueType::Object,
            [(vk("cfg"), VariantValue::Json(serde_json::json!({"k": 1})))],
        );
        assert!(v.is_ok());
    }

    #[test]
    fn deserialize_rejects_empty_variants() {
        let json = r#"{"value_type":"boolean","entries":{}}"#;
        let result: Result<Variants, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "deserialization must reject empty variant set"
        );
    }

    #[test]
    fn deserialize_rejects_type_mismatch() {
        // entries contains a string value but value_type is boolean
        let json = r#"{"value_type":"boolean","entries":{"on":{"string":"hello"}}}"#;
        let result: Result<Variants, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "deserialization must reject type-mismatched variant values"
        );
    }

    #[test]
    fn deserialize_valid_variants_round_trip() {
        let variants =
            Variants::new(ValueType::Boolean, [(vk("on"), VariantValue::Bool(true))]).unwrap();
        let json = serde_json::to_string(&variants).unwrap();
        let back: Variants = serde_json::from_str(&json).unwrap();
        assert_eq!(back, variants);
    }
}
