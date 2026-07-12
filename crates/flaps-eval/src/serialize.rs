//! Canonical serialization of the typed model back to flagd JSON.
//!
//! The output is canonical rather than byte-identical to the source:
//! `$evaluators` are inlined, keys are ordered, unary operators use the
//! array form and fractional weights are always explicit.

use serde_json::{Map, Value};

use crate::model::{Flag, FlagSet, Metadata, MetadataValue, State, Variants};
use crate::targeting::{Literal, Rule, SemVerOp};

pub(crate) fn flag_set_value(set: &FlagSet) -> Value {
    let mut root = Map::new();
    let mut flags = Map::new();
    for (key, flag) in &set.flags {
        flags.insert(key.clone(), flag_value(flag));
    }
    root.insert("flags".to_owned(), Value::Object(flags));
    if !set.metadata.is_empty() {
        root.insert("metadata".to_owned(), metadata_value(&set.metadata));
    }
    Value::Object(root)
}

fn flag_value(flag: &Flag) -> Value {
    let mut map = Map::new();
    let state = match flag.state {
        State::Enabled => "ENABLED",
        State::Disabled => "DISABLED",
    };
    map.insert("state".to_owned(), Value::String(state.to_owned()));
    map.insert("variants".to_owned(), variants_value(&flag.variants));
    if let Some(default_variant) = &flag.default_variant {
        map.insert(
            "defaultVariant".to_owned(),
            Value::String(default_variant.clone()),
        );
    }
    if let Some(targeting) = &flag.targeting {
        map.insert("targeting".to_owned(), rule_value(targeting));
    }
    if !flag.metadata.is_empty() {
        map.insert("metadata".to_owned(), metadata_value(&flag.metadata));
    }
    Value::Object(map)
}

fn variants_value(variants: &Variants) -> Value {
    let mut map = Map::new();
    match variants {
        Variants::Boolean(entries) => {
            for (name, value) in entries {
                map.insert(name.clone(), Value::Bool(*value));
            }
        }
        Variants::String(entries) => {
            for (name, value) in entries {
                map.insert(name.clone(), Value::String(value.clone()));
            }
        }
        Variants::Number(entries) => {
            for (name, value) in entries {
                map.insert(name.clone(), number_value(*value));
            }
        }
        Variants::Object(entries) => {
            for (name, value) in entries {
                map.insert(name.clone(), Value::Object(value.clone()));
            }
        }
    }
    Value::Object(map)
}

/// Converts flag or flag-set [`Metadata`] to its JSON representation.
///
/// This is the single source of truth for metadata-to-JSON conversion: any
/// caller that needs to expose `Metadata` as `serde_json::Value` (for example
/// the OFREP response DTOs in `flaps-server`) must reuse this function rather
/// than re-implementing the scalar mapping, so number formatting stays
/// consistent with the canonical ruleset serialization.
#[must_use]
pub fn metadata_to_json(metadata: &Metadata) -> Value {
    metadata_value(metadata)
}

fn metadata_value(metadata: &Metadata) -> Value {
    let mut map = Map::new();
    for (key, value) in metadata {
        let value = match value {
            MetadataValue::Bool(value) => Value::Bool(*value),
            MetadataValue::String(value) => Value::String(value.clone()),
            MetadataValue::Number(value) => number_value(*value),
        };
        map.insert(key.clone(), value);
    }
    Value::Object(map)
}

