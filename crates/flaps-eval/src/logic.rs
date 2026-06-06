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
//!
//! Two divergences from the reference implementation are assumed and
//! covered by tests: non finite intermediate numbers materialize as
//! `null` because JSON cannot represent them, and `substr` counts
//! characters rather than UTF-16 code units.

use serde_json::{Value, json};

use crate::eval::EvaluationError;
use crate::targeting::{Literal, Rule};

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
pub(crate) fn apply(rule: &Rule, data: &Value) -> Result<Value, EvaluationError> {
    match rule {
        Rule::Literal(literal) => Ok(literal_value(literal)),
        Rule::Array(items) => Ok(Value::Array(apply_all(items, data)?)),
        Rule::Var { path, default } => Ok(eval_var(path, default.as_ref(), data)),
        Rule::Missing(keys) => Ok(Value::Array(eval_missing(keys, data)?.1)),
        Rule::MissingSome { min, keys } => eval_missing_some(*min, keys, data),
        Rule::If(branches) => eval_if(branches, data),
        Rule::And(operands) => eval_and(operands, data),
        Rule::Or(operands) => eval_or(operands, data),
        Rule::Not(operand) => Ok(Value::Bool(!truthy(&apply(operand, data)?))),
        Rule::Truthy(operand) => Ok(Value::Bool(truthy(&apply(operand, data)?))),
        Rule::Eq(left, right) => Ok(Value::Bool(loose_eq(
            &apply(left, data)?,
            &apply(right, data)?,
        ))),
        Rule::StrictEq(left, right) => Ok(Value::Bool(strict_eq(
            &apply(left, data)?,
            &apply(right, data)?,
        ))),
        Rule::Neq(left, right) => Ok(Value::Bool(!loose_eq(
            &apply(left, data)?,
            &apply(right, data)?,
        ))),
        Rule::StrictNeq(left, right) => Ok(Value::Bool(!strict_eq(
            &apply(left, data)?,
            &apply(right, data)?,
        ))),
        Rule::Gt(left, right) => Ok(Value::Bool(lt(&apply(right, data)?, &apply(left, data)?))),
        Rule::Gte(left, right) => Ok(Value::Bool(lte(&apply(right, data)?, &apply(left, data)?))),
        Rule::Lt(operands) => eval_chain(operands, data, lt),
        Rule::Lte(operands) => eval_chain(operands, data, lte),
        Rule::Add(operands) => eval_add(operands, data),
        Rule::Sub(operands) => eval_sub(operands, data),
        Rule::Mul(operands) => eval_mul(operands, data),
        Rule::Div(left, right) => Ok(number_value(
            to_number(&apply(left, data)?) / to_number(&apply(right, data)?),
        )),
        Rule::Mod(left, right) => Ok(number_value(
            to_number(&apply(left, data)?) % to_number(&apply(right, data)?),
        )),
        Rule::Min(operands) => eval_extreme(operands, data, f64::min),
        Rule::Max(operands) => eval_extreme(operands, data, f64::max),
        Rule::Cat(operands) => eval_cat(operands, data),
        Rule::Substr(operands) => eval_substr(operands, data),
        Rule::In(needle, haystack) => eval_in(needle, haystack, data),
        Rule::Merge(operands) => eval_merge(operands, data),
        Rule::Map(array, logic) => eval_map(array, logic, data),
        Rule::Filter(array, logic) => eval_filter(array, logic, data),
        Rule::Reduce(array, logic, initial) => eval_reduce(array, logic, initial, data),
        Rule::All(array, test) => eval_all(array, test, data),
        Rule::None(array, test) => Ok(Value::Bool(!truthy(&eval_some(array, test, data)?))),
        Rule::Some(array, test) => eval_some(array, test, data),
        Rule::StartsWith(_, _) => Err(unsupported("starts_with")),
        Rule::EndsWith(_, _) => Err(unsupported("ends_with")),
        Rule::SemVer { .. } => Err(unsupported("sem_ver")),
        Rule::Fractional { .. } => Err(unsupported("fractional")),
        Rule::Ref(_) => Err(unsupported("$ref")),
    }
}

