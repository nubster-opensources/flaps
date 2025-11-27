//! Flag evaluation engine.

use std::collections::HashMap;
use std::io::Cursor;

use serde::{Deserialize, Serialize};

use crate::context::EvaluationContext;
use crate::flag::{Flag, FlagValue};
use crate::rule::{AttributeValue, Condition, Operator, RuleId, TargetingRule};
use crate::segment::{Segment, SegmentId};

/// Result of a flag evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationResult {
    /// The evaluated flag value.
    pub value: FlagValue,
    /// Reason for this evaluation result.
    pub reason: EvaluationReason,
    /// ID of the rule that matched (if any).
    pub rule_id: Option<RuleId>,
    /// Whether the user was in a rollout percentage.
    pub in_rollout: Option<bool>,
}

impl EvaluationResult {
    /// Creates a result for a default value.
    pub fn default_value(value: FlagValue) -> Self {
        Self {
            value,
            reason: EvaluationReason::Default,
            rule_id: None,
            in_rollout: None,
        }
    }

    /// Creates a result for a disabled flag.
    pub fn disabled(value: FlagValue) -> Self {
        Self {
            value,
            reason: EvaluationReason::FlagDisabled,
            rule_id: None,
            in_rollout: None,
        }
    }

    /// Creates a result for a flag not found.
    pub fn flag_not_found() -> Self {
        Self {
            value: FlagValue::Boolean(false),
            reason: EvaluationReason::FlagNotFound,
            rule_id: None,
            in_rollout: None,
        }
    }

    /// Creates a result for an environment not found.
    pub fn environment_not_found() -> Self {
        Self {
            value: FlagValue::Boolean(false),
            reason: EvaluationReason::EnvironmentNotFound,
            rule_id: None,
            in_rollout: None,
        }
    }

    /// Returns true if the flag is enabled and the value is truthy.
    ///
    /// This returns false when:
    /// - The flag is disabled in the environment
    /// - The flag was not found
    /// - The environment was not found
    /// - The user was excluded from rollout
    /// - There was an error during evaluation
    ///
    /// This ensures kill switches work correctly for all flag types,
    /// including string flags where the fallback value might be non-empty.
    pub fn is_enabled(&self) -> bool {
        match self.reason {
            EvaluationReason::FlagDisabled
            | EvaluationReason::FlagNotFound
            | EvaluationReason::EnvironmentNotFound
            | EvaluationReason::RolloutExcluded
            | EvaluationReason::Error => false,
            EvaluationReason::Default
            | EvaluationReason::TargetingMatch
            | EvaluationReason::RolloutIncluded => self.value.is_truthy(),
        }
    }

    /// Returns the boolean value or false.
    pub fn as_bool(&self) -> bool {
        self.value.as_bool().unwrap_or(false)
    }

    /// Returns the string value or empty string.
    pub fn as_str(&self) -> &str {
        self.value.as_str().unwrap_or("")
    }
}

/// Reason for an evaluation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationReason {
    /// Default value was returned (no rules matched).
    Default,
    /// A targeting rule matched.
    TargetingMatch,
    /// User was included in rollout percentage.
    RolloutIncluded,
    /// User was excluded from rollout percentage.
    RolloutExcluded,
    /// Flag is disabled in this environment.
    FlagDisabled,
    /// Environment was not found.
    EnvironmentNotFound,
    /// Flag was not found.
    FlagNotFound,
    /// Error during evaluation.
    Error,
}

/// The flag evaluation engine.
///
/// The evaluator processes flags and their targeting rules to determine
/// what value should be returned for a given user context.
#[derive(Debug, Clone, Default)]
pub struct Evaluator {
    /// Cached segments for segment-based targeting.
    segments: HashMap<SegmentId, Segment>,
}

impl Evaluator {
    /// Creates a new evaluator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an evaluator with preloaded segments.
    pub fn with_segments(segments: Vec<Segment>) -> Self {
        let segments = segments.into_iter().map(|s| (s.id, s)).collect();
        Self { segments }
    }

    /// Adds a segment to the evaluator.
    pub fn add_segment(&mut self, segment: Segment) {
        self.segments.insert(segment.id, segment);
    }

