//! Resolution tests for flag evaluation: OpenFeature reasons, variant
//! selection, disabled flags, metadata merging and adversarial input.

use std::collections::BTreeMap;

use flaps_eval::{EvaluationContext, EvaluationError, FlagSet, MetadataValue, Reason};

/// Parses a flag set document, panicking on invalid fixtures.
fn flag_set(document: &str) -> FlagSet {
    FlagSet::from_json(document).expect("valid flag set")
}

/// A flag set with one string flag serving colors behind a country rule.
fn color_set() -> FlagSet {
    flag_set(
        r#"{
            "flags": {
                "background": {
                    "state": "ENABLED",
                    "variants": { "red": "crimson", "green": "forest" },
                    "defaultVariant": "red",
                    "targeting": {
                        "if": [
                            {"==": [{"var": "country"}, "FR"]}, "green",
                            null
                        ]
                    }
                }
            }
        }"#,
    )
}

/// An evaluation context carrying a single string attribute.
fn context_with(key: &str, value: &str) -> EvaluationContext {
    EvaluationContext {
        attributes: BTreeMap::from([(key.to_owned(), value.into())]),
        ..EvaluationContext::default()
    }
}

#[test]
fn static_reason_when_targeting_is_absent() {
    let set = flag_set(
        r#"{
            "flags": {
                "plain": {
                    "state": "ENABLED",
                    "variants": { "on": true, "off": false },
                    "defaultVariant": "on"
                }
            }
        }"#,
    );

    let resolution = set
        .evaluate("plain", &EvaluationContext::default())
        .expect("evaluation succeeds");

    assert_eq!(resolution.reason, Reason::Static);
    assert_eq!(resolution.variant.as_deref(), Some("on"));
    assert_eq!(resolution.value, Some(serde_json::Value::Bool(true)));
}

#[test]
fn targeting_match_resolves_string_variants() {
    let resolution = color_set()
        .evaluate("background", &context_with("country", "FR"))
        .expect("evaluation succeeds");

    assert_eq!(resolution.reason, Reason::TargetingMatch);
    assert_eq!(resolution.variant.as_deref(), Some("green"));
    assert_eq!(resolution.value, Some("forest".into()));
}

#[test]
fn null_targeting_outcome_exits_to_the_default_variant() {
    let resolution = color_set()
        .evaluate("background", &context_with("country", "DE"))
        .expect("evaluation succeeds");

    assert_eq!(resolution.reason, Reason::Default);
    assert_eq!(resolution.variant.as_deref(), Some("red"));
    assert_eq!(resolution.value, Some("crimson".into()));
}

#[test]
fn disabled_flags_resolve_without_value_or_variant() {
    let set = flag_set(
        r#"{
            "flags": {
                "killed": {
                    "state": "DISABLED",
                    "variants": { "on": true, "off": false },
                    "defaultVariant": "on"
                }
            }
        }"#,
    );

    let resolution = set
        .evaluate("killed", &EvaluationContext::default())
        .expect("evaluation succeeds");

    assert_eq!(resolution.reason, Reason::Disabled);
    assert_eq!(resolution.variant, None);
    assert_eq!(resolution.value, None);
}

#[test]
fn disabled_flags_evaluate_no_rule() {
    let set = flag_set(
        r#"{
            "flags": {
                "killed": {
                    "state": "DISABLED",
                    "variants": { "on": true, "off": false },
                    "defaultVariant": "on",
                    "targeting": { "fractional": [["on", 50], ["off", 50]] }
                }
            }
        }"#,
    );

    let resolution = set
        .evaluate("killed", &EvaluationContext::default())
        .expect("disabled flags short-circuit before any rule runs");

    assert_eq!(resolution.reason, Reason::Disabled);
}

#[test]
fn unknown_flags_are_not_found() {
    let error = color_set()
        .evaluate("absent", &EvaluationContext::default())
        .expect_err("unknown keys fail");

    assert!(matches!(
        error,
        EvaluationError::FlagNotFound { flag_key } if flag_key == "absent"
    ));
}

#[test]
fn unknown_variant_names_are_invalid() {
    let set = flag_set(
        r#"{
            "flags": {
                "background": {
                    "state": "ENABLED",
                    "variants": { "red": "crimson" },
                    "defaultVariant": "red",
                    "targeting": { "if": [true, "purple", null] }
                }
            }
        }"#,
    );

    let error = set
        .evaluate("background", &EvaluationContext::default())
        .expect_err("unknown variant names fail");

    assert!(matches!(error, EvaluationError::InvalidVariant { .. }));
}

