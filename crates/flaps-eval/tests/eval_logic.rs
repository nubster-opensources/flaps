//! Evaluation tests for the JsonLogic interpreter: truthiness, coercion,
//! logic, numeric, string and array operators.
//!
//! Rules are exercised through flag targeting with a boolean probe flag, so
//! every fixture wraps the rule under test into an expression producing a
//! boolean, mirroring how the flagd conformance suites probe the engine.

use std::collections::BTreeMap;

use flaps_eval::{EvaluationContext, FlagSet};

/// Builds a flag set with a single boolean probe flag using the targeting.
fn probe_set(targeting_json: &str) -> FlagSet {
    let document = format!(
        r#"{{
            "flags": {{
                "probe": {{
                    "state": "ENABLED",
                    "variants": {{ "true": true, "false": false }},
                    "defaultVariant": "false",
                    "targeting": {targeting_json}
                }}
            }}
        }}"#
    );
    FlagSet::from_json(&document).expect("valid flag set")
}

/// Evaluates the targeting against the context; true when the rule matched.
fn matches_with(targeting_json: &str, context: &EvaluationContext) -> bool {
    let resolution = probe_set(targeting_json)
        .evaluate("probe", context)
        .expect("evaluation succeeds");
    resolution.variant.as_deref() == Some("true")
}

/// Same as [`matches_with`] with an empty evaluation context.
fn matches(targeting_json: &str) -> bool {
    matches_with(targeting_json, &EvaluationContext::default())
}

/// Builds an evaluation context from a JSON object of attributes.
fn context_with(attributes_json: &str) -> EvaluationContext {
    let attributes: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(attributes_json).expect("valid attributes");
    EvaluationContext {
        attributes,
        ..EvaluationContext::default()
    }
}

