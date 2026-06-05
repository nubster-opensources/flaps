//! Conversion from raw JSON values to the typed flag set model.
//!
//! All functions thread the JSON path of the element under inspection so
//! that every error pinpoints the offending location in the source document.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::ParseError;
use crate::model::{Flag, FlagSet, Metadata, MetadataValue, State, Variants};
use crate::targeting::{Bucket, Literal, Rule, SemVerOp};

type RulePair = (Box<Rule>, Box<Rule>);
type RuleTriple = (Box<Rule>, Box<Rule>, Box<Rule>);

pub(crate) fn flag_set(value: &Value) -> Result<FlagSet, ParseError> {
    let Value::Object(root) = value else {
        return Err(invalid("$", "the document root must be an object"));
    };

    let flags_value = root
        .get("flags")
        .ok_or_else(|| invalid("$", "missing required `flags` property"))?;
    let Value::Object(entries) = flags_value else {
        return Err(invalid("flags", "`flags` must be an object"));
    };

    let mut flags = BTreeMap::new();
    for (key, entry) in entries {
        let path = format!("flags.{key}");
        flags.insert(key.clone(), flag(&path, key, entry)?);
    }

    let metadata = match root.get("metadata") {
        Some(value) => metadata("metadata", value)?,
        None => Metadata::new(),
    };

    Ok(FlagSet { flags, metadata })
}

fn flag(path: &str, key: &str, value: &Value) -> Result<Flag, ParseError> {
    let Value::Object(properties) = value else {
        return Err(invalid(path, "a flag must be an object"));
    };

    for name in properties.keys() {
        if !matches!(
            name.as_str(),
            "state" | "variants" | "defaultVariant" | "targeting" | "metadata"
        ) {
            return Err(invalid(&format!("{path}.{name}"), "unknown flag property"));
        }
    }

    let state = state(&format!("{path}.state"), properties.get("state"))?;
    let variants = variants(&format!("{path}.variants"), key, properties.get("variants"))?;

    let default_variant = match properties.get("defaultVariant") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(name.clone()),
        Some(_) => {
            return Err(invalid(
                &format!("{path}.defaultVariant"),
                "`defaultVariant` must be a string",
            ));
        }
    };

    let targeting = match properties.get("targeting") {
        None => None,
        Some(Value::Object(map)) if map.is_empty() => None,
        Some(value) => Some(rule(&format!("{path}.targeting"), value)?),
    };

    let metadata = match properties.get("metadata") {
        Some(value) => metadata(&format!("{path}.metadata"), value)?,
        None => Metadata::new(),
    };

    Ok(Flag {
        state,
        variants,
        default_variant,
        targeting,
        metadata,
    })
}

fn state(path: &str, value: Option<&Value>) -> Result<State, ParseError> {
    match value {
        Some(Value::String(name)) if name == "ENABLED" => Ok(State::Enabled),
        Some(Value::String(name)) if name == "DISABLED" => Ok(State::Disabled),
        Some(_) => Err(invalid(path, "`state` must be \"ENABLED\" or \"DISABLED\"")),
        None => Err(invalid(path, "missing required `state` property")),
    }
}

