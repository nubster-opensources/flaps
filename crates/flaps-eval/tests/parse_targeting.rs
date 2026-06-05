//! Parsing tests for targeting rules: the JsonLogic AST and the flagd
//! custom operations.
//!
//! Test helpers intentionally return boxed rules because AST constructors
//! take boxed operands, and operator fixtures use upstream operator names
//! even when they read alike.
#![allow(clippy::unnecessary_box_returns, clippy::similar_names)]

use flaps_eval::{FlagSet, Literal, ParseError, Rule, SemVerOp};

/// Parses a document with a single flag whose targeting is `targeting_json`
/// and returns the parsed targeting rule.
fn targeting(targeting_json: &str) -> Rule {
    parse_targeting(targeting_json).expect("valid targeting")
}

/// Same as [`targeting`] but surfaces the parse error.
fn parse_targeting(targeting_json: &str) -> Result<Rule, ParseError> {
    let document = format!(
        r#"{{
            "flags": {{
                "test-flag": {{
                    "state": "ENABLED",
                    "variants": {{ "on": true, "off": false }},
                    "defaultVariant": "off",
                    "targeting": {targeting_json}
                }}
            }}
        }}"#
    );
    FlagSet::from_json(&document).map(|mut set| {
        set.flags
            .remove("test-flag")
            .expect("flag present")
            .targeting
            .expect("targeting present")
    })
}

fn literal(value: &str) -> Box<Rule> {
    Box::new(Rule::Literal(Literal::String(value.to_owned())))
}

fn var(path: &str) -> Box<Rule> {
    Box::new(Rule::Var {
        path: path.to_owned(),
        default: None,
    })
}

