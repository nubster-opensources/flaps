//! Compiler from the Flaps domain model to canonical flagd rulesets.
//!
//! Produces one versioned, content-hashed [`CompiledRuleset`] per environment:
//! reusable segments are inlined and per-environment overrides are resolved
//! at compile time. The compiler is pure: no I/O, no async, no global state.
//!
//! # Usage
//!
//! ```no_run
//! use std::collections::BTreeMap;
//! use flaps_compiler::{compile_environment, FlagConfig, Segments};
//! ```
//!
//! # Determinism
//!
//! The output is deterministic: identical inputs always produce the same
//! `content_hash`. This is achieved by routing all variant maps through
//! `BTreeMap` (which provides stable ordering) and by avoiding all sources
//! of non-determinism (`HashMap` iteration, timestamps, random values).

pub mod error;
pub mod input;
pub mod ruleset;

mod flag_compiler;
mod segment_compiler;

use std::collections::{BTreeMap, HashSet};

use sha2::{Digest, Sha256};

use flaps_domain::key::{EnvironmentKey, SegmentKey};
use flaps_domain::metadata::Metadata as DomainMetadata;
use flaps_eval::FlagSet;

pub use error::CompileError;
pub use input::{FlagConfig, Segments};
pub use ruleset::CompiledRuleset;

