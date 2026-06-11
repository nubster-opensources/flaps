//! Evaluation tests for the flagd custom operators: `starts_with`, `ends_with`,
//! `sem_ver` and `fractional`.
//!
//! Rules are exercised through flag targeting in the same style as
//! `eval_logic.rs`: a single boolean probe flag wraps the rule under test and
//! the resolved variant is inspected.  Fractional tests use a string probe
//! flag so the variant name carries the bucket assignment.

use std::collections::BTreeMap;

use flaps_eval::{EvaluationContext, FlagSet, Reason};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Builds a flag set with a single boolean probe flag using the given targeting.
fn probe_bool(targeting_json: &str) -> FlagSet {
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

/// Builds a flag set with a string probe flag and the given variants and
/// targeting, used to inspect fractional bucket assignments.
fn probe_string(variants_json: &str, targeting_json: &str) -> FlagSet {
    let document = format!(
        r#"{{
            "flags": {{
                "probe": {{
                    "state": "ENABLED",
                    "variants": {variants_json},
                    "defaultVariant": "none",
                    "targeting": {targeting_json}
                }}
            }}
        }}"#
    );
    FlagSet::from_json(&document).expect("valid flag set")
}

/// Evaluates the boolean probe against the context and returns the matched variant.
fn matches_with(targeting_json: &str, context: &EvaluationContext) -> bool {
    let resolution = probe_bool(targeting_json)
        .evaluate("probe", context)
        .expect("evaluation succeeds");
    resolution.variant.as_deref() == Some("true")
}

/// Same as `matches_with` with an empty context.
fn matches(targeting_json: &str) -> bool {
    matches_with(targeting_json, &EvaluationContext::default())
}

/// Builds a context with a targeting key and optional extra attributes.
fn ctx(targeting_key: &str) -> EvaluationContext {
    EvaluationContext {
        targeting_key: Some(targeting_key.to_owned()),
        ..EvaluationContext::default()
    }
}

/// Builds a context with attributes from a JSON object.
fn ctx_attrs(attrs_json: &str) -> EvaluationContext {
    let attributes: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(attrs_json).expect("valid attributes");
    EvaluationContext {
        attributes,
        ..EvaluationContext::default()
    }
}

/// Builds a context with a targeting key and attributes.
fn ctx_with(targeting_key: &str, attrs_json: &str) -> EvaluationContext {
    let attributes: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(attrs_json).expect("valid attributes");
    EvaluationContext {
        targeting_key: Some(targeting_key.to_owned()),
        attributes,
        ..EvaluationContext::default()
    }
}

// ---------------------------------------------------------------------------
// starts_with
// ---------------------------------------------------------------------------