/// Decides truthiness per the JsonLogic specification.
///
/// `null`, `false`, `0`, empty strings and empty arrays are falsy; every
/// other value, including the string `"0"` and empty objects, is truthy.
pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(boolean) => *boolean,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(_) => true,
    }
}

/// Evaluates every rule of a slice against the same scope.
fn apply_all(rules: &[Rule], data: &Value) -> Result<Vec<Value>, EvaluationError> {
    rules.iter().map(|rule| apply(rule, data)).collect()
}

/// Builds the temporary error for a custom operation pending implementation.
fn unsupported(operator: &'static str) -> EvaluationError {
    EvaluationError::UnsupportedOperation { operator }
}

/// Resolves a `var` rule: empty paths yield the whole scope, and absent or
/// `null` values fall back to the default.
fn eval_var(path: &str, default: Option<&Literal>, data: &Value) -> Value {
    if path.is_empty() {
        return data.clone();
    }
    match lookup(path, data) {
        Some(Value::Null) | None => default.map_or(Value::Null, literal_value),
        Some(value) => value,
    }
}

/// Resolves the key list of `missing`, then splits it into the resolved
/// keys and the keys whose value is absent, `null` or the empty string.
fn eval_missing(keys: &[Rule], data: &Value) -> Result<(Vec<Value>, Vec<Value>), EvaluationError> {
    let evaluated = apply_all(keys, data)?;
    let resolved = match evaluated.first() {
        Some(Value::Array(items)) => items.clone(),
        _ => evaluated,
    };
    let absent = resolved
        .iter()
        .map(to_string)
        .filter(|name| match lookup(name, data) {
            None | Some(Value::Null) => true,
            Some(Value::String(text)) => text.is_empty(),
            Some(_) => false,
        })
        .map(Value::String)
        .collect();
    Ok((resolved, absent))
}

/// Evaluates `missing_some`: enough present keys yield an empty array,
/// otherwise the missing keys are returned.
fn eval_missing_some(min: u64, keys: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let (resolved, absent) = eval_missing(keys, data)?;
    let present = resolved.len().saturating_sub(absent.len());
    if u64::try_from(present).unwrap_or(u64::MAX) >= min {
        Ok(Value::Array(Vec::new()))
    } else {
        Ok(Value::Array(absent))
    }
}

/// Evaluates `if` branches as condition and outcome pairs followed by an
/// optional else; exhausted branches yield `null`.
fn eval_if(branches: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let mut pairs = branches.chunks_exact(2);
    for pair in pairs.by_ref() {
        if truthy(&apply(&pair[0], data)?) {
            return apply(&pair[1], data);
        }
    }
    match pairs.remainder() {
        [fallback] => apply(fallback, data),
        _ => Ok(Value::Null),
    }
}

/// Evaluates `and`: the first falsy operand wins, otherwise the last one.
fn eval_and(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let mut last = Value::Null;
    for operand in operands {
        last = apply(operand, data)?;
        if !truthy(&last) {
            return Ok(last);
        }
    }
    Ok(last)
}

/// Evaluates `or`: the first truthy operand wins, otherwise the last one.
fn eval_or(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let mut last = Value::Null;
    for operand in operands {
        last = apply(operand, data)?;
        if truthy(&last) {
            return Ok(last);
        }
    }
    Ok(last)
}

/// Evaluates a comparison chain: the binary form compares two operands,
/// the ternary form tests betweenness.
fn eval_chain(
    operands: &[Rule],
    data: &Value,
    ordered: fn(&Value, &Value) -> bool,
) -> Result<Value, EvaluationError> {
    let Some((first, rest)) = operands.split_first() else {
        return Ok(Value::Bool(false));
    };
    let mut previous = apply(first, data)?;
    for operand in rest {
        let next = apply(operand, data)?;
        if !ordered(&previous, &next) {
            return Ok(Value::Bool(false));
        }
        previous = next;
    }
    Ok(Value::Bool(true))
}

