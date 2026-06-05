//! Conversion from raw JSON values to the typed flag set model.
//!
//! All functions thread the JSON path of the element under inspection so
//! that every error pinpoints the offending location in the source document.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::ParseError;
use crate::model::{Flag, FlagSet, Metadata, MetadataValue, State, Variants};
use crate::targeting::Rule;

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
    let _ = (path, value);
    todo!()
}

fn invalid(path: &str, reason: &str) -> ParseError {
    ParseError::InvalidDocument {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}