    /// Evaluates a flag for the given environment and context.
    pub fn evaluate(
        &self,
        flag: &Flag,
        environment: &str,
        context: &EvaluationContext,
    ) -> EvaluationResult {
        // Get environment config
        let env_config = match flag.get_environment(environment) {
            Some(config) => config,
            None => return EvaluationResult::environment_not_found(),
        };

        // Check if flag is enabled
        if !env_config.enabled {
            return EvaluationResult::disabled(flag.default_value());
        }

        // Sort rules by priority
        let mut rules: Vec<&TargetingRule> = env_config.rules.iter().collect();
        rules.sort_by_key(|r| r.priority);

        // Evaluate each rule in priority order
        for rule in rules {
            if self.evaluate_rule(rule, context) {
                // Rule matched, check rollout percentage
                if let Some(percentage) = rule.rollout_percentage {
                    let user_id = context.effective_user_id();
                    if self.is_in_rollout(&user_id, flag.key.as_str(), percentage) {
                        return EvaluationResult {
                            value: rule.value.clone(),
                            reason: EvaluationReason::TargetingMatch,
                            rule_id: Some(rule.id),
                            in_rollout: Some(true),
                        };
                    } else {
                        // User not in rollout for this rule, continue to next rule
                        continue;
                    }
                }

                // No rollout percentage, rule fully applies
                return EvaluationResult {
                    value: rule.value.clone(),
                    reason: EvaluationReason::TargetingMatch,
                    rule_id: Some(rule.id),
                    in_rollout: None,
                };
            }
        }

        // No rules matched, apply global rollout if configured
        if let Some(percentage) = env_config.rollout_percentage {
            let user_id = context.effective_user_id();
            let in_rollout = self.is_in_rollout(&user_id, flag.key.as_str(), percentage);
            return EvaluationResult {
                value: if in_rollout {
                    env_config.default_value.clone()
                } else {
                    flag.default_value()
                },
                reason: if in_rollout {
                    EvaluationReason::RolloutIncluded
                } else {
                    EvaluationReason::RolloutExcluded
                },
                rule_id: None,
                in_rollout: Some(in_rollout),
            };
        }

        // Return default value
        EvaluationResult::default_value(env_config.default_value.clone())
    }

    /// Evaluates a targeting rule against the context.
    fn evaluate_rule(&self, rule: &TargetingRule, context: &EvaluationContext) -> bool {
        // Empty conditions = catch-all rule
        if rule.conditions.is_empty() {
            return true;
        }

        // All conditions must match (AND logic)
        rule.conditions
            .iter()
            .all(|c| self.evaluate_condition(c, context))
    }

    /// Evaluates a single condition against the context.
    fn evaluate_condition(&self, condition: &Condition, context: &EvaluationContext) -> bool {
        // Special case: segment matching
        if condition.operator == Operator::MatchesSegment {
            if let Some(segment_id) = condition.value.as_segment_ref() {
                return self.evaluate_segment_membership(segment_id, context);
            }
            return false;
        }

        if condition.operator == Operator::NotMatchesSegment {
            if let Some(segment_id) = condition.value.as_segment_ref() {
                return !self.evaluate_segment_membership(segment_id, context);
            }
            return false;
        }

        // Get attribute value from context
        let attr_value = match context.get(&condition.attribute) {
            Some(value) => value,
            None => return false, // Attribute not found, condition fails
        };

        self.compare_values(attr_value, &condition.operator, &condition.value)
    }

