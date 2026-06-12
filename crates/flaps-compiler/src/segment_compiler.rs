//! Translates domain [`SegmentMatch`] expressions into `flaps-eval` [`Rule`]s.

use flaps_domain::segment::{MatchOperator, Predicate, SegmentMatch};
use flaps_eval::{Literal, Rule, SemVerOp};

use crate::error::CompileError;

/// Compiles a [`SegmentMatch`] into its equivalent [`Rule`].
///
/// # Errors
/// - [`CompileError::PredicateArity`] when a predicate has the wrong number of values.
/// - [`CompileError::NonScalarPredicateValue`] when a scalar operator receives an array or object.
pub(crate) fn compile_segment_match(m: &SegmentMatch) -> Result<Rule, CompileError> {
    match m {
        SegmentMatch::And(children) => {
            let rules = children
                .iter()
                .map(compile_segment_match)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Rule::And(rules))
        }
        SegmentMatch::Or(children) => {
            let rules = children
                .iter()
                .map(compile_segment_match)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Rule::Or(rules))
        }
        SegmentMatch::Not(inner) => {
            let inner_rule = compile_segment_match(inner)?;
            Ok(Rule::Not(Box::new(inner_rule)))
        }
        SegmentMatch::Predicate(p) => compile_predicate(p),
    }
}

/// Converts a [`serde_json::Value`] to a [`Literal`], rejecting non-scalars.
fn json_to_literal(v: &serde_json::Value, operator: &str) -> Result<Literal, CompileError> {
    match v {
        serde_json::Value::Null => Ok(Literal::Null),
        serde_json::Value::Bool(b) => Ok(Literal::Bool(*b)),
        serde_json::Value::Number(n) => Ok(Literal::Number(n.as_f64().unwrap_or(0.0))),
        serde_json::Value::String(s) => Ok(Literal::String(s.clone())),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Err(CompileError::NonScalarPredicateValue {
                operator: operator.to_owned(),
            })
        }
    }
}

/// Converts a list of [`serde_json::Value`]s into [`Rule::Array`] of literals.
fn json_array_to_rule_array(
    values: &[serde_json::Value],
    operator: &str,
) -> Result<Rule, CompileError> {
    let literals = values
        .iter()
        .map(|v| json_to_literal(v, operator).map(Rule::Literal))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Rule::Array(literals))
}

/// Compiles a [`Predicate`] into its flagd [`Rule`] equivalent.
fn compile_predicate(p: &Predicate) -> Result<Rule, CompileError> {
    let op_name = format!("{:?}", p.operator);
    let attr_rule = Box::new(Rule::Var {
        path: p.attribute.clone(),
        default: None,
    });

    match p.operator {
        // Arity = exactly 1 scalar value
        MatchOperator::Equals => {
            require_arity(&p.values, 1, &op_name)?;
            let lit = json_to_literal(&p.values[0], &op_name)?;
            Ok(Rule::Eq(attr_rule, Box::new(Rule::Literal(lit))))
        }
        MatchOperator::NotEquals => {
            require_arity(&p.values, 1, &op_name)?;
            let lit = json_to_literal(&p.values[0], &op_name)?;
            Ok(Rule::Neq(attr_rule, Box::new(Rule::Literal(lit))))
        }
        MatchOperator::StartsWith => {
            require_arity(&p.values, 1, &op_name)?;
            let lit = json_to_literal(&p.values[0], &op_name)?;
            Ok(Rule::StartsWith(attr_rule, Box::new(Rule::Literal(lit))))
        }
        MatchOperator::EndsWith => {
            require_arity(&p.values, 1, &op_name)?;
            let lit = json_to_literal(&p.values[0], &op_name)?;
            Ok(Rule::EndsWith(attr_rule, Box::new(Rule::Literal(lit))))
        }
        // Contains: In(Literal(v), Var) -- substring check
        MatchOperator::Contains => {
            require_arity(&p.values, 1, &op_name)?;
            let lit = json_to_literal(&p.values[0], &op_name)?;
            Ok(Rule::In(Box::new(Rule::Literal(lit)), attr_rule))
        }
        // Arity = >= 1 (any list)
        MatchOperator::In => {
            require_arity_min(&p.values, 1, &op_name)?;
            let arr = json_array_to_rule_array(&p.values, &op_name)?;
            Ok(Rule::In(attr_rule, Box::new(arr)))
        }
        MatchOperator::NotIn => {
            require_arity_min(&p.values, 1, &op_name)?;
            let arr = json_array_to_rule_array(&p.values, &op_name)?;
            Ok(Rule::Not(Box::new(Rule::In(attr_rule, Box::new(arr)))))
        }
        // SemVer operators: arity = exactly 1 scalar string value
        MatchOperator::SemVerEq => compile_semver(p, SemVerOp::Eq, attr_rule, &op_name),
        MatchOperator::SemVerNeq => compile_semver(p, SemVerOp::Neq, attr_rule, &op_name),
        MatchOperator::SemVerLt => compile_semver(p, SemVerOp::Lt, attr_rule, &op_name),
        MatchOperator::SemVerLte => compile_semver(p, SemVerOp::Lte, attr_rule, &op_name),
        MatchOperator::SemVerGt => compile_semver(p, SemVerOp::Gt, attr_rule, &op_name),
        MatchOperator::SemVerGte => compile_semver(p, SemVerOp::Gte, attr_rule, &op_name),
        MatchOperator::SemVerCaret => compile_semver(p, SemVerOp::CaretMatch, attr_rule, &op_name),
        MatchOperator::SemVerTilde => compile_semver(p, SemVerOp::TildeMatch, attr_rule, &op_name),
    }
}

/// Builds a [`Rule::SemVer`] node after validating the arity.
fn compile_semver(
    p: &Predicate,
    op: SemVerOp,
    attr_rule: Box<Rule>,
    op_name: &str,
) -> Result<Rule, CompileError> {
    require_arity(&p.values, 1, op_name)?;
    let lit = json_to_literal(&p.values[0], op_name)?;
    Ok(Rule::SemVer {
        value: attr_rule,
        op,
        version: Box::new(Rule::Literal(lit)),
    })
}

/// Validates that `values` has exactly `expected` elements.
fn require_arity(
    values: &[serde_json::Value],
    expected: usize,
    operator: &str,
) -> Result<(), CompileError> {
    if values.len() != expected {
        return Err(CompileError::PredicateArity {
            operator: operator.to_owned(),
            expected: expected.to_string(),
            got: values.len(),
        });
    }
    Ok(())
}

/// Validates that `values` has at least `min` elements.
fn require_arity_min(
    values: &[serde_json::Value],
    min: usize,
    operator: &str,
) -> Result<(), CompileError> {
    if values.len() < min {
        return Err(CompileError::PredicateArity {
            operator: operator.to_owned(),
            expected: format!(">={min}"),
            got: values.len(),
        });
    }
    Ok(())
}
