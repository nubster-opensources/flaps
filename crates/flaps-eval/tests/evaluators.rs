//! Tests for `$evaluators` resolution and inlining at parse time.

use flaps_eval::{FlagSet, ParseError, Rule};

fn document(evaluators: &str, targeting: &str) -> String {
    format!(
        r#"{{
            "$evaluators": {evaluators},
            "flags": {{
                "test-flag": {{
                    "state": "ENABLED",
                    "variants": {{ "on": true, "off": false }},
                    "defaultVariant": "off",
                    "targeting": {targeting}
                }}
            }}
        }}"#
    )
}

fn targeting_of(document: &str) -> Result<Rule, ParseError> {
    FlagSet::from_json(document).map(|mut set| {
        set.flags
            .remove("test-flag")
            .expect("flag present")
            .targeting
            .expect("targeting present")
    })
}

#[test]
fn inlines_evaluator_reference() {
    let document = document(
        r#"{"internal": {"ends_with": [{"var": "email"}, "@example.com"]}}"#,
        r#"{"if": [{"$ref": "internal"}, "on", "off"]}"#,
    );

    let targeting = targeting_of(&document).expect("valid document");

    let Rule::If(args) = targeting else {
        panic!("expected If");
    };
    assert!(matches!(args[0], Rule::EndsWith(_, _)));
}

#[test]
fn inlines_nested_evaluator_references() {
    let document = document(
        r#"{
            "inner": {"==": [{"var": "plan"}, "pro"]},
            "outer": {"and": [{"$ref": "inner"}, {"var": "active"}]}
        }"#,
        r#"{"if": [{"$ref": "outer"}, "on", "off"]}"#,
    );

    let targeting = targeting_of(&document).expect("valid document");

    let Rule::If(args) = targeting else {
        panic!("expected If");
    };
    let Rule::And(operands) = &args[0] else {
        panic!("expected And");
    };
    assert!(matches!(operands[0], Rule::Eq(_, _)));
}

#[test]
fn rejects_unknown_evaluator_reference() {
    let document = document("{}", r#"{"$ref": "missing"}"#);

    let error = targeting_of(&document).expect_err("unknown evaluator");

    assert!(
        matches!(error, ParseError::UnknownEvaluator { ref reference, .. } if reference == "missing")
    );
}

#[test]
fn rejects_evaluator_cycle() {
    let document = document(
        r#"{
            "a": {"and": [{"$ref": "b"}, true]},
            "b": {"or": [{"$ref": "a"}, false]}
        }"#,
        r#"{"$ref": "a"}"#,
    );

    let error = targeting_of(&document).expect_err("evaluator cycle");

    assert!(matches!(error, ParseError::EvaluatorCycle { .. }));
}

#[test]
fn standalone_rule_deserialization_keeps_references() {
    let rule: Rule = serde_json::from_str(r#"{"$ref": "name"}"#).expect("valid rule");

    assert_eq!(rule, Rule::Ref("name".to_owned()));
}