#[test]
fn non_variant_outcomes_are_invalid() {
    let fixtures = [r#"{ "+": [40, 2] }"#, r#"{ "merge": [["a"], ["b"]] }"#];
    for targeting in fixtures {
        let document = format!(
            r#"{{
                "flags": {{
                    "background": {{
                        "state": "ENABLED",
                        "variants": {{ "red": "crimson" }},
                        "defaultVariant": "red",
                        "targeting": {targeting}
                    }}
                }}
            }}"#
        );

        let error = flag_set(&document)
            .evaluate("background", &EvaluationContext::default())
            .expect_err("non variant outcomes fail");

        assert!(matches!(error, EvaluationError::InvalidVariant { .. }));
    }
}

#[test]
fn boolean_outcomes_without_matching_variants_are_invalid() {
    let set = flag_set(
        r#"{
            "flags": {
                "background": {
                    "state": "ENABLED",
                    "variants": { "red": "crimson" },
                    "defaultVariant": "red",
                    "targeting": { "==": [1, 1] }
                }
            }
        }"#,
    );

    let error = set
        .evaluate("background", &EvaluationContext::default())
        .expect_err("boolean outcomes need true and false variants");

    assert!(matches!(error, EvaluationError::InvalidVariant { .. }));
}

#[test]
fn default_variants_outside_the_variants_are_invalid() {
    let set = flag_set(
        r#"{
            "flags": {
                "background": {
                    "state": "ENABLED",
                    "variants": { "red": "crimson" },
                    "defaultVariant": "ghost"
                }
            }
        }"#,
    );

    let error = set
        .evaluate("background", &EvaluationContext::default())
        .expect_err("dangling default variants fail");

    assert!(matches!(error, EvaluationError::InvalidVariant { .. }));
}

#[test]
fn missing_default_variant_resolves_to_no_value() {
    let set = flag_set(
        r#"{
            "flags": {
                "background": {
                    "state": "ENABLED",
                    "variants": { "red": "crimson" },
                    "targeting": { "if": [false, "red"] }
                }
            }
        }"#,
    );

    let resolution = set
        .evaluate("background", &EvaluationContext::default())
        .expect("evaluation succeeds");

    assert_eq!(resolution.reason, Reason::Default);
    assert_eq!(resolution.variant, None);
    assert_eq!(resolution.value, None);
}

#[test]
fn static_without_default_variant_resolves_to_no_value() {
    let set = flag_set(
        r#"{
            "flags": {
                "background": {
                    "state": "ENABLED",
                    "variants": { "red": "crimson" }
                }
            }
        }"#,
    );

    let resolution = set
        .evaluate("background", &EvaluationContext::default())
        .expect("evaluation succeeds");

    assert_eq!(resolution.reason, Reason::Static);
    assert_eq!(resolution.variant, None);
    assert_eq!(resolution.value, None);
}

#[test]
fn metadata_merges_set_and_flag_entries_with_flag_priority() {
    let set = flag_set(
        r#"{
            "flags": {
                "plain": {
                    "state": "ENABLED",
                    "variants": { "on": true, "off": false },
                    "defaultVariant": "on",
                    "metadata": { "team": "checkout", "tier": 2 }
                }
            },
            "metadata": { "team": "platform", "owner": "ops" }
        }"#,
    );

    let resolution = set
        .evaluate("plain", &EvaluationContext::default())
        .expect("evaluation succeeds");

    assert_eq!(
        resolution.metadata.get("team"),
        Some(&MetadataValue::String("checkout".to_owned()))
    );
    assert_eq!(
        resolution.metadata.get("owner"),
        Some(&MetadataValue::String("ops".to_owned()))
    );
    assert_eq!(
        resolution.metadata.get("tier"),
        Some(&MetadataValue::Number(2.0))
    );
}

#[test]
fn adversarial_contexts_never_panic() {
    let set = flag_set(
        r#"{
            "flags": {
                "probe": {
                    "state": "ENABLED",
                    "variants": { "true": true, "false": false },
                    "defaultVariant": "false",
                    "targeting": {
                        "and": [
                            {"<": [{"var": "blob"}, {"var": "list.5.deep"}]},
                            {"+": [{"var": "blob"}, 1]},
                            {"in": [{"var": "list"}, {"var": "blob.nested"}]},
                            {"substr": [{"var": "blob"}, {"var": "list.0"}, -99]},
                            {"==": [{"var": ""}, {"var": "blob"}]}
                        ]
                    }
                }
            }
        }"#,
    );
    let context = EvaluationContext {
        attributes: serde_json::from_str(
            r#"{
                "blob": { "nested": { "deeper": [1, 2, {"x": null}] } },
                "list": [[["a"]], {}, "", 0, -0.0, 1e308],
                "targetingKey": "shadowed"
            }"#,
        )
        .expect("valid attributes"),
        targeting_key: Some("real-key".to_owned()),
        ..EvaluationContext::default()
    };

    let resolution = set
        .evaluate("probe", &context)
        .expect("adversarial input degrades to falsy results");

    assert_eq!(resolution.reason, Reason::TargetingMatch);
    assert_eq!(resolution.variant.as_deref(), Some("false"));
}