pub(crate) fn rule_value(rule: &Rule) -> Value {
    match rule {
        Rule::Literal(literal) => literal_value(literal),
        Rule::Array(items) => Value::Array(items.iter().map(rule_value).collect()),
        Rule::Var { path, default } => match default {
            None => op_scalar("var", Value::String(path.clone())),
            Some(default) => op_value(
                "var",
                vec![Value::String(path.clone()), literal_value(default)],
            ),
        },
        Rule::Missing(keys) => op("missing", keys),
        Rule::MissingSome { min, keys } => op_value(
            "missing_some",
            vec![
                Value::from(*min),
                Value::Array(keys.iter().map(rule_value).collect()),
            ],
        ),
        Rule::If(args) => op("if", args),
        Rule::And(args) => op("and", args),
        Rule::Or(args) => op("or", args),
        Rule::Not(arg) => op1("!", arg),
        Rule::Truthy(arg) => op1("!!", arg),
        Rule::Eq(a, b) => op2("==", a, b),
        Rule::StrictEq(a, b) => op2("===", a, b),
        Rule::Neq(a, b) => op2("!=", a, b),
        Rule::StrictNeq(a, b) => op2("!==", a, b),
        Rule::Gt(a, b) => op2(">", a, b),
        Rule::Gte(a, b) => op2(">=", a, b),
        Rule::Lt(args) => op("<", args),
        Rule::Lte(args) => op("<=", args),
        Rule::Add(args) => op("+", args),
        Rule::Sub(args) => op("-", args),
        Rule::Mul(args) => op("*", args),
        Rule::Div(a, b) => op2("/", a, b),
        Rule::Mod(a, b) => op2("%", a, b),
        Rule::Min(args) => op("min", args),
        Rule::Max(args) => op("max", args),
        Rule::Cat(args) => op("cat", args),
        Rule::Substr(args) => op("substr", args),
        Rule::In(a, b) => op2("in", a, b),
        Rule::Merge(args) => op("merge", args),
        Rule::Map(a, b) => op2("map", a, b),
        Rule::Filter(a, b) => op2("filter", a, b),
        Rule::Reduce(a, b, c) => {
            op_value("reduce", vec![rule_value(a), rule_value(b), rule_value(c)])
        }
        Rule::All(a, b) => op2("all", a, b),
        Rule::None(a, b) => op2("none", a, b),
        Rule::Some(a, b) => op2("some", a, b),
        Rule::StartsWith(a, b) => op2("starts_with", a, b),
        Rule::EndsWith(a, b) => op2("ends_with", a, b),
        Rule::SemVer { value, op, version } => op_value(
            "sem_ver",
            vec![
                rule_value(value),
                Value::String(sem_ver_symbol(*op).to_owned()),
                rule_value(version),
            ],
        ),
        Rule::Fractional { bucket_by, buckets } => {
            let mut args = Vec::with_capacity(buckets.len() + 1);
            if let Some(expression) = bucket_by {
                args.push(rule_value(expression));
            }
            for bucket in buckets {
                args.push(Value::Array(vec![
                    Value::String(bucket.variant.clone()),
                    Value::from(bucket.weight),
                ]));
            }
            op_value("fractional", args)
        }
        Rule::Ref(name) => op_scalar("$ref", Value::String(name.clone())),
    }
}

fn literal_value(literal: &Literal) -> Value {
    match literal {
        Literal::Null => Value::Null,
        Literal::Bool(value) => Value::Bool(*value),
        Literal::Number(value) => number_value(*value),
        Literal::String(value) => Value::String(value.clone()),
    }
}

fn sem_ver_symbol(op: SemVerOp) -> &'static str {
    match op {
        SemVerOp::Eq => "=",
        SemVerOp::Neq => "!=",
        SemVerOp::Lt => "<",
        SemVerOp::Lte => "<=",
        SemVerOp::Gt => ">",
        SemVerOp::Gte => ">=",
        SemVerOp::CaretMatch => "^",
        SemVerOp::TildeMatch => "~",
    }
}

/// Converts a finite `f64` to JSON.
///
/// Whole numbers that round-trip exactly through `i64` are emitted as plain
/// JSON integers (`3`, not `3.0`), so metadata and variant values never carry
/// a spurious fractional suffix. Fractional values, values outside `i64`
/// range, and non-finite values fall back to the standard float
/// representation, with NaN/infinite mapping to JSON `null`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::float_cmp
)]
fn number_value(value: f64) -> Value {
    if value.is_finite() {
        let truncated = value as i64;
        if truncated as f64 == value {
            return Value::from(truncated);
        }
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn op(name: &str, args: &[Rule]) -> Value {
    op_value(name, args.iter().map(rule_value).collect())
}

fn op1(name: &str, arg: &Rule) -> Value {
    op_value(name, vec![rule_value(arg)])
}

fn op2(name: &str, first: &Rule, second: &Rule) -> Value {
    op_value(name, vec![rule_value(first), rule_value(second)])
}

fn op_value(name: &str, args: Vec<Value>) -> Value {
    op_scalar(name, Value::Array(args))
}

fn op_scalar(name: &str, value: Value) -> Value {
    let mut map = Map::new();
    map.insert(name.to_owned(), value);
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_to_json_integer_number_has_no_fractional_suffix() {
        let mut metadata = Metadata::new();
        metadata.insert("priority".to_owned(), MetadataValue::Number(3.0));
        let value = metadata_to_json(&metadata);
        assert_eq!(value["priority"].to_string(), "3", "got: {value}");
    }

    #[test]
    fn metadata_to_json_fractional_number_keeps_decimals() {
        let mut metadata = Metadata::new();
        metadata.insert("ratio".to_owned(), MetadataValue::Number(1.5));
        let value = metadata_to_json(&metadata);
        assert_eq!(value["ratio"].to_string(), "1.5", "got: {value}");
    }

    #[test]
    fn metadata_to_json_bool_and_string_are_bare_scalars() {
        let mut metadata = Metadata::new();
        metadata.insert("enabled".to_owned(), MetadataValue::Bool(true));
        metadata.insert("owner".to_owned(), MetadataValue::String("team-a".into()));
        let value = metadata_to_json(&metadata);
        assert_eq!(value["enabled"], Value::Bool(true));
        assert_eq!(value["owner"], Value::String("team-a".into()));
    }
}