#[test]
fn parses_var_with_string_path() {
    let rule = targeting(r#"{"var": "email"}"#);

    assert_eq!(rule, *var("email"));
}

#[test]
fn parses_var_with_default_and_flagd_injected_path() {
    let with_default = targeting(r#"{"var": ["plan", "free"]}"#);
    let injected = targeting(r#"{"var": "$flagd.flagKey"}"#);

    assert_eq!(
        with_default,
        Rule::Var {
            path: "plan".to_owned(),
            default: Some(Literal::String("free".to_owned())),
        }
    );
    assert_eq!(injected, *var("$flagd.flagKey"));
}

#[test]
fn parses_if_with_condition_and_outcomes() {
    let rule = targeting(r#"{"if": [{"var": "beta"}, "on", "off"]}"#);

    assert_eq!(
        rule,
        Rule::If(vec![*var("beta"), *literal("on"), *literal("off")])
    );
}

#[test]
fn parses_logic_operators() {
    let and = targeting(r#"{"and": [true, false]}"#);
    let or = targeting(r#"{"or": [{"var": "a"}, {"var": "b"}]}"#);
    let not = targeting(r#"{"!": [{"var": "a"}]}"#);
    let truthy = targeting(r#"{"!!": [{"var": "a"}]}"#);

    assert_eq!(
        and,
        Rule::And(vec![
            Rule::Literal(Literal::Bool(true)),
            Rule::Literal(Literal::Bool(false)),
        ])
    );
    assert_eq!(or, Rule::Or(vec![*var("a"), *var("b")]));
    assert_eq!(not, Rule::Not(var("a")));
    assert_eq!(truthy, Rule::Truthy(var("a")));
}

#[test]
fn parses_unary_sugar_without_array() {
    let not = targeting(r#"{"!": {"var": "a"}}"#);

    assert_eq!(not, Rule::Not(var("a")));
}

#[test]
fn parses_equality_operators() {
    let eq = targeting(r#"{"==": [{"var": "country"}, "FR"]}"#);
    let strict_eq = targeting(r#"{"===": [1, 1]}"#);
    let neq = targeting(r#"{"!=": [{"var": "a"}, 1]}"#);
    let strict_neq = targeting(r#"{"!==": [{"var": "a"}, 1]}"#);

    assert_eq!(eq, Rule::Eq(var("country"), literal("FR")));
    assert!(matches!(strict_eq, Rule::StrictEq(_, _)));
    assert!(matches!(neq, Rule::Neq(_, _)));
    assert!(matches!(strict_neq, Rule::StrictNeq(_, _)));
}

#[test]
fn parses_comparisons_including_ternary_between() {
    let gt = targeting(r#"{">": [{"var": "age"}, 18]}"#);
    let between = targeting(r#"{"<": [1, {"var": "age"}, 99]}"#);
    let lte_binary = targeting(r#"{"<=": [{"var": "age"}, 65]}"#);

    assert!(matches!(gt, Rule::Gt(_, _)));
    let Rule::Lt(args) = between else {
        panic!("expected Lt");
    };
    assert_eq!(args.len(), 3);
    let Rule::Lte(args) = lte_binary else {
        panic!("expected Lte");
    };
    assert_eq!(args.len(), 2);
}

#[test]
fn parses_arithmetic_operators() {
    let add = targeting(r#"{"+": [1, 2, 3]}"#);
    let modulo = targeting(r#"{"%": [{"var": "n"}, 2]}"#);
    let min = targeting(r#"{"min": [1, 2]}"#);

    let Rule::Add(args) = add else {
        panic!("expected Add");
    };
    assert_eq!(args.len(), 3);
    assert!(matches!(modulo, Rule::Mod(_, _)));
    assert!(matches!(min, Rule::Min(_)));
}

#[test]
fn parses_string_and_array_operators() {
    let cat = targeting(r#"{"cat": [{"var": "$flagd.flagKey"}, {"var": "email"}]}"#);
    let substr = targeting(r#"{"substr": [{"var": "email"}, 0, 5]}"#);
    let merge = targeting(r#"{"merge": [[1, 2], [3]]}"#);
    let map = targeting(r#"{"map": [{"var": "scores"}, {"+": [{"var": ""}, 1]}]}"#);
    let reduce = targeting(
        r#"{"reduce": [{"var": "xs"}, {"+": [{"var": "current"}, {"var": "accumulator"}]}, 0]}"#,
    );
    let all = targeting(r#"{"all": [{"var": "xs"}, {">": [{"var": ""}, 0]}]}"#);

    assert!(matches!(cat, Rule::Cat(_)));
    let Rule::Substr(args) = substr else {
        panic!("expected Substr");
    };
    assert_eq!(args.len(), 3);
    assert!(matches!(merge, Rule::Merge(_)));
    assert!(matches!(map, Rule::Map(_, _)));
    assert!(matches!(reduce, Rule::Reduce(_, _, _)));
    assert!(matches!(all, Rule::All(_, _)));
}

#[test]
fn parses_in_with_array_of_expressions() {
    let rule = targeting(r#"{"in": [{"var": "country"}, ["FR", "BE"]]}"#);

    let Rule::In(needle, haystack) = rule else {
        panic!("expected In");
    };
    assert_eq!(*needle, *var("country"));
    assert_eq!(*haystack, Rule::Array(vec![*literal("FR"), *literal("BE")]));
}

#[test]
fn parses_missing_operators() {
    let missing = targeting(r#"{"missing": ["email", "country"]}"#);
    let missing_some = targeting(r#"{"missing_some": [1, ["email", "phone"]]}"#);

    let Rule::Missing(keys) = missing else {
        panic!("expected Missing");
    };
    assert_eq!(keys.len(), 2);
    let Rule::MissingSome { min, keys } = missing_some else {
        panic!("expected MissingSome");
    };
    assert_eq!(min, 1);
    assert_eq!(keys.len(), 2);
}

#[test]
fn parses_string_comparison_custom_operators() {
    let starts = targeting(r#"{"starts_with": [{"var": "ip"}, "192.168"]}"#);
    let ends = targeting(r#"{"ends_with": [{"var": "email"}, "@example.com"]}"#);

    assert_eq!(starts, Rule::StartsWith(var("ip"), literal("192.168")));
    assert_eq!(ends, Rule::EndsWith(var("email"), literal("@example.com")));
}

#[test]
fn parses_sem_ver_with_all_operators() {
    let cases = [
        ("=", SemVerOp::Eq),
        ("!=", SemVerOp::Neq),
        ("<", SemVerOp::Lt),
        ("<=", SemVerOp::Lte),
        (">", SemVerOp::Gt),
        (">=", SemVerOp::Gte),
        ("^", SemVerOp::CaretMatch),
        ("~", SemVerOp::TildeMatch),
    ];

    for (symbol, expected) in cases {
        let rule = targeting(&format!(
            r#"{{"sem_ver": [{{"var": "version"}}, "{symbol}", "1.0.0"]}}"#
        ));
        let Rule::SemVer { op, .. } = rule else {
            panic!("expected SemVer for `{symbol}`");
        };
        assert_eq!(op, expected, "operator `{symbol}`");
    }
}

#[test]
fn rejects_unknown_sem_ver_operator() {
    let error = parse_targeting(r#"{"sem_ver": [{"var": "v"}, "~>", "1.0.0"]}"#)
        .expect_err("invalid sem_ver operator");

    assert!(
        matches!(error, ParseError::InvalidArguments { ref operator, .. } if operator == "sem_ver")
    );
}

#[test]
fn parses_fractional_with_bucketing_expression() {
    let rule = targeting(
        r#"{"fractional": [
            {"cat": [{"var": "$flagd.flagKey"}, {"var": "email"}]},
            ["red", 50],
            ["green", 50]
        ]}"#,
    );

    let Rule::Fractional { bucket_by, buckets } = rule else {
        panic!("expected Fractional");
    };
    assert!(bucket_by.is_some());
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0].variant, "red");
    assert_eq!(buckets[0].weight, 50);
}

#[test]
fn parses_fractional_shorthand_with_default_weight() {
    let rule = targeting(r#"{"fractional": [["red"], ["green", 3]]}"#);

    let Rule::Fractional { bucket_by, buckets } = rule else {
        panic!("expected Fractional");
    };
    assert!(bucket_by.is_none());
    assert_eq!(buckets[0].weight, 1);
    assert_eq!(buckets[1].weight, 3);
}

#[test]
fn rejects_unknown_operator_with_precise_path() {
    let error = parse_targeting(r#"{"if": [{"regex_match": [{"var": "a"}, ".*"]}, "on", "off"]}"#)
        .expect_err("unknown operator");

    let ParseError::UnknownOperator { path, operator } = error else {
        panic!("expected UnknownOperator");
    };
    assert_eq!(operator, "regex_match");
    assert!(path.contains("test-flag"), "path was `{path}`");
}

#[test]
fn rejects_operation_with_multiple_keys() {
    let error = parse_targeting(r#"{"==": [1, 1], "!=": [1, 2]}"#).expect_err("two keys");

    assert!(matches!(error, ParseError::InvalidDocument { .. }));
}

#[test]
fn rejects_binary_operator_with_wrong_arity() {
    let error = parse_targeting(r#"{"==": [1]}"#).expect_err("missing operand");

    assert!(matches!(error, ParseError::InvalidArguments { ref operator, .. } if operator == "=="));
}

#[test]
fn deserializes_rule_directly_via_serde() {
    let rule: Rule = serde_json::from_str(r#"{"var": "email"}"#).expect("valid rule");

    assert_eq!(rule, *var("email"));
}
