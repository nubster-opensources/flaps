//! Evaluation of the flagd `sem_ver` custom operator.
//!
//! Compares two semantic version strings with a given operator.  The
//! comparison follows the flagd semantics rather than npm-style `VersionReq`:
//!
//! - `=`, `!=`, `<`, `<=`, `>`, `>=`: standard ordering via `semver::Version`.
//! - `^` (caret): same major version only (`a.major == b.major`).
//! - `~` (tilde): same major and minor (`a.major == b.major && a.minor == b.minor`).
//!
//! Leading `v`/`V` prefixes are stripped before parsing so both `1.2.3` and
//! `v1.2.3` are accepted.  An unparseable version yields `Value::Null`
//! (falsy) rather than propagating an error.

use semver::Version;
use serde_json::Value;

use crate::eval::EvaluationError;
use crate::logic::apply;
use crate::targeting::{Rule, SemVerOp};

/// Evaluates `sem_ver` by reducing `value` and `version`, then comparing
/// the two parsed semantic versions with the requested operator.
///
/// Returns `Value::Null` when either operand is not a parseable semver string.
pub(crate) fn eval_sem_ver(
    value: &Rule,
    op: SemVerOp,
    version: &Rule,
    data: &Value,
) -> Result<Value, EvaluationError> {
    let lhs_val = apply(value, data)?;
    let rhs_val = apply(version, data)?;

    let (Some(lhs_str), Some(rhs_str)) = (as_str(&lhs_val), as_str(&rhs_val)) else {
        return Ok(Value::Null);
    };

    let (Some(lhs), Some(rhs)) = (parse_version(lhs_str), parse_version(rhs_str)) else {
        return Ok(Value::Null);
    };

    let result = match op {
        SemVerOp::Eq => lhs == rhs,
        SemVerOp::Neq => lhs != rhs,
        SemVerOp::Lt => lhs < rhs,
        SemVerOp::Lte => lhs <= rhs,
        SemVerOp::Gt => lhs > rhs,
        SemVerOp::Gte => lhs >= rhs,
        // `^`: same major version only (NOT npm caret semantics).
        SemVerOp::CaretMatch => lhs.major == rhs.major,
        // `~`: same major AND minor (NOT npm tilde semantics).
        SemVerOp::TildeMatch => lhs.major == rhs.major && lhs.minor == rhs.minor,
    };

    Ok(Value::Bool(result))
}

/// Extracts the string content of a JSON value, or `None` when it is not
/// a string.
fn as_str(value: &Value) -> Option<&str> {
    value.as_str()
}

/// Parses a semantic version string, stripping an optional leading `v`/`V`.
fn parse_version(raw: &str) -> Option<Version> {
    let stripped = raw.strip_prefix(['v', 'V']).unwrap_or(raw);
    Version::parse(stripped).ok()
}