fn variants(path: &str, flag_key: &str, value: Option<&Value>) -> Result<Variants, ParseError> {
    let Some(value) = value else {
        return Err(invalid(path, "missing required `variants` property"));
    };
    let Value::Object(entries) = value else {
        return Err(invalid(path, "`variants` must be an object"));
    };

    let mut variants = match entries.values().next() {
        None | Some(Value::Bool(_)) => Variants::Boolean(BTreeMap::new()),
        Some(Value::String(_)) => Variants::String(BTreeMap::new()),
        Some(Value::Number(_)) => Variants::Number(BTreeMap::new()),
        Some(Value::Object(_)) => Variants::Object(BTreeMap::new()),
        Some(_) => {
            return Err(invalid(
                path,
                "variant values must be booleans, strings, numbers or objects",
            ));
        }
    };

    for (name, value) in entries {
        let matched = match (&mut variants, value) {
            (Variants::Boolean(map), Value::Bool(value)) => {
                map.insert(name.clone(), *value).is_none()
            }
            (Variants::String(map), Value::String(value)) => {
                map.insert(name.clone(), value.clone()).is_none()
            }
            (Variants::Number(map), Value::Number(value)) => {
                let Some(value) = value.as_f64() else {
                    return Err(invalid(
                        &format!("{path}.{name}"),
                        "numeric variant does not fit a 64 bit float",
                    ));
                };
                map.insert(name.clone(), value).is_none()
            }
            (Variants::Object(map), Value::Object(value)) => {
                map.insert(name.clone(), value.clone()).is_none()
            }
            _ => {
                return Err(ParseError::MixedVariantTypes {
                    flag_key: flag_key.to_owned(),
                });
            }
        };
        debug_assert!(matched, "JSON objects cannot carry duplicate keys");
    }

    Ok(variants)
}

fn metadata(path: &str, value: &Value) -> Result<Metadata, ParseError> {
    let Value::Object(entries) = value else {
        return Err(invalid(path, "`metadata` must be an object"));
    };

    let mut metadata = Metadata::new();
    for (key, value) in entries {
        let value = match value {
            Value::Bool(value) => MetadataValue::Bool(*value),
            Value::String(value) => MetadataValue::String(value.clone()),
            Value::Number(value) => {
                let Some(value) = value.as_f64() else {
                    return Err(invalid(
                        &format!("{path}.{key}"),
                        "numeric metadata does not fit a 64 bit float",
                    ));
                };
                MetadataValue::Number(value)
            }
            _ => {
                return Err(invalid(
                    &format!("{path}.{key}"),
                    "metadata values must be booleans, strings or numbers",
                ));
            }
        };
        metadata.insert(key.clone(), value);
    }

    Ok(metadata)
}

pub(crate) fn rule(path: &str, value: &Value) -> Result<Rule, ParseError> {
    match value {
        Value::Null => Ok(Rule::Literal(Literal::Null)),
        Value::Bool(value) => Ok(Rule::Literal(Literal::Bool(*value))),
        Value::Number(value) => {
            let Some(value) = value.as_f64() else {
                return Err(invalid(path, "number does not fit a 64 bit float"));
            };
            Ok(Rule::Literal(Literal::Number(value)))
        }
        Value::String(value) => Ok(Rule::Literal(Literal::String(value.clone()))),
        Value::Array(items) => Ok(Rule::Array(rules(path, items)?)),
        Value::Object(map) => operation(path, map),
    }
}

fn rules(path: &str, items: &[Value]) -> Result<Vec<Rule>, ParseError> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| rule(&format!("{path}[{index}]"), item))
        .collect()
}

fn operation(path: &str, map: &serde_json::Map<String, Value>) -> Result<Rule, ParseError> {
    let mut entries = map.iter();
    let (Some((operator, args)), None) = (entries.next(), entries.next()) else {
        return Err(invalid(
            path,
            "an operation must be an object with exactly one key",
        ));
    };

    match operator.as_str() {
        "$ref" | "var" | "missing" | "missing_some" | "if" => structural(path, operator, args),
        "!" | "!!" | "and" | "or" => logic(path, operator, args),
        "==" | "===" | "!=" | "!==" | ">" | ">=" | "<" | "<=" => comparison(path, operator, args),
        "+" | "-" | "*" | "/" | "%" | "min" | "max" => arithmetic(path, operator, args),
        "cat" | "substr" | "in" | "merge" | "map" | "filter" | "reduce" | "all" | "none"
        | "some" => collection(path, operator, args),
        "starts_with" | "ends_with" | "sem_ver" | "fractional" => custom(path, operator, args),
        _ => Err(ParseError::UnknownOperator {
            path: path.to_owned(),
            operator: operator.clone(),
        }),
    }
}

