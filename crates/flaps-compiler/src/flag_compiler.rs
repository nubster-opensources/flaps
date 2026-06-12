//! Translates a single [`FlagConfig`] into a `flaps-eval` [`Flag`].

use std::collections::BTreeMap;

use flaps_domain::{
    flag_env_config::{FlagEnvConfig, ServeTarget},
    key::{FlagKey, SegmentKey},
    variant::{ValueType, Variants as DomainVariants},
};
use flaps_eval::{Bucket, Flag, Literal, Metadata, Rule, State, Variants};

use crate::{error::CompileError, input::Segments, segment_compiler::compile_segment_match};

/// Serialized form of a single variant entry as produced by `serde_json`.
///
/// `VariantValue` serializes as `{"bool": v}`, `{"string": v}`, etc.
/// because of `#[serde(rename_all = "snake_case")]` on the enum.
#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SerializedVariantValue {
    Bool(bool),
    String(String),
    Number(f64),
    Json(serde_json::Value),
}

/// Serialized form of `DomainVariants` as produced by `serde_json::to_value`.
///
/// Uses `BTreeMap` for stable key ordering, which is critical for determinism (AC#3).
#[derive(serde::Deserialize)]
struct SerializedVariants {
    /// Declared value type (used only for type checking; the compiler reads
    /// `DomainVariants::value_type()` directly).
    #[allow(dead_code)]
    value_type: flaps_domain::variant::ValueType,
    /// Variant entries in alphabetical key order (`BTreeMap` guarantees this).
    entries: BTreeMap<String, SerializedVariantValue>,
}

/// Extracts variant entries from a [`DomainVariants`] via its JSON representation.
///
/// `DomainVariants` stores entries in a private `HashMap`; the only stable,
/// public way to iterate them without modifying the domain crate is via
/// `serde_json`. The `BTreeMap` target type in [`SerializedVariants`] ensures
/// keys are sorted regardless of `HashMap` iteration order, satisfying AC#3.
fn extract_entries(
    flag: &str,
    domain: &DomainVariants,
) -> Result<SerializedVariants, CompileError> {
    let raw = serde_json::to_value(domain).map_err(|e| CompileError::EvaluatorRejected {
        environment: String::new(),
        reason: format!("variant serialization failed for flag `{flag}`: {e}"),
    })?;
    serde_json::from_value(raw).map_err(|e| CompileError::EvaluatorRejected {
        environment: String::new(),
        reason: format!("variant deserialization failed for flag `{flag}`: {e}"),
    })
}

