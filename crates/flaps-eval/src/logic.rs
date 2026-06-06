//! Pure JsonLogic interpreter over the targeting AST.
//!
//! Implements the JsonLogic reference semantics, which carry their own
//! cross-language rules: a dedicated truthiness table where empty arrays
//! and empty strings are falsy but `"0"` is truthy, loose equality with
//! JavaScript style type coercion, and operators that never fail.
//! Structurally valid rules always produce a value; invalid operands
//! degrade to falsy or nullish results.
//!
//! This module knows nothing about flags or variants: it reduces a
//! [`Rule`] against a data value. The flagd specific semantics live in
//! [`crate::eval`].

use crate::eval::EvaluationError;
use crate::targeting::Rule;

/// Reduces a rule against the current data scope.
///
/// The data scope is the evaluation context object at the root, and is
/// rebound inside `map`, `filter`, `reduce`, `all`, `none` and `some` to
/// the element under iteration.
///
/// # Errors
///
/// Returns [`EvaluationError::UnsupportedOperation`] when the rule reaches
/// a flagd custom operation that is not implemented yet. The JsonLogic
/// operators themselves never fail.
pub(crate) fn apply(
    rule: &Rule,
    data: &serde_json::Value,
) -> Result<serde_json::Value, EvaluationError> {
    let _ = (rule, data);
    todo!()
}

/// Decides truthiness per the JsonLogic specification.
///
/// `null`, `false`, `0`, empty strings and empty arrays are falsy; every
/// other value, including the string `"0"` and empty objects, is truthy.
pub(crate) fn truthy(value: &serde_json::Value) -> bool {
    let _ = value;
    todo!()
}

/// Compares two values with JavaScript style loose equality.
///
/// Implements the abstract equality algorithm over JSON types: numbers and
/// strings compare through numeric coercion, booleans coerce to numbers,
/// and arrays and objects coerce to their string form before comparing
/// against primitives.
fn loose_eq(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    let _ = (left, right);
    todo!()
}

/// Compares two values with strict equality: equal types and equal values.
fn strict_eq(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    let _ = (left, right);
    todo!()
}

/// Coerces a value to a number, yielding `NaN` for inconvertible values.
fn to_number(value: &serde_json::Value) -> f64 {
    let _ = value;
    todo!()
}

/// Coerces a value to its string form, mirroring JavaScript coercion.
fn to_string(value: &serde_json::Value) -> String {
    let _ = value;
    todo!()
}

/// Resolves a `var` path against the data scope.
///
/// Supports dotted paths, numeric array indexes and the empty path, which
/// yields the whole scope.
fn lookup(path: &str, data: &serde_json::Value) -> Option<serde_json::Value> {
    let _ = (path, data);
    todo!()
}
