//! Parsing tests for the flagd document structure, excluding targeting.
//!
//! Exact float comparisons are intentional: JSON numbers round-trip exactly
//! through the parser, so any drift is a bug worth failing on.
#![allow(clippy::float_cmp)]

use flaps_eval::{FlagSet, MetadataValue, ParseError, State, Variants};

#[test]
fn parses_empty_flag_set() {
    let set = FlagSet::from_json(r#"{"flags": {}}"#).expect("valid document");

    assert!(set.flags.is_empty());
    assert!(set.metadata.is_empty());
}

#[test]
fn parses_boolean_flag_without_targeting() {
    let document = r#"{
        "flags": {
            "new-welcome-banner": {
                "state": "ENABLED",
                "variants": { "on": true, "off": false },
                "defaultVariant": "off"
            }
        }
    }"#;

    let set = FlagSet::from_json(document).expect("valid document");
    let flag = &set.flags["new-welcome-banner"];

    assert_eq!(flag.state, State::Enabled);
    assert_eq!(flag.default_variant.as_deref(), Some("off"));
    assert!(flag.targeting.is_none());
    let Variants::Boolean(variants) = &flag.variants else {
        panic!("expected boolean variants");
    };
    assert!(variants["on"]);
    assert!(!variants["off"]);
}

#[test]
fn parses_disabled_state_and_missing_default_variant() {
    let document = r#"{
        "flags": {
            "kill-switch": {
                "state": "DISABLED",
                "variants": { "on": true, "off": false }
            }
        }
    }"#;

    let set = FlagSet::from_json(document).expect("valid document");
    let flag = &set.flags["kill-switch"];

    assert_eq!(flag.state, State::Disabled);
    assert!(flag.default_variant.is_none());
}

#[test]
fn rejects_invalid_state() {
    let document = r#"{
        "flags": {
            "broken": {
                "state": "PAUSED",
                "variants": { "on": true },
                "defaultVariant": "on"
            }
        }
    }"#;

    let error = FlagSet::from_json(document).expect_err("invalid state");

    assert!(
        matches!(error, ParseError::InvalidDocument { ref path, .. } if path.contains("broken"))
    );
}

#[test]
fn rejects_mixed_variant_types() {
    let document = r#"{
        "flags": {
            "mixed": {
                "state": "ENABLED",
                "variants": { "on": true, "off": "no" },
                "defaultVariant": "on"
            }
        }
    }"#;

    let error = FlagSet::from_json(document).expect_err("mixed variants");

    assert!(matches!(error, ParseError::MixedVariantTypes { ref flag_key } if flag_key == "mixed"));
}

#[test]
fn parses_string_number_and_object_variants() {
    let document = r#"{
        "flags": {
            "greeting": {
                "state": "ENABLED",
                "variants": { "fr": "bonjour", "en": "hello" },
                "defaultVariant": "en"
            },
            "page-size": {
                "state": "ENABLED",
                "variants": { "small": 10, "large": 50.5 },
                "defaultVariant": "small"
            },
            "theme": {
                "state": "ENABLED",
                "variants": { "dark": { "bg": "black" }, "light": { "bg": "white" } },
                "defaultVariant": "light"
            }
        }
    }"#;

    let set = FlagSet::from_json(document).expect("valid document");

    let Variants::String(greeting) = &set.flags["greeting"].variants else {
        panic!("expected string variants");
    };
    assert_eq!(greeting["fr"], "bonjour");

    let Variants::Number(page_size) = &set.flags["page-size"].variants else {
        panic!("expected number variants");
    };
    assert_eq!(page_size["large"], 50.5);

    let Variants::Object(theme) = &set.flags["theme"].variants else {
        panic!("expected object variants");
    };
    assert_eq!(theme["dark"]["bg"], "black");
}

#[test]
fn parses_flag_set_and_flag_metadata() {
    let document = r#"{
        "flags": {
            "new-welcome-banner": {
                "state": "ENABLED",
                "variants": { "on": true, "off": false },
                "defaultVariant": "off",
                "metadata": { "version": "17", "experimental": true }
            }
        },
        "metadata": { "team": "user-experience", "flagSetId": "ecommerce", "weight": 1.5 }
    }"#;

    let set = FlagSet::from_json(document).expect("valid document");

    assert_eq!(
        set.metadata["team"],
        MetadataValue::String("user-experience".into())
    );
    assert_eq!(set.metadata["weight"], MetadataValue::Number(1.5));
    let flag = &set.flags["new-welcome-banner"];
    assert_eq!(flag.metadata["version"], MetadataValue::String("17".into()));
    assert_eq!(flag.metadata["experimental"], MetadataValue::Bool(true));
}

#[test]
fn rejects_document_without_flags_property() {
    let error = FlagSet::from_json(r#"{"metadata": {}}"#).expect_err("missing flags");

    assert!(matches!(error, ParseError::InvalidDocument { .. }));
}

#[test]
fn rejects_syntactically_invalid_json() {
    let error = FlagSet::from_json("{not json").expect_err("invalid JSON");

    assert!(matches!(error, ParseError::Json(_)));
}