fn structural(path: &str, operator: &str, args: &Value) -> Result<Rule, ParseError> {
    let path = format!("{path}.{operator}");
    match operator {
        "$ref" => match args {
            Value::String(name) => Ok(Rule::Ref(name.clone())),
            _ => Err(bad_args(&path, operator, "expects an evaluator name")),
        },
        "var" => var(&path, args),
        "missing" => Ok(Rule::Missing(rules(&path, op_args(args))?)),
        "missing_some" => missing_some(&path, args),
        "if" => Ok(Rule::If(variadic(&path, operator, args, 2)?)),
        _ => unreachable!("dispatched operators are exhaustive"),
    }
}

fn logic(path: &str, operator: &str, args: &Value) -> Result<Rule, ParseError> {
    let path = format!("{path}.{operator}");
    match operator {
        "!" => Ok(Rule::Not(unary(&path, operator, args)?)),
        "!!" => Ok(Rule::Truthy(unary(&path, operator, args)?)),
        "and" => Ok(Rule::And(variadic(&path, operator, args, 1)?)),
        "or" => Ok(Rule::Or(variadic(&path, operator, args, 1)?)),
        _ => unreachable!("dispatched operators are exhaustive"),
    }
}

fn comparison(path: &str, operator: &str, args: &Value) -> Result<Rule, ParseError> {
    let path = format!("{path}.{operator}");
    match operator {
        "==" => binary(&path, operator, args).map(|(a, b)| Rule::Eq(a, b)),
        "===" => binary(&path, operator, args).map(|(a, b)| Rule::StrictEq(a, b)),
        "!=" => binary(&path, operator, args).map(|(a, b)| Rule::Neq(a, b)),
        "!==" => binary(&path, operator, args).map(|(a, b)| Rule::StrictNeq(a, b)),
        ">" => binary(&path, operator, args).map(|(a, b)| Rule::Gt(a, b)),
        ">=" => binary(&path, operator, args).map(|(a, b)| Rule::Gte(a, b)),
        "<" => Ok(Rule::Lt(bounded(&path, operator, args, 2, 3)?)),
        "<=" => Ok(Rule::Lte(bounded(&path, operator, args, 2, 3)?)),
        _ => unreachable!("dispatched operators are exhaustive"),
    }
}

fn arithmetic(path: &str, operator: &str, args: &Value) -> Result<Rule, ParseError> {
    let path = format!("{path}.{operator}");
    match operator {
        "+" => Ok(Rule::Add(variadic(&path, operator, args, 1)?)),
        "-" => Ok(Rule::Sub(bounded(&path, operator, args, 1, 2)?)),
        "*" => Ok(Rule::Mul(variadic(&path, operator, args, 1)?)),
        "/" => binary(&path, operator, args).map(|(a, b)| Rule::Div(a, b)),
        "%" => binary(&path, operator, args).map(|(a, b)| Rule::Mod(a, b)),
        "min" => Ok(Rule::Min(variadic(&path, operator, args, 1)?)),
        "max" => Ok(Rule::Max(variadic(&path, operator, args, 1)?)),
        _ => unreachable!("dispatched operators are exhaustive"),
    }
}

fn collection(path: &str, operator: &str, args: &Value) -> Result<Rule, ParseError> {
    let path = format!("{path}.{operator}");
    match operator {
        "cat" => Ok(Rule::Cat(variadic(&path, operator, args, 1)?)),
        "substr" => Ok(Rule::Substr(bounded(&path, operator, args, 2, 3)?)),
        "in" => binary(&path, operator, args).map(|(a, b)| Rule::In(a, b)),
        "merge" => Ok(Rule::Merge(variadic(&path, operator, args, 1)?)),
        "map" => binary(&path, operator, args).map(|(a, b)| Rule::Map(a, b)),
        "filter" => binary(&path, operator, args).map(|(a, b)| Rule::Filter(a, b)),
        "reduce" => ternary(&path, operator, args).map(|(a, b, c)| Rule::Reduce(a, b, c)),
        "all" => binary(&path, operator, args).map(|(a, b)| Rule::All(a, b)),
        "none" => binary(&path, operator, args).map(|(a, b)| Rule::None(a, b)),
        "some" => binary(&path, operator, args).map(|(a, b)| Rule::Some(a, b)),
        _ => unreachable!("dispatched operators are exhaustive"),
    }
}