/// Evaluates `+`: variadic addition with a zero seed, so the unary form
/// casts its operand to a number.
fn eval_add(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let mut sum = 0.0;
    for operand in operands {
        sum += parse_float(&apply(operand, data)?);
    }
    Ok(number_value(sum))
}

/// Evaluates `-`: binary subtraction, or arithmetic negation when unary.
fn eval_sub(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    match operands {
        [operand] => Ok(number_value(-to_number(&apply(operand, data)?))),
        [left, right] => Ok(number_value(
            to_number(&apply(left, data)?) - to_number(&apply(right, data)?),
        )),
        _ => Ok(Value::Null),
    }
}

/// Evaluates `*`: variadic multiplication.
fn eval_mul(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let mut product = 1.0;
    for operand in operands {
        product *= parse_float(&apply(operand, data)?);
    }
    Ok(number_value(product))
}

/// Evaluates `min` or `max`; any non numeric operand poisons the result.
fn eval_extreme(
    operands: &[Rule],
    data: &Value,
    pick: fn(f64, f64) -> f64,
) -> Result<Value, EvaluationError> {
    let mut extreme: Option<f64> = None;
    for operand in operands {
        let number = to_number(&apply(operand, data)?);
        if number.is_nan() {
            return Ok(Value::Null);
        }
        extreme = Some(extreme.map_or(number, |current| pick(current, number)));
    }
    Ok(extreme.map_or(Value::Null, number_value))
}

/// Evaluates `cat`: concatenates the string form of every operand.
fn eval_cat(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let mut text = String::new();
    for operand in operands {
        text.push_str(&to_string(&apply(operand, data)?));
    }
    Ok(Value::String(text))
}

/// Evaluates `substr` with the JavaScript negative index semantics: a
/// negative position counts back from the end, and a negative length stops
/// that many characters before the end. Counts characters, not UTF-16 code
/// units.
fn eval_substr(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let Some((subject, indexes)) = operands.split_first() else {
        return Ok(Value::Null);
    };
    let chars: Vec<char> = to_string(&apply(subject, data)?).chars().collect();
    let length = i64::try_from(chars.len()).unwrap_or(i64::MAX);
    let start = match indexes.first() {
        Some(operand) => to_integer(&apply(operand, data)?),
        None => 0,
    };
    let begin = if start < 0 {
        (length + start).max(0)
    } else {
        start.min(length)
    };
    let end = match indexes.get(1) {
        Some(operand) => {
            let span = to_integer(&apply(operand, data)?);
            if span < 0 {
                length + span
            } else {
                begin + span
            }
        }
        None => length,
    }
    .clamp(begin, length);
    let begin = usize::try_from(begin).unwrap_or(0);
    let end = usize::try_from(end).unwrap_or(0);
    Ok(Value::String(chars[begin..end].iter().collect()))
}

/// Evaluates `in`: substring search when the haystack is a string, strict
/// membership when it is an array.
fn eval_in(needle: &Rule, haystack: &Rule, data: &Value) -> Result<Value, EvaluationError> {
    let needle = apply(needle, data)?;
    let found = match apply(haystack, data)? {
        Value::String(text) => text.contains(&to_string(&needle)),
        Value::Array(items) => items.iter().any(|item| strict_eq(item, &needle)),
        _ => false,
    };
    Ok(Value::Bool(found))
}

/// Evaluates `merge`: flattens array operands and wraps scalar operands.
fn eval_merge(operands: &[Rule], data: &Value) -> Result<Value, EvaluationError> {
    let mut merged = Vec::new();
    for operand in operands {
        match apply(operand, data)? {
            Value::Array(items) => merged.extend(items),
            scalar => merged.push(scalar),
        }
    }
    Ok(Value::Array(merged))
}

