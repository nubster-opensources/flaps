//! Per-environment flag configuration: targeting rules and rollout weights.

use serde::{Deserialize, Serialize};

use crate::{
    error::DomainError,
    key::{SegmentKey, VariantKey},
};

/// A variant paired with a non-negative integer weight for rollout distribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeightedVariant {
    /// The variant to serve.
    pub variant: VariantKey,
    /// Relative weight (0 is allowed; at least one weight must be > 0).
    pub weight: u32,
}

/// Determines which variant to serve when a rule matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServeTarget {
    /// Always serve a specific variant.
    Fixed(VariantKey),
    /// Distribute traffic across variants according to weights.
    ///
    /// Use [`ServeTarget::rollout`] to construct a validated rollout.
    Rollout(Vec<WeightedVariant>),
}

impl ServeTarget {
    /// Constructs a `Rollout` target, validating that the total weight is positive.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidRollout`] when the sum of weights is zero.
    pub fn rollout(weights: Vec<WeightedVariant>) -> Result<Self, DomainError> {
        let total: u64 = weights.iter().map(|w| u64::from(w.weight)).sum();
        if total == 0 {
            return Err(DomainError::InvalidRollout);
        }
        Ok(Self::Rollout(weights))
    }
}

/// A targeting rule: the flag is served via `serve` when the evaluation context
/// belongs to **all** segments listed in `segments`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetingRule {
    /// The segments that must all match for this rule to fire.
    pub segments: Vec<SegmentKey>,
    /// How to serve the flag when this rule fires.
    pub serve: ServeTarget,
}

/// Per-environment flag configuration.
///
/// Rules are evaluated in order; the first matching rule wins. If no rule
/// matches, `default_rule` is applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlagEnvConfig {
    /// Whether the flag is active in this environment.
    pub enabled: bool,
    /// Ordered list of targeting rules.
    pub rules: Vec<TargetingRule>,
    /// Fallback serve target applied when no rule matches.
    pub default_rule: ServeTarget,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::{SegmentKey, VariantKey};

    fn vk(s: &str) -> VariantKey {
        VariantKey::new(s).unwrap()
    }

    #[test]
    fn rollout_ok_positive_total() {
        let weights = vec![
            WeightedVariant {
                variant: vk("on"),
                weight: 70,
            },
            WeightedVariant {
                variant: vk("off"),
                weight: 30,
            },
        ];
        assert!(ServeTarget::rollout(weights).is_ok());
    }

    #[test]
    fn rollout_err_zero_total() {
        let weights = vec![
            WeightedVariant {
                variant: vk("on"),
                weight: 0,
            },
            WeightedVariant {
                variant: vk("off"),
                weight: 0,
            },
        ];
        assert!(matches!(
            ServeTarget::rollout(weights),
            Err(DomainError::InvalidRollout)
        ));
    }

    #[test]
    fn rollout_ok_partial_zero_weight() {
        // One zero weight is fine as long as total > 0
        let weights = vec![
            WeightedVariant {
                variant: vk("on"),
                weight: 100,
            },
            WeightedVariant {
                variant: vk("off"),
                weight: 0,
            },
        ];
        assert!(ServeTarget::rollout(weights).is_ok());
    }

    #[test]
    fn flag_env_config_serde_round_trip() {
        let config = FlagEnvConfig {
            enabled: true,
            rules: vec![TargetingRule {
                segments: vec![SegmentKey::new("beta-users").unwrap()],
                serve: ServeTarget::Fixed(vk("on")),
            }],
            default_rule: ServeTarget::rollout(vec![
                WeightedVariant {
                    variant: vk("on"),
                    weight: 10,
                },
                WeightedVariant {
                    variant: vk("off"),
                    weight: 90,
                },
            ])
            .unwrap(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: FlagEnvConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn default_rule_is_required_field() {
        // Structural: FlagEnvConfig::default_rule field must be present in serde JSON
        let config = FlagEnvConfig {
            enabled: false,
            rules: vec![],
            default_rule: ServeTarget::Fixed(vk("off")),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("default_rule"));
    }
}