/// Compiles all flags configured in one environment into a canonical [`CompiledRuleset`].
///
/// The compiler:
/// 1. Translates each [`FlagConfig`] into a `flaps-eval` [`flaps_eval::Flag`].
/// 2. Assembles a [`FlagSet`] and serializes it to canonical flagd JSON via `to_json()`.
/// 3. Hashes the document with SHA-256 (hex-encoded) to produce a content-addressable ETag.
/// 4. Advances the monotone version counter only when the hash changes.
/// 5. Validates the document by calling `FlagSet::from_json` (round-trip proof, AC#1).
///
/// # Errors
///
/// Returns [`CompileError`] on:
/// - unknown segment or variant references,
/// - invalid predicate arity or non-scalar values,
/// - non-object JSON values for `Object`-typed variants,
/// - or if the produced document is rejected by the evaluator (internal bug guard).
pub fn compile_environment(
    environment: &EnvironmentKey,
    flags: &[FlagConfig<'_>],
    segments: &Segments<'_>,
    environment_metadata: &DomainMetadata,
    previous: Option<&CompiledRuleset>,
) -> Result<CompiledRuleset, CompileError> {
    // Build the flag map; BTreeMap guarantees stable key ordering.
    let mut flag_map = BTreeMap::new();

    for fc in flags {
        let compiled = flag_compiler::compile_flag(
            &fc.flag.key,
            &fc.flag.variants,
            fc.config,
            segments,
            &fc.flag.metadata,
        )?;
        flag_map.insert(fc.flag.key.as_str().to_owned(), compiled);
    }

    let flag_set = FlagSet {
        flags: flag_map,
        metadata: flag_compiler::compile_metadata(environment_metadata),
    };

    let document = flag_set.to_json();

    // Content hash: hex-encoded SHA-256 of the canonical document.
    let content_hash = {
        let mut hasher = Sha256::new();
        hasher.update(document.as_bytes());
        hex::encode(hasher.finalize())
    };

    // Monotone version: stable when hash unchanged, incremented otherwise.
    let version = match previous {
        Some(prev) if prev.content_hash == content_hash => prev.version,
        Some(prev) => prev.version + 1,
        None => 1,
    };

    // AC#1: validate the compiled document is accepted by flaps-eval.
    FlagSet::from_json(&document).map_err(|e| CompileError::EvaluatorRejected {
        environment: environment.as_str().to_owned(),
        reason: e.to_string(),
    })?;

    Ok(CompiledRuleset {
        environment: environment.clone(),
        document,
        content_hash,
        version,
    })
}

/// Returns the set of environments whose flags reference `segment`.
///
/// When a segment is edited, only the environments returned here need to be
/// recompiled. This is the reverse-index required by AC#2.
#[must_use]
pub fn environments_referencing_segment<S: ::std::hash::BuildHasher>(
    segment: &SegmentKey,
    flags_by_environment: &std::collections::HashMap<EnvironmentKey, Vec<FlagConfig<'_>>, S>,
) -> HashSet<EnvironmentKey> {
    let mut result = HashSet::new();

    for (env, flag_configs) in flags_by_environment {
        'flag_loop: for fc in flag_configs {
            for rule in &fc.config.rules {
                for sk in &rule.segments {
                    if sk == segment {
                        result.insert(env.clone());
                        break 'flag_loop;
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use flaps_domain::{
        flag::{Flag, FlagType},
        flag_env_config::{FlagEnvConfig, ServeTarget, TargetingRule, WeightedVariant},
        key::{EnvironmentKey, FlagKey, SegmentKey, VariantKey},
        segment::{MatchOperator, Predicate, Segment, SegmentMatch},
        variant::{ValueType, VariantValue, Variants as DomainVariants},
    };
    use flaps_eval::FlagSet;

    use super::*;

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    fn ek(s: &str) -> EnvironmentKey {
        EnvironmentKey::new(s).unwrap()
    }

    fn fk(s: &str) -> FlagKey {
        FlagKey::new(s).unwrap()
    }

    fn vk(s: &str) -> VariantKey {
        VariantKey::new(s).unwrap()
    }

    fn sk(s: &str) -> SegmentKey {
        SegmentKey::new(s).unwrap()
    }

    fn bool_flag(key: &str) -> Flag {
        Flag {
            key: fk(key),
            name: key.to_owned(),
            description: None,
            flag_type: FlagType::Release,
            value_type: ValueType::Boolean,
            variants: DomainVariants::new(
                ValueType::Boolean,
                [
                    (vk("on"), VariantValue::Bool(true)),
                    (vk("off"), VariantValue::Bool(false)),
                ],
            )
            .unwrap(),
            metadata: flaps_domain::metadata::Metadata::new(),
        }
    }

    fn string_flag(key: &str) -> Flag {
        Flag {
            key: fk(key),
            name: key.to_owned(),
            description: None,
            flag_type: FlagType::Experiment,
            value_type: ValueType::String,
            variants: DomainVariants::new(
                ValueType::String,
                [
                    (vk("a"), VariantValue::String("alpha".into())),
                    (vk("b"), VariantValue::String("beta".into())),
                ],
            )
            .unwrap(),
            metadata: flaps_domain::metadata::Metadata::new(),
        }
    }

    fn simple_config(variant: &str) -> FlagEnvConfig {
        FlagEnvConfig {
            enabled: true,
            rules: vec![],
            default_rule: ServeTarget::Fixed(vk(variant)),
        }
    }

    fn disabled_config(variant: &str) -> FlagEnvConfig {
        FlagEnvConfig {
            enabled: false,
            rules: vec![],
            default_rule: ServeTarget::Fixed(vk(variant)),
        }
    }

    fn no_segments() -> Segments<'static> {
        Segments::new([])
    }

    // -------------------------------------------------------------------------
    // 1. Variants: each value_type compiles to the right arm
    // -------------------------------------------------------------------------

    #[test]
    fn boolean_variants_compile() {
        let flag = bool_flag("my-flag");
        let config = simple_config("on");
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "boolean flag should compile: {result:?}");
        let doc = result.unwrap().document;
        let parsed = FlagSet::from_json(&doc).unwrap();
        let compiled_flag = &parsed.flags["my-flag"];
        assert!(matches!(
            compiled_flag.variants,
            flaps_eval::Variants::Boolean(_)
        ));
    }

    #[test]
    fn string_variants_compile() {
        let flag = string_flag("str-flag");
        let config = simple_config("a");
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok());
        let doc = result.unwrap().document;
        let parsed = FlagSet::from_json(&doc).unwrap();
        assert!(matches!(
            parsed.flags["str-flag"].variants,
            flaps_eval::Variants::String(_)
        ));
    }

    #[test]
    fn number_variants_compile() {
        let flag = Flag {
            key: fk("num-flag"),
            name: "num-flag".into(),
            description: None,
            flag_type: FlagType::Experiment,
            value_type: ValueType::Number,
            variants: DomainVariants::new(
                ValueType::Number,
                [(vk("high"), VariantValue::Number(1.0))],
            )
            .unwrap(),
            metadata: flaps_domain::metadata::Metadata::new(),
        };
        let config = simple_config("high");
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok());
        let parsed = FlagSet::from_json(&result.unwrap().document).unwrap();
        assert!(matches!(
            parsed.flags["num-flag"].variants,
            flaps_eval::Variants::Number(_)
        ));
    }

    #[test]
    fn object_variants_compile_when_json_is_object() {
        let flag = Flag {
            key: fk("cfg-flag"),
            name: "cfg-flag".into(),
            description: None,
            flag_type: FlagType::Ops,
            value_type: ValueType::Object,
            variants: DomainVariants::new(
                ValueType::Object,
                [(vk("v1"), VariantValue::Json(serde_json::json!({"key": 1})))],
            )
            .unwrap(),
            metadata: flaps_domain::metadata::Metadata::new(),
        };
        let config = simple_config("v1");
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn object_variant_not_object_returns_error() {
        // Json value is an array, not an object -> ObjectVariantNotObject
        let flag = Flag {
            key: fk("cfg-flag"),
            name: "cfg-flag".into(),
            description: None,
            flag_type: FlagType::Ops,
            value_type: ValueType::Object,
            variants: DomainVariants::new(
                ValueType::Object,
                [(vk("v1"), VariantValue::Json(serde_json::json!([1, 2, 3])))],
            )
            .unwrap(),
            metadata: flaps_domain::metadata::Metadata::new(),
        };
        let config = simple_config("v1");
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(
            matches!(result, Err(CompileError::ObjectVariantNotObject { .. })),
            "expected ObjectVariantNotObject, got {result:?}"
        );
    }

    // -------------------------------------------------------------------------
    // 2. Serve: Fixed / Rollout / default without rules
    // -------------------------------------------------------------------------

    #[test]
    fn no_rules_fixed_default_produces_no_targeting() {
        let flag = bool_flag("my-flag");
        let config = simple_config("on");
        let env = ek("prod");
        let ruleset = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        let parsed = FlagSet::from_json(&ruleset.document).unwrap();
        let compiled_flag = &parsed.flags["my-flag"];
        assert!(
            compiled_flag.targeting.is_none(),
            "should have no targeting"
        );
        assert_eq!(compiled_flag.default_variant, Some("on".to_owned()));
    }

    #[test]
    fn rollout_produce_fractional_rule() {
        let flag = bool_flag("rollout-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![],
            default_rule: ServeTarget::rollout(vec![
                WeightedVariant {
                    variant: vk("on"),
                    weight: 30,
                },
                WeightedVariant {
                    variant: vk("off"),
                    weight: 70,
                },
            ])
            .unwrap(),
        };
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
        let doc = result.unwrap().document;
        // Fractional targeting must be present (rollout default with no rules still needs targeting)
        let parsed = FlagSet::from_json(&doc).unwrap();
        let f = &parsed.flags["rollout-flag"];
        assert!(f.targeting.is_some(), "rollout should produce targeting");
    }

    #[test]
    fn disabled_flag_has_disabled_state() {
        let flag = bool_flag("my-flag");
        let config = disabled_config("off");
        let env = ek("prod");
        let ruleset = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        let parsed = FlagSet::from_json(&ruleset.document).unwrap();
        assert_eq!(parsed.flags["my-flag"].state, flaps_eval::State::Disabled);
    }

    // -------------------------------------------------------------------------
    // 3. Segment inlining: And/Or/Not/Predicate -> flagd targeting
    // -------------------------------------------------------------------------

    fn beta_segment(key: &str) -> Segment {
        Segment {
            key: sk(key),
            name: key.into(),
            match_expr: SegmentMatch::Predicate(Predicate {
                attribute: "tier".into(),
                operator: MatchOperator::Equals,
                values: vec![serde_json::json!("beta")],
            }),
        }
    }

    #[test]
    fn single_segment_inlined_as_predicate() {
        let seg = beta_segment("beta-users");
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("beta-users")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segs = Segments::new([(sk("beta-users"), &seg.match_expr)]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
        let doc = result.unwrap().document;
        // Ensure round-trip works
        FlagSet::from_json(&doc).unwrap();
        // The targeting tree should contain the "==" operator
        assert!(doc.contains("=="), "should contain equality operator");
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn multiple_segments_inlined_as_and() {
        let seg1 = beta_segment("seg1");
        let seg2 = Segment {
            key: sk("seg2"),
            name: "seg2".into(),
            match_expr: SegmentMatch::Predicate(Predicate {
                attribute: "role".into(),
                operator: MatchOperator::Equals,
                values: vec![serde_json::json!("admin")],
            }),
        };
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("seg1"), sk("seg2")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segment_lookup = Segments::new([
            (sk("seg1"), &seg1.match_expr),
            (sk("seg2"), &seg2.match_expr),
        ]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segment_lookup,
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
        let doc = result.unwrap().document;
        // "and" operator should be present (multiple segments)
        assert!(
            doc.contains("\"and\""),
            "should contain and operator: {doc}"
        );
    }

    #[test]
    fn zero_segments_produces_literal_true_condition() {
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![], // zero segments -> always match
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
        let doc = result.unwrap().document;
        assert!(
            doc.contains("true"),
            "zero segments should produce true condition: {doc}"
        );
    }

    #[test]
    fn predicate_arity_error_on_wrong_count() {
        let bad_segment = SegmentMatch::Predicate(Predicate {
            attribute: "email".into(),
            operator: MatchOperator::Equals, // expects exactly 1
            values: vec![],                  // got 0
        });
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("bad")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segs = Segments::new([(sk("bad"), &bad_segment)]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        );
        assert!(
            matches!(result, Err(CompileError::PredicateArity { .. })),
            "expected PredicateArity, got {result:?}"
        );
    }

    #[test]
    fn in_operator_with_multiple_values() {
        let seg = SegmentMatch::Predicate(Predicate {
            attribute: "tier".into(),
            operator: MatchOperator::In,
            values: vec![serde_json::json!("beta"), serde_json::json!("alpha")],
        });
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("tier-check")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segs = Segments::new([(sk("tier-check"), &seg)]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn contains_operator_flips_args() {
        // Contains -> In(Literal(v), Var) -- haystack/needle order is inverted
        let seg = SegmentMatch::Predicate(Predicate {
            attribute: "email".into(),
            operator: MatchOperator::Contains,
            values: vec![serde_json::json!("@example.com")],
        });
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("email-check")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segs = Segments::new([(sk("email-check"), &seg)]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn semver_operator_compiles() {
        let seg = SegmentMatch::Predicate(Predicate {
            attribute: "app-version".into(),
            operator: MatchOperator::SemVerGte,
            values: vec![serde_json::json!("2.0.0")],
        });
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("version-check")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segs = Segments::new([(sk("version-check"), &seg)]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
        let doc = result.unwrap().document;
        assert!(
            doc.contains("sem_ver"),
            "should contain sem_ver operator: {doc}"
        );
    }

    #[test]
    fn non_scalar_predicate_value_returns_error() {
        let bad_segment = SegmentMatch::Predicate(Predicate {
            attribute: "email".into(),
            operator: MatchOperator::Equals,
            values: vec![serde_json::json!({"nested": "object"})],
        });
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("bad")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segs = Segments::new([(sk("bad"), &bad_segment)]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        );
        assert!(
            matches!(result, Err(CompileError::NonScalarPredicateValue { .. })),
            "expected NonScalarPredicateValue, got {result:?}"
        );
    }

    // -------------------------------------------------------------------------
    // 4. Ordered rules -> Rule::If pairs
    // -------------------------------------------------------------------------

    #[test]
    fn multiple_rules_produce_ordered_if_arms() {
        let seg_beta = SegmentMatch::Predicate(Predicate {
            attribute: "tier".into(),
            operator: MatchOperator::Equals,
            values: vec![serde_json::json!("beta")],
        });
        let seg_alpha = SegmentMatch::Predicate(Predicate {
            attribute: "tier".into(),
            operator: MatchOperator::Equals,
            values: vec![serde_json::json!("alpha")],
        });
        let flag = string_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![
                TargetingRule {
                    segments: vec![sk("beta")],
                    serve: ServeTarget::Fixed(vk("b")),
                },
                TargetingRule {
                    segments: vec![sk("alpha")],
                    serve: ServeTarget::Fixed(vk("a")),
                },
            ],
            default_rule: ServeTarget::Fixed(vk("a")),
        };
        let segs = Segments::new([(sk("beta"), &seg_beta), (sk("alpha"), &seg_alpha)]);
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        );
        assert!(result.is_ok(), "{result:?}");
        let doc = result.unwrap().document;
        // "if" operator must be present
        assert!(doc.contains("\"if\""), "should contain if operator: {doc}");
        // beta must appear before alpha in the if arms
        let beta_pos = doc.find("\"beta\"").unwrap_or(usize::MAX);
        let alpha_pos = doc.find("\"alpha\"").unwrap_or(usize::MAX);
        assert!(
            beta_pos < alpha_pos,
            "beta rule should come before alpha rule"
        );
    }

    // -------------------------------------------------------------------------
    // 5. Override by environment (snapshot: two envs, different configs)
    // -------------------------------------------------------------------------

    #[test]
    fn different_envs_produce_different_documents() {
        let flag = bool_flag("my-flag");
        let config_prod = simple_config("on");
        let config_staging = simple_config("off");
        let env_prod = ek("prod");
        let env_staging = ek("staging");

        let rs_prod = compile_environment(
            &env_prod,
            &[FlagConfig {
                flag: &flag,
                config: &config_prod,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        let rs_staging = compile_environment(
            &env_staging,
            &[FlagConfig {
                flag: &flag,
                config: &config_staging,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();

        assert_ne!(rs_prod.document, rs_staging.document);
        assert_ne!(rs_prod.content_hash, rs_staging.content_hash);
    }

    // -------------------------------------------------------------------------
    // 6. Determinism: same input -> same hash
    // -------------------------------------------------------------------------

    #[test]
    fn same_input_produces_identical_hash() {
        let flag = bool_flag("my-flag");
        let config = simple_config("on");
        let env = ek("prod");

        let r1 = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        let r2 = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();

        assert_eq!(r1.content_hash, r2.content_hash);
        assert_eq!(r1.document, r2.document);
    }

    // -------------------------------------------------------------------------
    // 7. Version monotone: stable when unchanged, +1 when changed
    // -------------------------------------------------------------------------

    #[test]
    fn version_starts_at_one_with_no_previous() {
        let flag = bool_flag("my-flag");
        let config = simple_config("on");
        let env = ek("prod");
        let rs = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        assert_eq!(rs.version, 1);
    }

    #[test]
    fn version_stable_when_hash_unchanged() {
        let flag = bool_flag("my-flag");
        let config = simple_config("on");
        let env = ek("prod");

        let first = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        let second = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            Some(&first),
        )
        .unwrap();

        assert_eq!(first.content_hash, second.content_hash);
        assert_eq!(first.version, second.version, "version must not change");
    }

    #[test]
    fn version_increments_when_hash_changes() {
        let flag = bool_flag("my-flag");
        let config_v1 = simple_config("on");
        let config_v2 = simple_config("off");
        let env = ek("prod");

        let first = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config_v1,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        let second = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config_v2,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            Some(&first),
        )
        .unwrap();

        assert_ne!(first.content_hash, second.content_hash);
        assert_eq!(second.version, first.version + 1, "version must increment");
    }

    // -------------------------------------------------------------------------
    // 8. environments_referencing_segment
    // -------------------------------------------------------------------------

    #[test]
    fn environments_referencing_segment_finds_correct_envs() {
        let flag = bool_flag("my-flag");
        let config_with_seg = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("beta-users")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let config_without_seg = simple_config("off");

        let mut by_env: HashMap<EnvironmentKey, Vec<FlagConfig<'_>>> = HashMap::new();
        by_env.insert(
            ek("prod"),
            vec![FlagConfig {
                flag: &flag,
                config: &config_with_seg,
            }],
        );
        by_env.insert(
            ek("dev"),
            vec![FlagConfig {
                flag: &flag,
                config: &config_without_seg,
            }],
        );

        let result = environments_referencing_segment(&sk("beta-users"), &by_env);
        assert!(
            result.contains(&ek("prod")),
            "prod should reference the segment"
        );
        assert!(
            !result.contains(&ek("dev")),
            "dev should not reference the segment"
        );
    }

    // -------------------------------------------------------------------------
    // 11. InvalidVariantValue: NaN f64 variant fails serialization gracefully
    // -------------------------------------------------------------------------

    #[test]
    fn number_variant_nan_returns_invalid_variant_value_error() {
        // DomainVariants::new accepts NaN because matches_type only checks
        // Number == Number without inspecting the f64 payload.  The compiler
        // must therefore handle the serde_json serialization failure gracefully
        // instead of panicking.
        let flag = Flag {
            key: fk("nan-flag"),
            name: "nan-flag".into(),
            description: None,
            flag_type: FlagType::Experiment,
            value_type: ValueType::Number,
            variants: DomainVariants::new(
                ValueType::Number,
                [(vk("bad"), VariantValue::Number(f64::NAN))],
            )
            .unwrap(),
            metadata: flaps_domain::metadata::Metadata::new(),
        };
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![],
            default_rule: ServeTarget::Fixed(vk("bad")),
        };
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(
            matches!(result, Err(CompileError::InvalidVariantValue { .. })),
            "expected InvalidVariantValue for NaN variant, got {result:?}"
        );
    }

    #[test]
    fn environments_referencing_segment_empty_when_no_reference() {
        let flag = bool_flag("my-flag");
        let config = simple_config("on");
        let mut by_env: HashMap<EnvironmentKey, Vec<FlagConfig<'_>>> = HashMap::new();
        by_env.insert(
            ek("prod"),
            vec![FlagConfig {
                flag: &flag,
                config: &config,
            }],
        );

        let result = environments_referencing_segment(&sk("nonexistent"), &by_env);
        assert!(result.is_empty());
    }

    // -------------------------------------------------------------------------
    // 9. Round-trip: every compiled document is re-parsed without error (AC#1)
    // -------------------------------------------------------------------------

    #[test]
    fn round_trip_boolean_flag() {
        let flag = bool_flag("rt-flag");
        let config = simple_config("on");
        let env = ek("prod");
        let rs = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        // AC#1 is enforced inside compile_environment; re-check explicitly.
        FlagSet::from_json(&rs.document).expect("compiled document must round-trip");
    }

    #[test]
    fn round_trip_with_complex_targeting() {
        let seg = SegmentMatch::And(vec![
            SegmentMatch::Predicate(Predicate {
                attribute: "tier".into(),
                operator: MatchOperator::In,
                values: vec![serde_json::json!("beta"), serde_json::json!("alpha")],
            }),
            SegmentMatch::Not(Box::new(SegmentMatch::Predicate(Predicate {
                attribute: "blocked".into(),
                operator: MatchOperator::Equals,
                values: vec![serde_json::json!(true)],
            }))),
        ]);
        let flag = bool_flag("complex-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("complex-seg")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let segs = Segments::new([(sk("complex-seg"), &seg)]);
        let env = ek("prod");
        let rs = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &segs,
            &DomainMetadata::new(),
            None,
        )
        .unwrap();
        FlagSet::from_json(&rs.document).expect("complex document must round-trip");
    }

    // -------------------------------------------------------------------------
    // 10. Unknown references: UnknownSegment / UnknownVariant
    // -------------------------------------------------------------------------

    #[test]
    fn unknown_segment_returns_error() {
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![sk("ghost-segment")],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(
            matches!(result, Err(CompileError::UnknownSegment { .. })),
            "expected UnknownSegment, got {result:?}"
        );
    }

    #[test]
    fn unknown_variant_in_fixed_serve_returns_error() {
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![],
            default_rule: ServeTarget::Fixed(VariantKey::new("nonexistent").unwrap()),
        };
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(
            matches!(result, Err(CompileError::UnknownVariant { .. })),
            "expected UnknownVariant, got {result:?}"
        );
    }

    #[test]
    fn unknown_variant_in_rollout_returns_error() {
        let flag = bool_flag("my-flag");
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![],
            default_rule: ServeTarget::rollout(vec![
                WeightedVariant {
                    variant: VariantKey::new("on").unwrap(),
                    weight: 50,
                },
                WeightedVariant {
                    variant: VariantKey::new("ghost").unwrap(),
                    weight: 50,
                },
            ])
            .unwrap(),
        };
        let env = ek("prod");
        let result = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        );
        assert!(
            matches!(result, Err(CompileError::UnknownVariant { .. })),
            "expected UnknownVariant for rollout, got {result:?}"
        );
    }

    // -------------------------------------------------------------------------
    // 12. Metadata propagation (#55): flag-level and flag-set-level
    // -------------------------------------------------------------------------

    #[test]
    fn flag_metadata_is_carried_into_the_compiled_document() {
        let mut flag = bool_flag("my-flag");
        flag.metadata.insert(
            "owner".to_owned(),
            flaps_domain::metadata::MetadataValue::String("team-a".into()),
        );
        let config = simple_config("on");
        let env = ek("prod");
        let ruleset = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &DomainMetadata::new(),
            None,
        )
        .unwrap();

        assert!(
            ruleset.document.contains("\"owner\""),
            "compiled document should carry flag metadata: {}",
            ruleset.document
        );
        let parsed = FlagSet::from_json(&ruleset.document).unwrap();
        let compiled_flag = &parsed.flags["my-flag"];
        assert_eq!(
            compiled_flag.metadata.get("owner"),
            Some(&flaps_eval::MetadataValue::String("team-a".into()))
        );
    }

    #[test]
    fn environment_metadata_is_carried_into_flag_set_metadata() {
        let flag = bool_flag("my-flag");
        let config = simple_config("on");
        let env = ek("prod");
        let mut environment_metadata = DomainMetadata::new();
        environment_metadata.insert(
            "region".to_owned(),
            flaps_domain::metadata::MetadataValue::String("eu-west".into()),
        );

        let ruleset = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &environment_metadata,
            None,
        )
        .unwrap();

        assert!(
            ruleset.document.contains("\"region\""),
            "compiled document should carry environment metadata: {}",
            ruleset.document
        );
        let parsed = FlagSet::from_json(&ruleset.document).unwrap();
        assert_eq!(
            parsed.metadata.get("region"),
            Some(&flaps_eval::MetadataValue::String("eu-west".into())),
            "FlagSet::from_json must relit the flag-set metadata (round-trip)"
        );
    }

    #[test]
    fn flag_metadata_and_environment_metadata_coexist_at_their_own_level() {
        let mut flag = bool_flag("my-flag");
        flag.metadata.insert(
            "team".to_owned(),
            flaps_domain::metadata::MetadataValue::String("flag-owner".into()),
        );
        let config = simple_config("on");
        let env = ek("prod");
        let mut environment_metadata = DomainMetadata::new();
        environment_metadata.insert(
            "team".to_owned(),
            flaps_domain::metadata::MetadataValue::String("flagset-owner".into()),
        );

        let ruleset = compile_environment(
            &env,
            &[FlagConfig {
                flag: &flag,
                config: &config,
            }],
            &no_segments(),
            &environment_metadata,
            None,
        )
        .unwrap();

        let parsed = FlagSet::from_json(&ruleset.document).unwrap();
        // Both levels keep their own metadata in the compiled document; the
        // engine merges them at evaluation time (flag wins), not the compiler.
        assert_eq!(
            parsed.flags["my-flag"].metadata.get("team"),
            Some(&flaps_eval::MetadataValue::String("flag-owner".into()))
        );
        assert_eq!(
            parsed.metadata.get("team"),
            Some(&flaps_eval::MetadataValue::String("flagset-owner".into()))
        );
    }
}