#[test]
fn truthiness_follows_the_jsonlogic_table() {
    assert!(!matches(r#"{"!!": [0]}"#));
    assert!(matches(r#"{"!!": [1]}"#));
    assert!(matches(r#"{"!!": [-1]}"#));
    assert!(!matches(r#"{"!!": [[]]}"#));
    assert!(matches(r#"{"!!": [[1, 2]]}"#));
    assert!(!matches(r#"{"!!": [""]}"#));
    assert!(matches(r#"{"!!": ["anything"]}"#));
    assert!(matches(r#"{"!!": ["0"]}"#));
    assert!(!matches(r#"{"!!": [null]}"#));
}

#[test]
fn not_negates_truthiness() {
    assert!(matches(r#"{"!": [""]}"#));
    assert!(!matches(r#"{"!": ["a"]}"#));
    assert!(matches(r#"{"!": ""}"#));
}

#[test]
fn loose_equality_coerces_numbers_strings_and_booleans() {
    assert!(matches(r#"{"==": [1, 1]}"#));
    assert!(matches(r#"{"==": [1, "1"]}"#));
    assert!(matches(r#"{"==": [0, false]}"#));
    assert!(matches(r#"{"==": ["0", false]}"#));
    assert!(!matches(r#"{"==": [1, 2]}"#));
}

#[test]
fn null_is_loose_equal_only_to_null() {
    assert!(matches(r#"{"==": [null, null]}"#));
    assert!(!matches(r#"{"==": [null, 0]}"#));
    assert!(!matches(r#"{"==": [null, false]}"#));
    assert!(!matches(r#"{"==": [null, ""]}"#));
}

#[test]
fn single_element_arrays_coerce_in_loose_equality() {
    assert!(matches(r#"{"==": [[1], 1]}"#));
    assert!(matches(r#"{"==": [["1"], 1]}"#));
}

#[test]
fn strict_equality_requires_matching_types() {
    assert!(matches(r#"{"===": [1, 1]}"#));
    assert!(!matches(r#"{"===": [1, "1"]}"#));
    assert!(matches(r#"{"===": ["a", "a"]}"#));
}

#[test]
fn composites_are_never_strictly_equal() {
    assert!(!matches(r#"{"===": [[1, 2], [1, 2]]}"#));
}

#[test]
fn loose_and_strict_inequality() {
    assert!(matches(r#"{"!=": [1, 2]}"#));
    assert!(!matches(r#"{"!=": [1, "1"]}"#));
    assert!(matches(r#"{"!==": [1, "1"]}"#));
    assert!(!matches(r#"{"!==": [1, 1]}"#));
}

#[test]
fn and_returns_the_first_falsy_operand() {
    assert!(matches(r#"{"===": [{"and": [true, "", 3]}, ""]}"#));
    assert!(matches(r#"{"===": [{"and": [0, "a"]}, 0]}"#));
}

#[test]
fn and_returns_the_last_operand_when_all_truthy() {
    assert!(matches(r#"{"===": [{"and": [true, "a", 3]}, 3]}"#));
}

#[test]
fn or_returns_the_first_truthy_operand() {
    assert!(matches(r#"{"===": [{"or": [false, 0, "a"]}, "a"]}"#));
}

#[test]
fn or_returns_the_last_operand_when_all_falsy() {
    assert!(matches(r#"{"===": [{"or": [false, 0]}, 0]}"#));
}

#[test]
fn if_selects_the_first_matching_condition_pair() {
    assert!(matches(r#"{"===": [{"if": [true, "yes", "no"]}, "yes"]}"#));
    assert!(matches(r#"{"===": [{"if": [false, "yes", "no"]}, "no"]}"#));

    let context = context_with(r#"{"temp": 55}"#);
    let cascade = r#"{"===": [
        {"if": [
            {"<": [{"var": "temp"}, 0]}, "freezing",
            {"<": [{"var": "temp"}, 100]}, "liquid",
            "gas"
        ]},
        "liquid"
    ]}"#;
    assert!(matches_with(cascade, &context));
}

#[test]
fn if_without_an_else_yields_null() {
    assert!(matches(r#"{"===": [{"if": [false, "x"]}, null]}"#));
}

#[test]
fn numeric_comparisons_coerce_operands() {
    assert!(matches(r#"{">": ["2", 1]}"#));
    assert!(matches(r#"{"<": [1, "2"]}"#));
    assert!(matches(r#"{">=": [2, 2]}"#));
    assert!(!matches(r#"{"<=": [3, 2]}"#));
}

#[test]
fn string_comparisons_are_lexicographic() {
    assert!(matches(r#"{"<": ["a", "b"]}"#));
    assert!(matches(r#"{">": ["b", "a"]}"#));
    assert!(!matches(r#"{"<": ["b", "a"]}"#));
}

#[test]
fn comparisons_with_non_numeric_operands_are_false() {
    assert!(!matches(r#"{"<": ["abc", 5]}"#));
    assert!(!matches(r#"{">": ["abc", 5]}"#));
}

#[test]
fn between_tests_exclusive_and_inclusive_bounds() {
    assert!(matches(r#"{"<": [1, 2, 3]}"#));
    assert!(!matches(r#"{"<": [1, 1, 3]}"#));
    assert!(matches(r#"{"<=": [1, 1, 3]}"#));
    assert!(!matches(r#"{"<=": [1, 4, 3]}"#));

    let context = context_with(r#"{"temp": 37}"#);
    assert!(matches_with(
        r#"{"<": [0, {"var": "temp"}, 100]}"#,
        &context
    ));
}

#[test]
fn addition_and_multiplication_are_variadic() {
    assert!(matches(r#"{"===": [{"+": [2, 2, 2, 2, 2]}, 10]}"#));
    assert!(matches(r#"{"===": [{"*": [2, 2, 2, 2, 2]}, 32]}"#));
}

#[test]
fn unary_minus_negates_and_unary_plus_casts() {
    assert!(matches(r#"{"===": [{"-": [2]}, -2]}"#));
    assert!(matches(r#"{"===": [{"+": ["3.14"]}, 3.14]}"#));
}

#[test]
fn subtraction_division_and_modulo() {
    assert!(matches(r#"{"===": [{"-": [4, 2]}, 2]}"#));
    assert!(matches(r#"{"===": [{"/": [4, 2]}, 2]}"#));
    assert!(matches(r#"{"===": [{"%": [101, 2]}, 1]}"#));
}

#[test]
fn min_and_max_select_extremes() {
    assert!(matches(r#"{"===": [{"min": [1, 2, 3]}, 1]}"#));
    assert!(matches(r#"{"===": [{"max": [1, 2, 3]}, 3]}"#));
}

#[test]
fn non_finite_results_materialize_as_null() {
    assert!(matches(r#"{"===": [{"/": [1, 0]}, null]}"#));
    assert!(matches(r#"{"===": [{"/": [0, 0]}, null]}"#));
    assert!(matches(r#"{"===": [{"+": ["abc"]}, null]}"#));
}

#[test]
fn cat_concatenates_with_string_coercion() {
    assert!(matches(
        r#"{"===": [{"cat": ["I love ", "pie"]}, "I love pie"]}"#
    ));
    assert!(matches(r#"{"===": [{"cat": ["pi=", 3.14]}, "pi=3.14"]}"#));
    assert!(matches(r#"{"===": [{"cat": [1, 2]}, "12"]}"#));
}

#[test]
fn substr_supports_negative_positions_and_lengths() {
    assert!(matches(
        r#"{"===": [{"substr": ["jsonlogic", 4]}, "logic"]}"#
    ));
    assert!(matches(
        r#"{"===": [{"substr": ["jsonlogic", -5]}, "logic"]}"#
    ));
    assert!(matches(
        r#"{"===": [{"substr": ["jsonlogic", 1, 3]}, "son"]}"#
    ));
    assert!(matches(
        r#"{"===": [{"substr": ["jsonlogic", 4, -2]}, "log"]}"#
    ));
}

#[test]
fn in_finds_substrings_and_array_members() {
    assert!(matches(r#"{"in": ["Spring", "Springfield"]}"#));
    assert!(!matches(r#"{"in": ["Illinois", "Springfield"]}"#));
    assert!(matches(r#"{"in": ["Mike", ["Bob", "Mike"]]}"#));
    assert!(!matches(r#"{"in": ["Todd", ["Bob", "Mike"]]}"#));
}

#[test]
fn array_membership_is_strict() {
    assert!(!matches(r#"{"in": [1, ["1", 2]]}"#));
    assert!(matches(r#"{"in": [2, ["1", 2]]}"#));
}

#[test]
fn merge_flattens_arrays_and_wraps_scalars() {
    assert!(matches(r#"{"in": [3, {"merge": [[1, 2], [3, 4]]}]}"#));
    assert!(matches(r#"{"in": [1, {"merge": [1, [2, 3]]}]}"#));
    assert!(!matches(r#"{"in": [5, {"merge": [[1, 2], [3, 4]]}]}"#));
}

#[test]
fn map_rebinds_the_scope_to_each_element() {
    let context = context_with(r#"{"integers": [1, 2, 3, 4, 5]}"#);
    let doubled = r#"{"in": [10, {"map": [{"var": "integers"}, {"*": [{"var": ""}, 2]}]}]}"#;
    assert!(matches_with(doubled, &context));
}

#[test]
fn filter_keeps_elements_with_truthy_outcomes() {
    let context = context_with(r#"{"integers": [1, 2, 3, 4, 5]}"#);
    let odds = r#"{"in": [1, {"filter": [{"var": "integers"}, {"%": [{"var": ""}, 2]}]}]}"#;
    let evens_absent = r#"{"in": [2, {"filter": [{"var": "integers"}, {"%": [{"var": ""}, 2]}]}]}"#;
    assert!(matches_with(odds, &context));
    assert!(!matches_with(evens_absent, &context));
}

#[test]
fn reduce_folds_with_current_and_accumulator() {
    let context = context_with(r#"{"integers": [1, 2, 3, 4, 5]}"#);
    let sum = r#"{"===": [
        {"reduce": [
            {"var": "integers"},
            {"+": [{"var": "current"}, {"var": "accumulator"}]},
            0
        ]},
        15
    ]}"#;
    assert!(matches_with(sum, &context));
}

#[test]
fn all_none_and_some_quantify_over_arrays() {
    assert!(matches(r#"{"all": [[1, 2, 3], {">": [{"var": ""}, 0]}]}"#));
    assert!(!matches(r#"{"all": [[1, 2, 3], {">": [{"var": ""}, 1]}]}"#));
    assert!(matches(r#"{"some": [[1, 2, 3], {">": [{"var": ""}, 2]}]}"#));
    assert!(!matches(
        r#"{"some": [[1, 2, 3], {">": [{"var": ""}, 3]}]}"#
    ));
    assert!(matches(r#"{"none": [[1, 2, 3], {">": [{"var": ""}, 3]}]}"#));
    assert!(!matches(
        r#"{"none": [[1, 2, 3], {">": [{"var": ""}, 2]}]}"#
    ));
}

#[test]
fn quantifiers_over_empty_arrays() {
    assert!(!matches(r#"{"all": [[], {"!!": [{"var": ""}]}]}"#));
    assert!(!matches(r#"{"some": [[], {"!!": [{"var": ""}]}]}"#));
    assert!(matches(r#"{"none": [[], {"!!": [{"var": ""}]}]}"#));
}

#[test]
fn var_reads_nested_attributes_and_defaults() {
    let context = context_with(r#"{"user": {"email": "a@b.c"}, "letters": ["a", "b", "c"]}"#);
    assert!(matches_with(
        r#"{"==": [{"var": "user.email"}, "a@b.c"]}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"==": [{"var": "letters.1"}, "b"]}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"===": [{"var": ["absent", 26]}, 26]}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"===": [{"var": "absent"}, null]}"#,
        &context
    ));
}

#[test]
fn targeting_key_is_exposed_as_an_attribute() {
    let context = EvaluationContext {
        targeting_key: Some("user-1".to_owned()),
        ..EvaluationContext::default()
    };
    assert!(matches_with(
        r#"{"==": [{"var": "targetingKey"}, "user-1"]}"#,
        &context
    ));
}

#[test]
fn flagd_properties_are_injected_into_the_context() {
    let context = EvaluationContext {
        timestamp: 1234,
        ..EvaluationContext::default()
    };
    assert!(matches_with(
        r#"{"==": [{"var": "$flagd.flagKey"}, "probe"]}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"===": [{"var": "$flagd.timestamp"}, 1234]}"#,
        &context
    ));
}

#[test]
fn missing_lists_absent_empty_and_null_keys() {
    let context = context_with(r#"{"a": 1, "b": "", "c": null}"#);
    assert!(!matches_with(
        r#"{"in": ["a", {"missing": ["a", "b", "c", "d"]}]}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"in": ["b", {"missing": ["a", "b", "c", "d"]}]}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"in": ["c", {"missing": ["a", "b", "c", "d"]}]}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"in": ["d", {"missing": ["a", "b", "c", "d"]}]}"#,
        &context
    ));
    assert!(matches_with(r#"{"!": {"missing": ["a"]}}"#, &context));
}

#[test]
fn missing_some_requires_a_minimum_of_present_keys() {
    let context = context_with(r#"{"a": 1}"#);
    assert!(matches_with(
        r#"{"!": {"missing_some": [1, ["a", "b", "c"]]}}"#,
        &context
    ));
    assert!(matches_with(
        r#"{"in": ["b", {"missing_some": [2, ["a", "b", "c"]]}]}"#,
        &context
    ));
}

#[test]
fn custom_operations_evaluate_successfully() {
    // All four flagd custom operators must produce a valid resolution now that
    // they are implemented.  The exact variant is not asserted here; the
    // dedicated `eval_custom_operators` integration tests cover correctness.
    // `$ref` is the only operator that remains unsupported at eval time
    // (references are inlined at parse time and should never reach the engine).
    let fixtures = [
        r#"{"fractional": [["true", 50], ["false", 50]]}"#,
        r#"{"sem_ver": ["1.1.2", ">=", "1.0.0"]}"#,
        r#"{"starts_with": ["192.168.0.1", "192.168"]}"#,
        r#"{"ends_with": ["noreply@example.com", "@example.com"]}"#,
    ];
    for targeting in fixtures {
        probe_set(targeting)
            .evaluate("probe", &EvaluationContext::default())
            .expect("custom operations must not return an error");
    }
}
