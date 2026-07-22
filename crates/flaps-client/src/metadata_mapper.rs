//! Maps flaps-eval [`Metadata`] to OpenFeature [`FlagMetadata`].
//!
//! Mirrors `flaps_eval::metadata_to_json`, the single source of truth also
//! used by the OFREP response DTOs in `flaps-server`, so a local resolution
//! and the equivalent OFREP response agree on both the entries present and
//! their JSON-ish type (bool, string, integer or float).

use flaps_eval::{Metadata, MetadataValue};
use open_feature::{FlagMetadata, FlagMetadataValue};

use crate::coerce::{I64_MAX_SAFE_F64, I64_MIN_F64};

/// Converts flaps-eval ruleset [`Metadata`] to an OpenFeature [`FlagMetadata`].
///
/// Returns `None` when `metadata` is empty, matching the OFREP response
/// contract where the `metadata` field is omitted rather than serialized as
/// an empty object (see `flaps_server::routes::ofrep::metadata_field`).
#[must_use]
pub(crate) fn map_metadata(metadata: &Metadata) -> Option<FlagMetadata> {
    if metadata.is_empty() {
        return None;
    }

    let mut flag_metadata = FlagMetadata::default();
    for (key, value) in metadata {
        flag_metadata.add_value(key.clone(), map_value(value));
    }
    Some(flag_metadata)
}

/// Converts a single flaps-eval metadata value, preserving its type.
fn map_value(value: &MetadataValue) -> FlagMetadataValue {
    match value {
        MetadataValue::Bool(value) => FlagMetadataValue::Bool(*value),
        MetadataValue::String(value) => FlagMetadataValue::String(value.clone()),
        MetadataValue::Number(value) => number_value(*value),
    }
}

/// Converts a metadata number to an OpenFeature metadata value, preferring
/// [`FlagMetadataValue::Int`] when the value is integer-compatible.
///
/// Uses the same exactness check as [`crate::coerce::to_int`] (whole number,
/// within the range that round-trips losslessly through `i64`), so a local
/// resolution agrees on type with the OFREP JSON representation of the same
/// metadata entry, which applies the equivalent check in
/// `flaps_eval::serialize::number_value`.
fn number_value(value: f64) -> FlagMetadataValue {
    if value.fract() == 0.0 && (I64_MIN_F64..=I64_MAX_SAFE_F64).contains(&value) {
        #[allow(clippy::cast_possible_truncation)]
        return FlagMetadataValue::Int(value as i64);
    }
    FlagMetadataValue::Float(value)
}

#[cfg(test)]
mod tests {
    use flaps_eval::{Metadata, MetadataValue};
    use open_feature::{FlagMetadata, FlagMetadataValue};

    use super::map_metadata;

    #[test]
    fn empty_metadata_maps_to_none() {
        let metadata = Metadata::new();
        assert_eq!(map_metadata(&metadata), None);
    }

    #[test]
    fn bool_value_retains_type() {
        let mut metadata = Metadata::new();
        metadata.insert("enabled".to_owned(), MetadataValue::Bool(true));
        let mapped = map_metadata(&metadata).expect("non-empty metadata maps to Some");
        assert_eq!(
            mapped.values.get("enabled"),
            Some(&FlagMetadataValue::Bool(true))
        );
    }

    #[test]
    fn string_value_retains_type() {
        let mut metadata = Metadata::new();
        metadata.insert("owner".to_owned(), MetadataValue::String("team-a".into()));
        let mapped = map_metadata(&metadata).expect("non-empty metadata maps to Some");
        assert_eq!(
            mapped.values.get("owner"),
            Some(&FlagMetadataValue::String("team-a".to_owned()))
        );
    }

    #[test]
    fn integer_compatible_number_maps_to_int() {
        let mut metadata = Metadata::new();
        metadata.insert("priority".to_owned(), MetadataValue::Number(3.0));
        let mapped = map_metadata(&metadata).expect("non-empty metadata maps to Some");
        assert_eq!(
            mapped.values.get("priority"),
            Some(&FlagMetadataValue::Int(3))
        );
    }

    #[test]
    fn fractional_number_maps_to_float() {
        let mut metadata = Metadata::new();
        metadata.insert("ratio".to_owned(), MetadataValue::Number(1.5));
        let mapped = map_metadata(&metadata).expect("non-empty metadata maps to Some");
        assert_eq!(
            mapped.values.get("ratio"),
            Some(&FlagMetadataValue::Float(1.5))
        );
    }

    #[test]
    fn negative_integer_compatible_number_maps_to_int() {
        let mut metadata = Metadata::new();
        metadata.insert("offset".to_owned(), MetadataValue::Number(-2.0));
        let mapped = map_metadata(&metadata).expect("non-empty metadata maps to Some");
        assert_eq!(
            mapped.values.get("offset"),
            Some(&FlagMetadataValue::Int(-2))
        );
    }

    #[test]
    fn mixed_entries_all_present() {
        let mut metadata = Metadata::new();
        metadata.insert("enabled".to_owned(), MetadataValue::Bool(false));
        metadata.insert("owner".to_owned(), MetadataValue::String("team-b".into()));
        metadata.insert("priority".to_owned(), MetadataValue::Number(2.0));
        metadata.insert("ratio".to_owned(), MetadataValue::Number(0.25));

        let mapped: FlagMetadata =
            map_metadata(&metadata).expect("non-empty metadata maps to Some");
        assert_eq!(mapped.values.len(), 4);
        assert_eq!(
            mapped.values.get("enabled"),
            Some(&FlagMetadataValue::Bool(false))
        );
        assert_eq!(
            mapped.values.get("owner"),
            Some(&FlagMetadataValue::String("team-b".to_owned()))
        );
        assert_eq!(
            mapped.values.get("priority"),
            Some(&FlagMetadataValue::Int(2))
        );
        assert_eq!(
            mapped.values.get("ratio"),
            Some(&FlagMetadataValue::Float(0.25))
        );
    }
}