fn custom(path: &str, operator: &str, args: &Value) -> Result<Rule, ParseError> {
    let path = format!("{path}.{operator}");
    match operator {
        "starts_with" => binary(&path, operator, args).map(|(a, b)| Rule::StartsWith(a, b)),
        "ends_with" => binary(&path, operator, args).map(|(a, b)| Rule::EndsWith(a, b)),
        "sem_ver" => sem_ver(&path, args),
        "fractional" => fractional(&path, args),
        _ => unreachable!("dispatched operators are exhaustive"),
    }
}

fn var(path: &str, args: &Value) -> Result<Rule, ParseError> {
    const EXPECTS: &str = "expects a string path and an optional literal default";
    match args {
        Value::String(attribute) => Ok(Rule::Var {
            path: attribute.clone(),
            default: None,
        }),
        Value::Array(items) => match items.as_slice() {
            [Value::String(attribute)] => Ok(Rule::Var {
                path: attribute.clone(),
                default: None,
            }),
            [Value::String(attribute), default] => Ok(Rule::Var {
                path: attribute.clone(),
                default: Some(literal(&format!("{path}[1]"), default)?),
            }),
            _ => Err(bad_args(path, "var", EXPECTS)),
        },
        _ => Err(bad_args(path, "var", EXPECTS)),
    }
}

fn missing_some(path: &str, args: &Value) -> Result<Rule, ParseError> {
    const EXPECTS: &str = "expects a minimum count and an array of keys";
    let [min, keys] = op_args(args) else {
        return Err(bad_args(path, "missing_some", EXPECTS));
    };
    let Some(min) = min.as_u64() else {
        return Err(bad_args(path, "missing_some", EXPECTS));
    };
    let Value::Array(keys) = keys else {
        return Err(bad_args(path, "missing_some", EXPECTS));
    };
    Ok(Rule::MissingSome {
        min,
        keys: rules(&format!("{path}[1]"), keys)?,
    })
}

fn sem_ver(path: &str, args: &Value) -> Result<Rule, ParseError> {
    let [value, operator, version] = op_args(args) else {
        return Err(bad_args(
            path,
            "sem_ver",
            "expects a value, a comparison operator and a version",
        ));
    };
    let op = match operator {
        Value::String(symbol) => match symbol.as_str() {
            "=" => SemVerOp::Eq,
            "!=" => SemVerOp::Neq,
            "<" => SemVerOp::Lt,
            "<=" => SemVerOp::Lte,
            ">" => SemVerOp::Gt,
            ">=" => SemVerOp::Gte,
            "^" => SemVerOp::CaretMatch,
            "~" => SemVerOp::TildeMatch,
            _ => {
                return Err(bad_args(
                    path,
                    "sem_ver",
                    "the comparison operator must be one of =, !=, <, <=, >, >=, ^ or ~",
                ));
            }
        },
        _ => {
            return Err(bad_args(
                path,
                "sem_ver",
                "the comparison operator must be a string",
            ));
        }
    };
    Ok(Rule::SemVer {
        value: Box::new(rule(&format!("{path}[0]"), value)?),
        op,
        version: Box::new(rule(&format!("{path}[2]"), version)?),
    })
}

fn fractional(path: &str, args: &Value) -> Result<Rule, ParseError> {
    let Value::Array(items) = args else {
        return Err(bad_args(path, "fractional", "expects an array"));
    };
    let (bucket_by, bucket_items) = match items.split_first() {
        None => {
            return Err(bad_args(path, "fractional", "expects at least one bucket"));
        }
        Some((Value::Array(_), _)) => (None, items.as_slice()),
        Some((expression, rest)) => (
            Some(Box::new(rule(&format!("{path}[0]"), expression)?)),
            rest,
        ),
    };
    if bucket_items.is_empty() {
        return Err(bad_args(path, "fractional", "expects at least one bucket"));
    }

    let mut buckets = Vec::with_capacity(bucket_items.len());
    for (index, item) in bucket_items.iter().enumerate() {
        buckets.push(bucket(&format!("{path}[{index}]"), item)?);
    }
    Ok(Rule::Fractional { bucket_by, buckets })
}

