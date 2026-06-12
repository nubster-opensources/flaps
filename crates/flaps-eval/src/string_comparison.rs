//! Evaluation of the flagd `starts_with` and `ends_with` custom operators.
//!
//! Both operators evaluate two string operands and compare them.  When either
//! operand evaluates to a non-string JSON value the result degrades to
//! `Value::Null` (falsy in JsonLogic) rather than propagating an error,
//! matching the flagd reference semantics for non-conforming inputs.

use serde_json::Value;

use crate::eval::EvaluationError;
use crate::logic::apply;
use crate::targeting::Rule;

/// Identifies which end of the left string is compared.
#[derive(Clone, Copy)]
pub(crate) enum Affix {
    /// The left string must start with the right string.
    Prefix,
    /// The left string must end with the right string.
    Suffix,
}

/// Evaluates `starts_with` or `ends_with` by first reducing both operands,
/// then requiring both to be JSON strings.
///
/// Returns `Value::Null` when either operand is not a string.
pub(crate) fn eval_string_comparison(
    affix: Affix,
    left: &Rule,
    right: &Rule,
    data: &Value,
) -> Result<Value, EvaluationError> {
    let left_val = apply(left, data)?;
    let right_val = apply(right, data)?;

    match (left_val, right_val) {
        (Value::String(haystack), Value::String(needle)) => {
            let result = match affix {
                Affix::Prefix => haystack.starts_with(needle.as_str()),
                Affix::Suffix => haystack.ends_with(needle.as_str()),
            };
            Ok(Value::Bool(result))
        }
        _ => Ok(Value::Null),
    }
}