    /// Compares two values using the given operator.
    fn compare_values(
        &self,
        actual: &AttributeValue,
        operator: &Operator,
        expected: &AttributeValue,
    ) -> bool {
        match operator {
            Operator::Equals => self.values_equal(actual, expected),
            Operator::NotEquals => !self.values_equal(actual, expected),

            Operator::Contains => {
                if let (Some(actual_str), Some(expected_str)) = (actual.as_str(), expected.as_str())
                {
                    actual_str.contains(expected_str)
                } else {
                    false
                }
            },

            Operator::StartsWith => {
                if let (Some(actual_str), Some(expected_str)) = (actual.as_str(), expected.as_str())
                {
                    actual_str.starts_with(expected_str)
                } else {
                    false
                }
            },

            Operator::EndsWith => {
                if let (Some(actual_str), Some(expected_str)) = (actual.as_str(), expected.as_str())
                {
                    actual_str.ends_with(expected_str)
                } else {
                    false
                }
            },

            Operator::In => {
                if let Some(list) = expected.as_string_list() {
                    if let Some(actual_str) = actual.as_str() {
                        list.iter().any(|s| s == actual_str)
                    } else {
                        false
                    }
                } else {
                    false
                }
            },

            Operator::NotIn => {
                if let Some(list) = expected.as_string_list() {
                    if let Some(actual_str) = actual.as_str() {
                        !list.iter().any(|s| s == actual_str)
                    } else {
                        true // Not found = not in list
                    }
                } else {
                    true
                }
            },

            Operator::GreaterThan => {
                if let (Some(actual_num), Some(expected_num)) =
                    (actual.as_number(), expected.as_number())
                {
                    actual_num > expected_num
                } else {
                    false
                }
            },

            Operator::GreaterThanOrEqual => {
                if let (Some(actual_num), Some(expected_num)) =
                    (actual.as_number(), expected.as_number())
                {
                    actual_num >= expected_num
                } else {
                    false
                }
            },

            Operator::LessThan => {
                if let (Some(actual_num), Some(expected_num)) =
                    (actual.as_number(), expected.as_number())
                {
                    actual_num < expected_num
                } else {
                    false
                }
            },

            Operator::LessThanOrEqual => {
                if let (Some(actual_num), Some(expected_num)) =
                    (actual.as_number(), expected.as_number())
                {
                    actual_num <= expected_num
                } else {
                    false
                }
            },

            Operator::SemverGreaterThan | Operator::SemverLessThan => {
                // TODO: Implement semver comparison
                false
            },

            Operator::Regex => {
                // TODO: Implement regex matching
                false
            },

            Operator::MatchesSegment | Operator::NotMatchesSegment => {
                // Handled above
                false
            },
        }
    }

    /// Checks if two attribute values are equal.
    fn values_equal(&self, a: &AttributeValue, b: &AttributeValue) -> bool {
        match (a, b) {
            (AttributeValue::String(a), AttributeValue::String(b)) => a == b,
            (AttributeValue::Number(a), AttributeValue::Number(b)) => (a - b).abs() < f64::EPSILON,
            (AttributeValue::Boolean(a), AttributeValue::Boolean(b)) => a == b,
            _ => false,
        }
    }

    /// Evaluates if a user belongs to a segment.
    fn evaluate_segment_membership(
        &self,
        segment_id: SegmentId,
        context: &EvaluationContext,
    ) -> bool {
        let segment = match self.segments.get(&segment_id) {
            Some(s) => s,
            None => return false, // Segment not found
        };

        // Check explicit exclusions first (highest priority)
        if let Some(ref user_id) = context.user_id {
            if segment.is_excluded(user_id) {
                return false;
            }
        }

        // Check explicit inclusions
        if let Some(ref user_id) = context.user_id {
            if segment.is_included(user_id) {
                return true;
            }
        }

        // Evaluate segment rules (OR logic between rules)
        for rule in &segment.rules {
            // All conditions in a rule must match (AND logic)
            let rule_matches = rule.conditions.iter().all(|c| {
                let condition = Condition::new(c.attribute.clone(), c.operator, c.value.clone());
                self.evaluate_condition(&condition, context)
            });

            if rule_matches {
                return true;
            }
        }

        false
    }

    /// Determines if a user is in the rollout percentage.
    ///
    /// Uses a stable hash so the same user always gets the same result
    /// for a given flag.
    pub fn is_in_rollout(&self, user_id: &str, flag_key: &str, percentage: u8) -> bool {
        if percentage >= 100 {
            return true;
        }
        if percentage == 0 {
            return false;
        }

        // Create a stable key combining user and flag
        let key = format!("{}{}", flag_key, user_id);
        let hash = self.murmur3_hash(&key);
        let bucket = (hash % 100) as u8;

        bucket < percentage
    }