fn bucket(path: &str, value: &Value) -> Result<Bucket, ParseError> {
    const EXPECTS: &str = "a bucket pairs a variant name with an optional integer weight";
    let Value::Array(pair) = value else {
        return Err(bad_args(path, "fractional", EXPECTS));
    };
    match pair.as_slice() {
        [Value::String(variant)] => Ok(Bucket {
            variant: variant.clone(),
            weight: 1,
        }),
        [Value::String(variant), weight] => {
            let weight = weight
                .as_u64()
                .and_then(|weight| u32::try_from(weight).ok())
                .ok_or_else(|| bad_args(path, "fractional", EXPECTS))?;
            Ok(Bucket {
                variant: variant.clone(),
                weight,
            })
        }
        _ => Err(bad_args(path, "fractional", EXPECTS)),
    }
}

fn literal(path: &str, value: &Value) -> Result<Literal, ParseError> {
    match value {
        Value::Null => Ok(Literal::Null),
        Value::Bool(value) => Ok(Literal::Bool(*value)),
        Value::Number(value) => value
            .as_f64()
            .map(Literal::Number)
            .ok_or_else(|| invalid(path, "number does not fit a 64 bit float")),
        Value::String(value) => Ok(Literal::String(value.clone())),
        _ => Err(invalid(path, "expected a literal value")),
    }
}

fn op_args(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        single => std::slice::from_ref(single),
    }
}

fn unary(path: &str, operator: &str, args: &Value) -> Result<Box<Rule>, ParseError> {
    let [argument] = op_args(args) else {
        return Err(bad_args(path, operator, "expects exactly one argument"));
    };
    Ok(Box::new(rule(&format!("{path}[0]"), argument)?))
}

fn binary(path: &str, operator: &str, args: &Value) -> Result<RulePair, ParseError> {
    let [first, second] = op_args(args) else {
        return Err(bad_args(path, operator, "expects exactly two arguments"));
    };
    Ok((
        Box::new(rule(&format!("{path}[0]"), first)?),
        Box::new(rule(&format!("{path}[1]"), second)?),
    ))
}

fn ternary(path: &str, operator: &str, args: &Value) -> Result<RuleTriple, ParseError> {
    let [first, second, third] = op_args(args) else {
        return Err(bad_args(path, operator, "expects exactly three arguments"));
    };
    Ok((
        Box::new(rule(&format!("{path}[0]"), first)?),
        Box::new(rule(&format!("{path}[1]"), second)?),
        Box::new(rule(&format!("{path}[2]"), third)?),
    ))
}

fn variadic(path: &str, operator: &str, args: &Value, min: usize) -> Result<Vec<Rule>, ParseError> {
    let items = op_args(args);
    if items.len() < min {
        return Err(bad_args(
            path,
            operator,
            &format!("expects at least {min} argument(s)"),
        ));
    }
    rules(path, items)
}

fn bounded(
    path: &str,
    operator: &str,
    args: &Value,
    min: usize,
    max: usize,
) -> Result<Vec<Rule>, ParseError> {
    let items = op_args(args);
    if items.len() < min || items.len() > max {
        return Err(bad_args(
            path,
            operator,
            &format!("expects between {min} and {max} arguments"),
        ));
    }
    rules(path, items)
}

fn bad_args(path: &str, operator: &str, reason: &str) -> ParseError {
    ParseError::InvalidArguments {
        path: path.to_owned(),
        operator: operator.to_owned(),
        reason: reason.to_owned(),
    }
}

fn invalid(path: &str, reason: &str) -> ParseError {
    ParseError::InvalidDocument {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}
