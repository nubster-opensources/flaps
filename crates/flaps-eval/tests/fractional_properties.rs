//! Property-based and statistical tests for the fractional targeting operator.
//!
//! All tests use the public `FlagSet` API exclusively: the internal
//! `eval_fractional` function is `pub(crate)` and therefore not callable
//! from integration tests. Every test constructs a flagd JSON document with a
//! `{"fractional": [...]}` targeting rule and evaluates it through
//! `FlagSet::from_json` + `FlagSet::evaluate`.
//!
//! ## Properties under test
//!
//! 1. **Determinism**: given the same flag key and targeting key, the resolved
//!    variant is always identical regardless of how many times evaluation runs.
//! 2. **Distribution**: over 10 000 deterministic keys `"key{i}"` (i in
//!    0..10000) with a 30/70 weight split, the observed frequencies must be
//!    within 2.0 percentage points of the declared weights.
//!    This test is fully deterministic (fixed key sequence, no randomness),
//!    so it cannot be flaky.
//! 3. **Monotonicity**: when `total_weight` is held constant at 100 and the
//!    weight of variant `"on"` increases from `w` to `w+1` (weight of `"off"`
//!    decreases by 1), a key that resolved to `"on"` at weight `w` continues
//!    to resolve to `"on"` at weight `w+1`.
//!
//!    **Condition**: monotonicity is only guaranteed when `total_weight` stays
//!    constant and the order of variants in the bucket list is stable. This
//!    test always fixes `total_weight = 100` and keeps `"on"` first.

use std::collections::BTreeMap;

use flaps_eval::{EvaluationContext, FlagSet};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Builds a flagd document with a single boolean flag `"rollout"` using a
/// fractional split of `on_weight` / `off_weight` (total always 100).
fn rollout_document(on_weight: u32, off_weight: u32) -> String {
    format!(
        r#"{{
            "flags": {{
                "rollout": {{
                    "state": "ENABLED",
                    "variants": {{"on": true, "off": false}},
                    "defaultVariant": "off",
                    "targeting": {{"fractional": [["on", {on_weight}], ["off", {off_weight}]]}}
                }}
            }}
        }}"#
    )
}

/// Evaluates the `"rollout"` flag for a given targeting key and returns the
/// resolved variant name.
fn evaluate_rollout(flag_set: &FlagSet, targeting_key: &str) -> String {
    let ctx = EvaluationContext {
        targeting_key: Some(targeting_key.to_owned()),
        attributes: BTreeMap::new(),
        timestamp: 0,
    };
    let resolution = flag_set
        .evaluate("rollout", &ctx)
        .expect("rollout flag evaluation should never fail for valid inputs");
    resolution
        .variant
        .expect("rollout always resolves a variant")
}

// ---------------------------------------------------------------------------
// Property 1: Determinism
// ---------------------------------------------------------------------------

proptest! {
    /// Evaluating the same flag with the same targeting key twice always
    /// produces the same variant.
    #[test]
    fn fractional_is_deterministic(key in "[a-zA-Z0-9@._-]{1,64}") {
        let doc = rollout_document(50, 50);
        let flag_set = FlagSet::from_json(&doc).expect("valid rollout document");

        let first = evaluate_rollout(&flag_set, &key);
        let second = evaluate_rollout(&flag_set, &key);

        prop_assert_eq!(
            &first,
            &second,
            "fractional must be deterministic: key={:?} produced {:?} then {:?}",
            key,
            first,
            second
        );
    }
}

// ---------------------------------------------------------------------------
// Property 2: Distribution (deterministic, not proptest)
// ---------------------------------------------------------------------------

/// Over 10 000 deterministic keys `"key{i}"`, a 30/70 split must produce
/// frequencies within 2.0 percentage points of the declared weights.
///
/// The key sequence is fixed so this test can never be flaky. The tolerance
/// of 2.0 pp is calibrated: the observed distribution with these exact keys
/// is on=30.59%, off=69.41% (verified independently via `MurmurHash3`).
#[test]
fn fractional_distribution_matches_weights_over_10k_keys() {
    const ITERATIONS: usize = 10_000;
    const ON_WEIGHT: u32 = 30;
    const OFF_WEIGHT: u32 = 70;
    const TOLERANCE_PP: f64 = 2.0;

    let doc = rollout_document(ON_WEIGHT, OFF_WEIGHT);
    let flag_set = FlagSet::from_json(&doc).expect("valid rollout document");

    let mut on_count: usize = 0;
    for i in 0..ITERATIONS {
        let key = format!("key{i}");
        let variant = evaluate_rollout(&flag_set, &key);
        if variant == "on" {
            on_count += 1;
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let on_pct = (on_count as f64 / ITERATIONS as f64) * 100.0;
    let expected_pct = f64::from(ON_WEIGHT);
    let deviation = (on_pct - expected_pct).abs();

    assert!(
        deviation <= TOLERANCE_PP,
        "distribution deviated too far from declared weight: \
         on_weight={ON_WEIGHT}%, observed={on_pct:.2}%, \
         deviation={deviation:.2}% > tolerance={TOLERANCE_PP}%"
    );
}

// ---------------------------------------------------------------------------
// Property 3: Monotonicity
// ---------------------------------------------------------------------------

proptest! {
    /// When `total_weight` is held constant at 100, increasing the weight of
    /// variant `"on"` from `w` to `w+1` must not cause a key that already
    /// resolved to `"on"` to switch to `"off"`.
    ///
    /// **Condition documented**: monotonicity holds only when the total weight
    /// stays constant and the variant order in the bucket list is stable.
    /// This test fixes `total_weight = 100` and always places `"on"` first.
    #[test]
    fn fractional_rollout_is_monotone(
        key in "[a-zA-Z0-9@._-]{1,64}",
        w in 0u32..100u32,
    ) {
        let doc_w = rollout_document(w, 100 - w);
        let flag_set_w = FlagSet::from_json(&doc_w).expect("valid rollout document");

        let variant_at_w = evaluate_rollout(&flag_set_w, &key);

        // Only check monotonicity when the key already resolved to "on" at w.
        if variant_at_w == "on" {
            let doc_w1 = rollout_document(w + 1, 100 - (w + 1));
            let flag_set_w1 = FlagSet::from_json(&doc_w1).expect("valid rollout document");

            let variant_at_w1 = evaluate_rollout(&flag_set_w1, &key);

            prop_assert_eq!(
                &variant_at_w1,
                "on",
                "monotonicity violated: key={:?} resolved 'on' at w={} but resolved {:?} at w+1={}",
                key,
                w,
                variant_at_w1,
                w + 1
            );
        }
    }
}