/// Evaluates the array operand of an iteration operator; non arrays
/// iterate as empty.
fn iteration_items(array: &Rule, data: &Value) -> Result<Vec<Value>, EvaluationError> {
    match apply(array, data)? {
        Value::Array(items) => Ok(items),
        _ => Ok(Vec::new()),
    }
}

/// Evaluates `map`: applies the logic to every element, rebinding the
/// scope to the element.
fn eval_map(array: &Rule, logic: &Rule, data: &Value) -> Result<Value, EvaluationError> {
    let mapped = iteration_items(array, data)?
        .iter()
        .map(|item| apply(logic, item))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Array(mapped))
}

/// Evaluates `filter`: keeps the elements with truthy outcomes.
fn eval_filter(array: &Rule, logic: &Rule, data: &Value) -> Result<Value, EvaluationError> {
    let mut kept = Vec::new();
    for item in iteration_items(array, data)? {
        if truthy(&apply(logic, &item)?) {
            kept.push(item);
        }
    }
    Ok(Value::Array(kept))
}

/// Evaluates `reduce`: folds the elements with a scope exposing `current`
/// and `accumulator`.
fn eval_reduce(
    array: &Rule,
    logic: &Rule,
    initial: &Rule,
    data: &Value,
) -> Result<Value, EvaluationError> {
    let mut accumulator = apply(initial, data)?;
    for item in iteration_items(array, data)? {
        let scope = json!({ "current": item, "accumulator": accumulator });
        accumulator = apply(logic, &scope)?;
    }
    Ok(accumulator)
}

/// Evaluates `all`: every element satisfies the test, and empty arrays do
/// not.
fn eval_all(array: &Rule, test: &Rule, data: &Value) -> Result<Value, EvaluationError> {
    let items = iteration_items(array, data)?;
    if items.is_empty() {
        return Ok(Value::Bool(false));
    }
    for item in items {
        if !truthy(&apply(test, &item)?) {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(true))
}

/// Evaluates `some`: at least one element satisfies the test.
fn eval_some(array: &Rule, test: &Rule, data: &Value) -> Result<Value, EvaluationError> {
    for item in iteration_items(array, data)? {
        if truthy(&apply(test, &item)?) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

/// Compares two values with JavaScript style loose equality.
///
/// Implements the abstract equality algorithm over JSON types: `null` is
/// equal only to `null`, booleans coerce to numbers, numbers and strings
/// compare through numeric coercion, and composites coerce to their string
/// form before comparing against primitives. Two composites are never
/// equal, mirroring the reference comparison of object identities.
fn loose_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Bool(boolean), other) | (other, Value::Bool(boolean)) => {
            loose_eq(&number_value(f64::from(u8::from(*boolean))), other)
        }
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Number(_), Value::Number(_) | Value::String(_))
        | (Value::String(_), Value::Number(_)) => nums_eq(to_number(left), to_number(right)),
        (
            composite @ (Value::Array(_) | Value::Object(_)),
            primitive @ (Value::String(_) | Value::Number(_)),
        )
        | (
            primitive @ (Value::String(_) | Value::Number(_)),
            composite @ (Value::Array(_) | Value::Object(_)),
        ) => loose_eq(&Value::String(to_string(composite)), primitive),
        _ => false,
    }
}

/// Compares two values with strict equality: equal types and equal values.
/// Composites are never strictly equal, mirroring the reference comparison
/// of object identities.
fn strict_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Number(_), Value::Number(_)) => nums_eq(to_number(left), to_number(right)),
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
    }
}

/// Compares two numbers exactly; `NaN` never equals anything.
fn nums_eq(left: f64, right: f64) -> bool {
    left == right
}