/// Validates that a [`ServeTarget`] only names variants declared in the flag.
fn validate_serve_target(
    flag: &str,
    serve: &ServeTarget,
    domain: &DomainVariants,
) -> Result<(), CompileError> {
    match serve {
        ServeTarget::Fixed(vk) => {
            if !domain.contains(vk) {
                return Err(CompileError::UnknownVariant {
                    flag: flag.to_owned(),
                    variant: vk.as_str().to_owned(),
                });
            }
        }
        ServeTarget::Rollout(rollout) => {
            for wv in rollout.weights() {
                if !domain.contains(&wv.variant) {
                    return Err(CompileError::UnknownVariant {
                        flag: flag.to_owned(),
                        variant: wv.variant.as_str().to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Compiles domain [`DomainVariants`] into `flaps-eval` [`Variants`].
///
/// The resulting map is always a `BTreeMap`, guaranteeing deterministic
/// serialization order regardless of the underlying `HashMap` in the domain.
fn compile_variants(flag: &str, domain: &DomainVariants) -> Result<Variants, CompileError> {
    let extracted = extract_entries(flag, domain)?;

    match domain.value_type() {
        ValueType::Boolean => {
            let map = extracted
                .entries
                .into_iter()
                .map(|(k, v)| {
                    let SerializedVariantValue::Bool(b) = v else {
                        unreachable!("domain guarantees type homogeneity")
                    };
                    (k, b)
                })
                .collect();
            Ok(Variants::Boolean(map))
        }
        ValueType::String => {
            let map = extracted
                .entries
                .into_iter()
                .map(|(k, v)| {
                    let SerializedVariantValue::String(s) = v else {
                        unreachable!("domain guarantees type homogeneity")
                    };
                    (k, s)
                })
                .collect();
            Ok(Variants::String(map))
        }
        ValueType::Number => {
            let map = extracted
                .entries
                .into_iter()
                .map(|(k, v)| {
                    let SerializedVariantValue::Number(n) = v else {
                        unreachable!("domain guarantees type homogeneity")
                    };
                    (k, n)
                })
                .collect();
            Ok(Variants::Number(map))
        }
        ValueType::Object => {
            let mut map = BTreeMap::new();
            for (k, v) in extracted.entries {
                match v {
                    SerializedVariantValue::Json(serde_json::Value::Object(obj)) => {
                        map.insert(k, obj);
                    }
                    _ => {
                        return Err(CompileError::ObjectVariantNotObject {
                            flag: flag.to_owned(),
                            variant: k,
                        });
                    }
                }
            }
            Ok(Variants::Object(map))
        }
    }
}

/// Compiles the condition for a single targeting rule: AND of inlined segments.
fn compile_condition(
    flag: &str,
    segment_keys: &[SegmentKey],
    segments: &Segments<'_>,
) -> Result<Rule, CompileError> {
    let mut rules: Vec<Rule> = segment_keys
        .iter()
        .map(|sk| {
            let match_expr = segments
                .get(sk)
                .ok_or_else(|| CompileError::UnknownSegment {
                    flag: flag.to_owned(),
                    segment: sk.as_str().to_owned(),
                })?;
            compile_segment_match(match_expr)
        })
        .collect::<Result<_, _>>()?;

    Ok(match rules.len() {
        0 => Rule::Literal(Literal::Bool(true)),
        1 => rules.remove(0),
        _ => Rule::And(rules),
    })
}

/// Compiles a [`ServeTarget`] into a targeting [`Rule`] arm.
fn compile_serve(serve: &ServeTarget) -> Rule {
    match serve {
        ServeTarget::Fixed(vk) => Rule::Literal(Literal::String(vk.as_str().to_owned())),
        ServeTarget::Rollout(rollout) => {
            let buckets = rollout
                .weights()
                .iter()
                .map(|wv| Bucket {
                    variant: wv.variant.as_str().to_owned(),
                    weight: wv.weight,
                })
                .collect();
            Rule::Fractional {
                bucket_by: None,
                buckets,
            }
        }
    }
}

/// Compiles targeting rules and default variant for a flag in one environment.
fn compile_targeting(
    flag: &str,
    config: &FlagEnvConfig,
    segments: &Segments<'_>,
) -> Result<(Option<Rule>, Option<String>), CompileError> {
    // Simple case: no explicit rules and a Fixed default -> skip the targeting tree.
    if config.rules.is_empty() {
        match &config.default_rule {
            ServeTarget::Fixed(vk) => {
                return Ok((None, Some(vk.as_str().to_owned())));
            }
            ServeTarget::Rollout(_) => {
                // No rules, just a rollout fallback: emit the Fractional rule directly
                // without wrapping in Rule::If (which requires at least 2 arguments).
                return Ok((Some(compile_serve(&config.default_rule)), None));
            }
        }
    }

    // General case (at least one targeting rule):
    // Rule::If([cond1, serve1, ..., condN, serveN, serve_default])
    let mut if_arms: Vec<Rule> = Vec::new();

    for rule in &config.rules {
        let cond = compile_condition(flag, &rule.segments, segments)?;
        let serve = compile_serve(&rule.serve);
        if_arms.push(cond);
        if_arms.push(serve);
    }

    // Trailing else arm (the default)
    if_arms.push(compile_serve(&config.default_rule));

    // default_variant: present only when the fallback is Fixed
    let default_variant = match &config.default_rule {
        ServeTarget::Fixed(vk) => Some(vk.as_str().to_owned()),
        ServeTarget::Rollout(_) => None,
    };

    Ok((Some(Rule::If(if_arms)), default_variant))
}

/// Compiles one flag for a given environment configuration into a `flaps-eval` [`Flag`].
///
/// # Errors
/// - [`CompileError::UnknownVariant`] when a serve target names an undeclared variant.
/// - [`CompileError::UnknownSegment`] when a rule references an unknown segment.
/// - [`CompileError::ObjectVariantNotObject`] when an Object-typed variant value is not a JSON object.
/// - [`CompileError::PredicateArity`] / [`CompileError::NonScalarPredicateValue`] from segment inlining.
pub(crate) fn compile_flag(
    flag_key: &FlagKey,
    domain_variants: &DomainVariants,
    config: &FlagEnvConfig,
    segments: &Segments<'_>,
) -> Result<Flag, CompileError> {
    let flag_str = flag_key.as_str();

    let state = if config.enabled {
        State::Enabled
    } else {
        State::Disabled
    };

    let variants = compile_variants(flag_str, domain_variants)?;

    // Validate all variant references before building the targeting tree.
    validate_serve_target(flag_str, &config.default_rule, domain_variants)?;
    for rule in &config.rules {
        validate_serve_target(flag_str, &rule.serve, domain_variants)?;
    }

    let (targeting, default_variant) = compile_targeting(flag_str, config, segments)?;

    Ok(Flag {
        state,
        variants,
        default_variant,
        targeting,
        metadata: Metadata::new(),
    })
}