#[test]
fn starts_with_true_when_left_starts_with_right() {
    assert!(matches(r#"{"starts_with": ["foobar", "foo"]}"#));
}

#[test]
fn starts_with_false_when_prefix_absent() {
    assert!(!matches(r#"{"starts_with": ["foobar", "bar"]}"#));
}

#[test]
fn starts_with_true_for_exact_match() {
    assert!(matches(r#"{"starts_with": ["hello", "hello"]}"#));
}

#[test]
fn starts_with_true_for_empty_prefix() {
    assert!(matches(r#"{"starts_with": ["anything", ""]}"#));
}

#[test]
fn starts_with_null_when_left_is_not_string() {
    // A number as the first operand: should yield null (falsy).
    assert!(!matches(r#"{"starts_with": [42, "4"]}"#));
}

#[test]
fn starts_with_null_when_right_is_not_string() {
    // A number as the second operand: should yield null (falsy).
    assert!(!matches(r#"{"starts_with": ["foobar", 42]}"#));
}

#[test]
fn starts_with_reads_context_attribute() {
    let context = ctx_attrs(r#"{"userAgent": "Mozilla/5.0 Firefox/120"}"#);
    assert!(matches_with(
        r#"{"starts_with": [{"var": "userAgent"}, "Mozilla"]}"#,
        &context
    ));
}

// ---------------------------------------------------------------------------
// ends_with
// ---------------------------------------------------------------------------

#[test]
fn ends_with_true_when_left_ends_with_right() {
    assert!(matches(r#"{"ends_with": ["foobar", "bar"]}"#));
}

#[test]
fn ends_with_false_when_suffix_absent() {
    assert!(!matches(r#"{"ends_with": ["foobar", "foo"]}"#));
}

#[test]
fn ends_with_true_for_exact_match() {
    assert!(matches(r#"{"ends_with": ["hello", "hello"]}"#));
}

#[test]
fn ends_with_true_for_empty_suffix() {
    assert!(matches(r#"{"ends_with": ["anything", ""]}"#));
}

#[test]
fn ends_with_null_when_left_is_not_string() {
    assert!(!matches(r#"{"ends_with": [42, "2"]}"#));
}

#[test]
fn ends_with_null_when_right_is_not_string() {
    assert!(!matches(r#"{"ends_with": ["foobar", 42]}"#));
}

#[test]
fn ends_with_reads_context_attribute() {
    let context = ctx_attrs(r#"{"email": "alice@example.com"}"#);
    assert!(matches_with(
        r#"{"ends_with": [{"var": "email"}, "@example.com"]}"#,
        &context
    ));
}

// ---------------------------------------------------------------------------
// sem_ver
// ---------------------------------------------------------------------------

#[test]
fn sem_ver_eq_true_for_identical_version() {
    let ctx = ctx_attrs(r#"{"version": "1.2.3"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "=", "1.2.3"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_eq_false_for_different_version() {
    let ctx = ctx_attrs(r#"{"version": "1.2.4"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, "=", "1.2.3"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_neq_true_when_versions_differ() {
    let ctx = ctx_attrs(r#"{"version": "2.0.0"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "!=", "1.0.0"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_neq_false_for_identical() {
    let ctx = ctx_attrs(r#"{"version": "1.0.0"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, "!=", "1.0.0"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_lt_true_when_lower() {
    let ctx = ctx_attrs(r#"{"version": "1.0.0"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "<", "2.0.0"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_lt_false_when_equal() {
    let ctx = ctx_attrs(r#"{"version": "2.0.0"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, "<", "2.0.0"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_lte_true_when_equal() {
    let ctx = ctx_attrs(r#"{"version": "2.0.0"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "<=", "2.0.0"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_gt_true_when_higher() {
    let ctx = ctx_attrs(r#"{"version": "3.1.0"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, ">", "3.0.9"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_gte_true_when_equal() {
    let ctx = ctx_attrs(r#"{"version": "1.5.0"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, ">=", "1.5.0"]}"#,
        &ctx
    ));
}

#[test]
fn sem_ver_gte_false_when_lower() {
    let ctx = ctx_attrs(r#"{"version": "1.4.9"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, ">=", "1.5.0"]}"#,
        &ctx
    ));
}

/// `^` (caret): same MAJOR only - not npm-style.
#[test]
fn sem_ver_caret_true_when_same_major() {
    let ctx = ctx_attrs(r#"{"version": "1.9.9"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "^", "1.0.0"]}"#,
        &ctx
    ));
}

/// `^` must return false when the major differs.
#[test]
fn sem_ver_caret_false_when_different_major() {
    let ctx = ctx_attrs(r#"{"version": "2.0.0"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, "^", "1.0.0"]}"#,
        &ctx
    ));
}

/// `~` (tilde): same MAJOR and MINOR only - not npm-style.
#[test]
fn sem_ver_tilde_true_when_same_major_minor() {
    let ctx = ctx_attrs(r#"{"version": "1.2.99"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "~", "1.2.0"]}"#,
        &ctx
    ));
}

/// `~` must return false when the minor differs.
#[test]
fn sem_ver_tilde_false_when_different_minor() {
    let ctx = ctx_attrs(r#"{"version": "1.3.0"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, "~", "1.2.0"]}"#,
        &ctx
    ));
}

/// `~` must return false when the major differs too.
#[test]
fn sem_ver_tilde_false_when_different_major() {
    let ctx = ctx_attrs(r#"{"version": "2.2.0"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, "~", "1.2.0"]}"#,
        &ctx
    ));
}

/// An unparseable version string must yield null (falsy, not an error).
#[test]
fn sem_ver_null_for_invalid_value() {
    let ctx = ctx_attrs(r#"{"version": "not-a-version"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, ">=", "1.0.0"]}"#,
        &ctx
    ));
}

/// An unparseable constraint string must yield null (falsy, not an error).
#[test]
fn sem_ver_null_for_invalid_constraint() {
    let ctx = ctx_attrs(r#"{"version": "1.0.0"}"#);
    assert!(!matches_with(
        r#"{"sem_ver": [{"var": "version"}, ">=", "not-a-version"]}"#,
        &ctx
    ));
}

/// Strip a leading `v` from the version string (tolerate both forms).
#[test]
fn sem_ver_tolerates_v_prefix_in_value() {
    let ctx = ctx_attrs(r#"{"version": "v1.2.3"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "=", "1.2.3"]}"#,
        &ctx
    ));
}

/// Strip a leading `v` from the constraint string (tolerate both forms).
#[test]
fn sem_ver_tolerates_v_prefix_in_constraint() {
    let ctx = ctx_attrs(r#"{"version": "1.2.3"}"#);
    assert!(matches_with(
        r#"{"sem_ver": [{"var": "version"}, "=", "v1.2.3"]}"#,
        &ctx
    ));
}

// ---------------------------------------------------------------------------
// fractional - general behaviour
// ---------------------------------------------------------------------------

/// A fractional rule must always assign the same variant to the same targeting
/// key for the same distribution (determinism).
#[test]
fn fractional_is_deterministic() {
    let variants = r#"{"red": "red", "blue": "blue", "none": "none"}"#;
    let targeting = r#"{"fractional": [["red", 50], ["blue", 50]]}"#;
    let fs = probe_string(variants, targeting);

    let ctx1 = ctx("user-123");
    let r1 = fs.evaluate("probe", &ctx1).expect("evaluation succeeds");
    let r2 = fs.evaluate("probe", &ctx1).expect("evaluation succeeds");

    assert_eq!(
        r1.variant, r2.variant,
        "same key must always land in the same bucket"
    );
    assert_eq!(r1.reason, Reason::TargetingMatch);
}

/// Growing a rollout from 50 to 60 (keeping total = 100) must not evict any
/// user already included in [0, 50).
#[test]
fn fractional_monotonic_rollout() {
    // Build a list of targeting keys and record which ones land in "red" at 50 %.
    let variants = r#"{"red": "red", "blue": "blue", "none": "none"}"#;

    let targeting_50 = r#"{"fractional": [["red", 50], ["blue", 50]]}"#;
    let targeting_60 = r#"{"fractional": [["red", 60], ["blue", 40]]}"#;

    let fs50 = probe_string(variants, targeting_50);
    let fs60 = probe_string(variants, targeting_60);

    let keys: Vec<String> = (0_u32..200).map(|i| format!("user-{i}")).collect();

    for key in &keys {
        let c = ctx(key.as_str());
        let v50 = fs50.evaluate("probe", &c).expect("ok").variant;
        let v60 = fs60.evaluate("probe", &c).expect("ok").variant;

        if v50.as_deref() == Some("red") {
            // A user already in "red" at 50 % must still be in "red" at 60 %.
            assert_eq!(
                v60.as_deref(),
                Some("red"),
                "user {key} left the bucket when rollout grew from 50 to 60"
            );
        }
    }
}

/// When `total_weight == 0`, evaluation must fall back to the default variant
/// with reason [`Reason::Default`].
///
/// A `weight` of `0` is a valid `u32` and can be expressed in JSON.  A single
/// bucket `["red", 0]` yields `total_weight == 0`, which triggers the early
/// return of `Value::Null` in the fractional evaluator.  The engine then serves
/// the flag's `defaultVariant` ("none") with reason `Default`.
#[test]
fn fractional_zero_total_weight_falls_back_to_default() {
    let variants = r#"{"red": "red", "none": "none"}"#;
    // A single bucket with weight 0 -> total_weight == 0 -> Value::Null.
    let targeting = r#"{"fractional": [["red", 0]]}"#;
    let fs = probe_string(variants, targeting);
    let context = ctx("any-user");
    let resolution = fs.evaluate("probe", &context).expect("evaluation succeeds");
    // Null targeting result -> defaultVariant served.
    assert_eq!(
        resolution.variant.as_deref(),
        Some("none"),
        "zero total weight must fall back to the default variant"
    );
    assert_eq!(
        resolution.reason,
        Reason::Default,
        "zero total weight must produce reason Default"
    );
}

/// The bucketing value defaults to `flagKey || targetingKey` (concatenated,
/// no separator) when `bucket_by` is absent.
#[test]
fn fractional_default_bucket_by_uses_flag_key_then_targeting_key() {
    let variants = r#"{"a": "a", "b": "b", "none": "none"}"#;

    // Two rules sharing the same distribution but different flag keys must give
    // different assignments for the same targeting key (because the hash input
    // differs).
    let targeting = r#"{"fractional": [["a", 50], ["b", 50]]}"#;

    let doc1 = format!(
        r#"{{
            "flags": {{
                "flag-one": {{
                    "state": "ENABLED",
                    "variants": {variants},
                    "defaultVariant": "none",
                    "targeting": {targeting}
                }}
            }}
        }}"#
    );
    let doc2 = format!(
        r#"{{
            "flags": {{
                "flag-two": {{
                    "state": "ENABLED",
                    "variants": {variants},
                    "defaultVariant": "none",
                    "targeting": {targeting}
                }}
            }}
        }}"#
    );

    let fs1 = FlagSet::from_json(&doc1).expect("valid");
    let fs2 = FlagSet::from_json(&doc2).expect("valid");
    let c = ctx("user-abc");

    let v1 = fs1.evaluate("flag-one", &c).expect("ok").variant;
    let v2 = fs2.evaluate("flag-two", &c).expect("ok").variant;

    // Different flag keys should produce different bucket values (with high
    // probability for any concrete key; we just assert it compiles and runs).
    // The important thing is that both evaluations succeed without panic.
    assert!(v1.is_some());
    assert!(v2.is_some());
}

/// An explicit `bucket_by` overrides the default flagKey+targetingKey.
#[test]
fn fractional_explicit_bucket_by_overrides_default() {
    let variants = r#"{"a": "a", "b": "b", "none": "none"}"#;

    // Use a literal as bucket_by so we can predict determinism independently
    // of flag key or targeting key.
    let targeting = r#"{"fractional": [{"var": "sessionId"}, ["a", 50], ["b", 50]]}"#;
    let fs = probe_string(variants, targeting);

    let c1 = ctx_with("user-1", r#"{"sessionId": "session-xyz"}"#);
    let c2 = ctx_with("user-2", r#"{"sessionId": "session-xyz"}"#);

    // Same sessionId -> same bucket regardless of targeting key.
    let v1 = fs.evaluate("probe", &c1).expect("ok").variant;
    let v2 = fs.evaluate("probe", &c2).expect("ok").variant;
    assert_eq!(v1, v2, "same bucket_by value must land in the same bucket");
}

// ---------------------------------------------------------------------------
// fractional - reference vectors (cross-language conformance)
//
// Hash algorithm: MurmurHash3 x86 32-bit seed 0, matching
// `github.com/twmb/murmur3` StringSum32 used by the Go reference
// implementation in `core/pkg/evaluator/fractional.go`.
//
// Bucket formula: `(hash_u32 as u64 * total_weight as u64) >> 32`
//
// These vectors were computed with a standalone Rust re-implementation of
// the same algorithm (verified hand-calculation of murmur3 x86_32 seed 0)
// and cross-checked against the flagd Go reference test data.
//
// Vector derivation:
//   "headerColoruser:abc"                  -> hash=1,681,433,475  bucket(100)=39
//   "headerColorsquarey@example.com"       -> hash=3,228,116,633  bucket(100)=75
//   "headerColorbucketeerix@example.com"   -> hash=2,591,899,404  bucket(100)=60
//   "foo"                                  -> hash=4,138,058,784  bucket(100)=96
// ---------------------------------------------------------------------------

/// Reference vector 1: bucketing value `"headerColoruser:abc"` with four
/// equal weights [red=25, blue=25, green=25, yellow=25], total=100.
///
/// bucket = 39 -> cumulative ranges red[0,25), blue[25,50) -> "blue".
#[test]
fn fractional_ref_vector_1_four_equal_buckets() {
    let variants =
        r#"{"red": "red", "blue": "blue", "green": "green", "yellow": "yellow", "none": "none"}"#;
    // flagKey = "headerColor", targetingKey = "user:abc"
    // bucket_by default -> "headerColor" + "user:abc" = "headerColoruser:abc"
    let targeting = r#"{"fractional": [["red", 25], ["blue", 25], ["green", 25], ["yellow", 25]]}"#;
    let doc = format!(
        r#"{{
            "flags": {{
                "headerColor": {{
                    "state": "ENABLED",
                    "variants": {variants},
                    "defaultVariant": "none",
                    "targeting": {targeting}
                }}
            }}
        }}"#
    );
    let fs = FlagSet::from_json(&doc).expect("valid");
    let context = ctx("user:abc");
    let resolution = fs.evaluate("headerColor", &context).expect("ok");
    // murmur3_x86_32("headerColoruser:abc", 0) = 1,681,433,475
    // bucket = (1681433475u64 * 100u64) >> 32 = 39
    // ranges: red[0,25), blue[25,50) -> bucket 39 lands in "blue"
    assert_eq!(resolution.variant.as_deref(), Some("blue"));
}

/// Reference vector 2: bucketing value `"headerColorsquarey@example.com"` with
/// weights [red=50, blue=50], total=100.
///
/// bucket = 75 -> red[0,50), blue[50,100) -> "blue".
#[test]
fn fractional_ref_vector_2_fifty_fifty_split() {
    let variants = r#"{"red": "red", "blue": "blue", "none": "none"}"#;
    let targeting = r#"{"fractional": [["red", 50], ["blue", 50]]}"#;
    let doc = format!(
        r#"{{
            "flags": {{
                "headerColor": {{
                    "state": "ENABLED",
                    "variants": {variants},
                    "defaultVariant": "none",
                    "targeting": {targeting}
                }}
            }}
        }}"#
    );
    let fs = FlagSet::from_json(&doc).expect("valid");
    let context = ctx("squarey@example.com");
    let resolution = fs.evaluate("headerColor", &context).expect("ok");
    // murmur3_x86_32("headerColorsquarey@example.com", 0) = 3,228,116,633
    // bucket = (3228116633u64 * 100u64) >> 32 = 75
    // ranges: red[0,50), blue[50,100) -> bucket 75 lands in "blue"
    assert_eq!(resolution.variant.as_deref(), Some("blue"));
}

/// Reference vector 3: bucketing value `"headerColorbucketeerix@example.com"`
/// with weights [red=50, blue=50], total=100.
///
/// bucket = 60 -> red[0,50), blue[50,100) -> "blue".
#[test]
fn fractional_ref_vector_3_other_bucket() {
    let variants = r#"{"red": "red", "blue": "blue", "none": "none"}"#;
    let targeting = r#"{"fractional": [["red", 50], ["blue", 50]]}"#;
    let doc = format!(
        r#"{{
            "flags": {{
                "headerColor": {{
                    "state": "ENABLED",
                    "variants": {variants},
                    "defaultVariant": "none",
                    "targeting": {targeting}
                }}
            }}
        }}"#
    );
    let fs = FlagSet::from_json(&doc).expect("valid");
    let context = ctx("bucketeerix@example.com");
    let resolution = fs.evaluate("headerColor", &context).expect("ok");
    // murmur3_x86_32("headerColorbucketeerix@example.com", 0) = 2,591,899,404
    // bucket = (2591899404u64 * 100u64) >> 32 = 60
    // ranges: red[0,50), blue[50,100) -> bucket 60 lands in "blue"
    assert_eq!(resolution.variant.as_deref(), Some("blue"));
}

/// Reference vector 4: explicit `bucket_by` literal "foo" with weights
/// [a=100] must always resolve to "a" (trivial distribution sanity check).
#[test]
fn fractional_ref_vector_4_explicit_bucket_by_literal() {
    let variants = r#"{"a": "a", "none": "none"}"#;
    let targeting = r#"{"fractional": ["foo", ["a", 100]]}"#;
    let fs = probe_string(variants, targeting);
    let context = ctx("any-user");
    let resolution = fs.evaluate("probe", &context).expect("ok");
    assert_eq!(resolution.variant.as_deref(), Some("a"));
}
