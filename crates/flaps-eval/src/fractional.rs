//! Evaluation of the flagd `fractional` custom operator.
//!
//! Deterministically assigns an evaluation to one of a set of weighted variant
//! buckets using the following algorithm (matching the flagd Go reference
//! implementation in `core/pkg/evaluator/fractional.go`):
//!
//! 1. Determine the bucketing value: if `bucket_by` is present and evaluates
//!    to a string, use that string; otherwise concatenate the flag key and the
//!    targeting key (flag key first, no separator).
//! 2. Hash the bucketing value with `MurmurHash3` x86 32-bit, seed 0.
//! 3. Map the hash into `[0, total_weight)` using pure integer arithmetic:
//!    `bucket = (hash as u64 * total_weight as u64) >> 32`.
//! 4. Walk the buckets accumulating cumulative weights; return the first
//!    variant whose cumulative weight exceeds the bucket value.
//!
//! Non-conforming inputs degrade to `Value::Null` rather than propagating
//! errors, matching the flagd semantics for out-of-spec inputs.

use serde_json::Value;

use crate::eval::EvaluationError;
use crate::logic::apply;
use crate::targeting::{Bucket, Rule};

/// Computes the `MurmurHash3` x86 32-bit hash of `data` with the given `seed`.
///
/// Canonical Austin Appleby algorithm, matching `twmb/murmur3` `Sum32` used by
/// the flagd reference implementation so bucketing agrees across languages.
fn murmur3_x86_32(data: &[u8], seed: u32) -> u32 {
    const C1: u32 = 0xcc9e_2d51;
    const C2: u32 = 0x1b87_3593;

    let mut hash = seed;
    let mut chunks = data.chunks_exact(4);
    for chunk in &mut chunks {
        let mut k = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k = k.wrapping_mul(C1);
        k = k.rotate_left(15);
        k = k.wrapping_mul(C2);
        hash ^= k;
        hash = hash.rotate_left(13);
        hash = hash.wrapping_mul(5).wrapping_add(0xe654_6b64);
    }

    let tail = chunks.remainder();
    let mut k: u32 = 0;
    if let Some(&b) = tail.get(2) {
        k ^= u32::from(b) << 16;
    }
    if let Some(&b) = tail.get(1) {
        k ^= u32::from(b) << 8;
    }
    if let Some(&b) = tail.first() {
        k ^= u32::from(b);
        k = k.wrapping_mul(C1);
        k = k.rotate_left(15);
        k = k.wrapping_mul(C2);
        hash ^= k;
    }

    #[allow(clippy::cast_possible_truncation)]
    // MurmurHash3 mixes the low 32 bits of the input length by design.
    let len = data.len() as u32;
    hash ^= len;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(0x85eb_ca6b);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(0xc2b2_ae35);
    hash ^= hash >> 16;
    hash
}

/// Resolves the bucketing value for a `fractional` rule.
///
/// When `bucket_by` is absent or does not evaluate to a string, falls back to
/// the flagd default: the flag key concatenated with the targeting key
/// (flag key first, no separator).
fn bucketing_value(bucket_by: Option<&Rule>, data: &Value) -> Result<String, EvaluationError> {
    if let Some(rule) = bucket_by {
        let evaluated = apply(rule, data)?;
        if let Value::String(text) = evaluated {
            return Ok(text);
        }
    }

    // Default: flagKey concatenated with targetingKey.
    let flag_key = data
        .get("$flagd")
        .and_then(|flagd| flagd.get("flagKey"))
        .and_then(Value::as_str)
        .unwrap_or("");

    let targeting_key = data
        .get("targetingKey")
        .and_then(Value::as_str)
        .unwrap_or("");

    Ok(format!("{flag_key}{targeting_key}"))
}

/// Hashes `value` with `MurmurHash3` x86 32-bit seed 0 and maps the result
/// into `[0, total_weight)` using the high-precision integer formula.
///
/// Formula: `(hash as u64 * total_weight as u64) >> 32`
///
/// This matches the Go reference implementation and avoids the float-division
/// rounding errors present in older SDK implementations.
fn murmur3_bucket(value: &str, total_weight: u64) -> u64 {
    let hash = murmur3_x86_32(value.as_bytes(), 0);
    (u64::from(hash) * total_weight) >> 32
}

/// Evaluates a `fractional` rule against the evaluation scope.
///
/// Returns `Value::Null` when `total_weight` is zero (no buckets or all
/// weights zero) or when the bucketing value cannot be resolved.
pub(crate) fn eval_fractional(
    bucket_by: Option<&Rule>,
    buckets: &[Bucket],
    data: &Value,
) -> Result<Value, EvaluationError> {
    let total_weight: u64 = buckets.iter().map(|b| u64::from(b.weight)).sum();

    if total_weight == 0 {
        return Ok(Value::Null);
    }

    let value = bucketing_value(bucket_by, data)?;
    let bucket = murmur3_bucket(&value, total_weight);

    let mut range_end: u64 = 0;
    for b in buckets {
        range_end += u64::from(b.weight);
        if bucket < range_end {
            return Ok(Value::String(b.variant.clone()));
        }
    }

    // Unreachable when total_weight > 0 and the bucket is in [0, total_weight),
    // but degrade gracefully rather than panic.
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::murmur3_x86_32;

    /// Verifies cross-language hash conformance against known reference vectors.
    ///
    /// These values match `twmb/murmur3` `Sum32` (seed 0) used by the flagd Go
    /// reference implementation.  Any regression here breaks bucketing
    /// compatibility with the reference implementation.
    #[test]
    fn murmur3_empty_string() {
        assert_eq!(murmur3_x86_32(b"", 0), 0);
    }

    #[test]
    fn murmur3_hello() {
        assert_eq!(murmur3_x86_32(b"hello", 0), 0x248b_fa47);
    }

    #[test]
    fn murmur3_header_color_user_abc() {
        assert_eq!(murmur3_x86_32(b"headerColoruser:abc", 0), 1_681_433_475);
    }

    #[test]
    fn murmur3_header_color_squarey() {
        assert_eq!(
            murmur3_x86_32(b"headerColorsquarey@example.com", 0),
            3_228_116_633
        );
    }

    #[test]
    fn murmur3_header_color_bucketeerix() {
        assert_eq!(
            murmur3_x86_32(b"headerColorbucketeerix@example.com", 0),
            2_591_899_404
        );
    }

    #[test]
    fn murmur3_foo() {
        assert_eq!(murmur3_x86_32(b"foo", 0), 4_138_058_784);
    }
}