/// Orders two values strictly: lexicographically when both are strings,
/// numerically otherwise, with non numeric operands comparing false.
fn lt(left: &Value, right: &Value) -> bool {
    if let (Value::String(a), Value::String(b)) = (left, right) {
        return a < b;
    }
    to_number(left) < to_number(right)
}

/// Orders two values inclusively, with the same coercion as [`lt`].
fn lte(left: &Value, right: &Value) -> bool {
    if let (Value::String(a), Value::String(b)) = (left, right) {
        return a <= b;
    }
    to_number(left) <= to_number(right)
}

/// Coerces a value to a number like the JavaScript `Number` conversion:
/// `null` and empty strings yield zero, and inconvertible values yield
/// `NaN`.
fn to_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(boolean) => f64::from(u8::from(*boolean)),
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                0.0
            } else {
                trimmed.parse().unwrap_or(f64::NAN)
            }
        }
        Value::Array(items) => match items.as_slice() {
            [] => 0.0,
            [single] => to_number(single),
            _ => f64::NAN,
        },
        Value::Object(_) => f64::NAN,
    }
}

/// Coerces a value to a number like the JavaScript `parseFloat` function:
/// the longest numeric prefix of the string form is parsed, and the empty
/// prefix yields `NaN`. The additive operators use this conversion.
fn parse_float(value: &Value) -> f64 {
    numeric_prefix(to_string(value).trim_start())
        .parse()
        .unwrap_or(f64::NAN)
}

/// Extracts the longest leading slice parseable as a decimal number,
/// covering an optional sign, an optional fraction and an optional
/// exponent.
fn numeric_prefix(text: &str) -> &str {
    let bytes = text.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut digits = 0;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
        digits += 1;
    }
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return "";
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mantissa_end = end;
        let mut exponent = end + 1 + usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let mut exponent_digits = 0;
        while bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            exponent += 1;
            exponent_digits += 1;
        }
        end = if exponent_digits > 0 {
            exponent
        } else {
            mantissa_end
        };
    }
    &text[..end]
}

/// Coerces a value to an integer for index arithmetic: the numeric form is
/// truncated and `NaN` yields zero.
#[expect(
    clippy::cast_possible_truncation,
    reason = "truncation is the JavaScript ToInteger semantics"
)]
fn to_integer(value: &Value) -> i64 {
    let number = to_number(value);
    if number.is_nan() {
        0
    } else {
        number.trunc() as i64
    }
}

/// Coerces a value to its string form, mirroring the JavaScript `String`
/// conversion: integral numbers print without a fraction, array elements
/// join on commas with `null` printing empty, and objects print as
/// `[object Object]`.
fn to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Number(number) => format_number(number.as_f64().unwrap_or(f64::NAN)),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    to_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// Formats a number like JavaScript: integral values print without a
/// fraction and negative zero prints as zero.
fn format_number(number: f64) -> String {
    if number == 0.0 {
        "0".to_owned()
    } else if number.fract() == 0.0 && number.abs() < 1e21 {
        format!("{number:.0}")
    } else {
        number.to_string()
    }
}

/// Materializes a float as a JSON number; non finite values become `null`
/// because JSON cannot represent them.
fn number_value(number: f64) -> Value {
    serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number)
}

/// Converts an AST literal to its JSON value.
fn literal_value(literal: &Literal) -> Value {
    match literal {
        Literal::Null => Value::Null,
        Literal::Bool(boolean) => Value::Bool(*boolean),
        Literal::Number(number) => number_value(*number),
        Literal::String(text) => Value::String(text.clone()),
    }
}

/// Resolves a dotted path against the data scope, descending objects by
/// key and arrays by numeric index.
fn lookup(path: &str, data: &Value) -> Option<Value> {
    let mut current = data;
    for segment in path.split('.') {
        current = match current {
            Value::Object(entries) => entries.get(segment)?,
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current.clone())
}