    /// Computes a murmur3 hash of the input string.
    fn murmur3_hash(&self, input: &str) -> u32 {
        let mut reader = Cursor::new(input.as_bytes());
        murmur3::murmur3_32(&mut reader, 0).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use crate::environment::EnvironmentConfig;
    use crate::flag::UserId;
    use crate::project::ProjectId;

    use super::*;

    fn create_test_flag() -> Flag {
        Flag::new_boolean(
            "test-flag",
            "Test Flag",
            ProjectId::new(),
            UserId::new("creator"),
        )
        .with_environment("dev", EnvironmentConfig::enabled_boolean(true))
        .with_environment("prod", EnvironmentConfig::disabled())
    }

    #[test]
    fn test_evaluate_enabled_flag() {
        let evaluator = Evaluator::new();
        let flag = create_test_flag();
        let context = EvaluationContext::with_user_id("user-1");

        let result = evaluator.evaluate(&flag, "dev", &context);
        assert!(result.is_enabled());
        assert_eq!(result.reason, EvaluationReason::Default);
    }

    #[test]
    fn test_evaluate_disabled_flag() {
        let evaluator = Evaluator::new();
        let flag = create_test_flag();
        let context = EvaluationContext::with_user_id("user-1");

        let result = evaluator.evaluate(&flag, "prod", &context);
        assert!(!result.is_enabled());
        assert_eq!(result.reason, EvaluationReason::FlagDisabled);
    }

    #[test]
    fn test_evaluate_unknown_environment() {
        let evaluator = Evaluator::new();
        let flag = create_test_flag();
        let context = EvaluationContext::with_user_id("user-1");

        let result = evaluator.evaluate(&flag, "unknown", &context);
        assert!(!result.is_enabled());
        assert_eq!(result.reason, EvaluationReason::EnvironmentNotFound);
    }

    #[test]
    fn test_evaluate_with_targeting_rule() {
        let evaluator = Evaluator::new();
        let flag = Flag::new_boolean(
            "premium-feature",
            "Premium Feature",
            ProjectId::new(),
            UserId::new("creator"),
        )
        .with_environment(
            "prod",
            EnvironmentConfig::enabled_boolean(false).with_rule(
                TargetingRule::new(1, FlagValue::Boolean(true))
                    .with_condition(Condition::equals("plan", "pro")),
            ),
        );

        // User with pro plan
        let pro_context = EvaluationContext::with_user_id("user-1").set("plan", "pro");
        let result = evaluator.evaluate(&flag, "prod", &pro_context);
        assert!(result.is_enabled());
        assert_eq!(result.reason, EvaluationReason::TargetingMatch);

        // User with free plan
        let free_context = EvaluationContext::with_user_id("user-2").set("plan", "free");
        let result = evaluator.evaluate(&flag, "prod", &free_context);
        assert!(!result.is_enabled());
        assert_eq!(result.reason, EvaluationReason::Default);
    }

    #[test]
    fn test_rollout_percentage_stability() {
        let evaluator = Evaluator::new();

        // Same user + flag should always give same result
        let in_rollout_1 = evaluator.is_in_rollout("user-123", "my-flag", 50);
        let in_rollout_2 = evaluator.is_in_rollout("user-123", "my-flag", 50);
        assert_eq!(in_rollout_1, in_rollout_2);

        // Different users may get different results
        // (we can't test exact values, but we can test distribution)
        let mut in_count = 0;
        for i in 0..1000 {
            if evaluator.is_in_rollout(&format!("user-{}", i), "test-flag", 50) {
                in_count += 1;
            }
        }
        // Should be roughly 50% (allow 10% margin for randomness)
        assert!(
            in_count > 400 && in_count < 600,
            "Got {} in rollout",
            in_count
        );
    }

    #[test]
    fn test_rollout_boundary_cases() {
        let evaluator = Evaluator::new();

        // 0% = no one
        assert!(!evaluator.is_in_rollout("any-user", "flag", 0));

        // 100% = everyone
        assert!(evaluator.is_in_rollout("any-user", "flag", 100));
    }

    #[test]
    fn test_disabled_string_flag_is_not_enabled() {
        // Regression test: disabled string flags should return is_enabled() == false
        // even though the fallback value is a non-empty string.
        // This ensures kill switches work for all flag types.
        let evaluator = Evaluator::new();
        let flag = Flag::new_string(
            "ab-test",
            "A/B Test",
            vec!["variant-a".to_string(), "variant-b".to_string()],
            ProjectId::new(),
            UserId::new("creator"),
        )
        .with_environment("dev", EnvironmentConfig::enabled_string("variant-a"))
        .with_environment("prod", EnvironmentConfig::disabled());

        let context = EvaluationContext::with_user_id("user-1");

        // Enabled environment: should be enabled with the variant
        let dev_result = evaluator.evaluate(&flag, "dev", &context);
        assert!(dev_result.is_enabled());
        assert_eq!(dev_result.value.as_str(), Some("variant-a"));
        assert_eq!(dev_result.reason, EvaluationReason::Default);

        // Disabled environment: should NOT be enabled even with a non-empty fallback
        let prod_result = evaluator.evaluate(&flag, "prod", &context);
        assert!(!prod_result.is_enabled()); // This was the bug!
        assert_eq!(prod_result.reason, EvaluationReason::FlagDisabled);
        // Value is still returned for logging/debugging, but is_enabled is false
        assert_eq!(prod_result.value.as_bool(), None); // It's a string flag
    }
}
