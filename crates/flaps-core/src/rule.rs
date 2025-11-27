//! Targeting rules and conditions.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::flag::FlagValue;
use crate::segment::SegmentId;

/// Unique identifier for a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub Uuid);

impl RuleId {
    /// Creates a new random rule ID.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for RuleId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A targeting rule that determines flag values for matching users.
///
/// Rules are evaluated in priority order. The first matching rule
/// determines the flag value for the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetingRule {
    /// Unique identifier.
    pub id: RuleId,
    /// Priority (lower = higher priority, evaluated first).
    pub priority: u32,
    /// Conditions that must ALL match (AND logic).
    pub conditions: Vec<Condition>,
    /// Value to return when this rule matches.
    pub value: FlagValue,
    /// Optional rollout percentage for this rule (0-100).
    pub rollout_percentage: Option<u8>,
    /// Optional description for documentation.
    pub description: Option<String>,
}

impl TargetingRule {
    /// Creates a new targeting rule.
    pub fn new(priority: u32, value: FlagValue) -> Self {
        Self {
            id: RuleId::new(),
            priority,
            conditions: Vec::new(),
            value,
            rollout_percentage: None,
            description: None,
        }
    }

    /// Adds a condition to this rule.
    pub fn with_condition(mut self, condition: Condition) -> Self {
        self.conditions.push(condition);
        self
    }

    /// Sets the rollout percentage.
    pub fn with_rollout(mut self, percentage: u8) -> Self {
        self.rollout_percentage = Some(percentage.min(100));
        self
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Returns true if this rule has no conditions (matches everyone).
    pub fn is_catch_all(&self) -> bool {
        self.conditions.is_empty()
    }
}

/// A condition that must be satisfied for a rule to match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// Attribute to check (e.g., "email", "plan", "country").
    pub attribute: String,
    /// Comparison operator.
    pub operator: Operator,
    /// Value to compare against.
    pub value: AttributeValue,
}

impl Condition {
    /// Creates a new condition.
    pub fn new(
        attribute: impl Into<String>,
        operator: Operator,
        value: impl Into<AttributeValue>,
    ) -> Self {
        Self {
            attribute: attribute.into(),
            operator,
            value: value.into(),
        }
    }

    /// Creates an equals condition.
    pub fn equals(attribute: impl Into<String>, value: impl Into<AttributeValue>) -> Self {
        Self::new(attribute, Operator::Equals, value)
    }

    /// Creates a "not equals" condition.
    pub fn not_equals(attribute: impl Into<String>, value: impl Into<AttributeValue>) -> Self {
        Self::new(attribute, Operator::NotEquals, value)
    }

    /// Creates a "contains" condition.
    pub fn contains(attribute: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            attribute,
            Operator::Contains,
            AttributeValue::String(value.into()),
        )
    }

    /// Creates a "starts with" condition.
    pub fn starts_with(attribute: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            attribute,
            Operator::StartsWith,
            AttributeValue::String(value.into()),
        )
    }

    /// Creates an "ends with" condition.
    pub fn ends_with(attribute: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            attribute,
            Operator::EndsWith,
            AttributeValue::String(value.into()),
        )
    }

    /// Creates an "in list" condition.
    pub fn in_list(attribute: impl Into<String>, values: Vec<String>) -> Self {
        Self::new(attribute, Operator::In, AttributeValue::StringList(values))
    }

    /// Creates a "matches segment" condition.
    pub fn matches_segment(segment_id: SegmentId) -> Self {
        Self::new(
            "",
            Operator::MatchesSegment,
            AttributeValue::SegmentRef(segment_id),
        )
    }
}

/// Comparison operators for conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    /// Exact equality.
    Equals,
    /// Not equal.
    NotEquals,
    /// String contains substring.
    Contains,
    /// String starts with prefix.
    StartsWith,
    /// String ends with suffix.
    EndsWith,
    /// Value is in a list.
    In,
    /// Value is not in a list.
    NotIn,
    /// Numeric greater than.
    GreaterThan,
    /// Numeric greater than or equal.
    GreaterThanOrEqual,
    /// Numeric less than.
    LessThan,
    /// Numeric less than or equal.
    LessThanOrEqual,
    /// Semantic version greater than.
    SemverGreaterThan,
    /// Semantic version less than.
    SemverLessThan,
    /// Matches a segment.
    MatchesSegment,
    /// Does not match a segment.
    NotMatchesSegment,
    /// Regular expression match.
    Regex,
}

/// Value used in conditions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AttributeValue {
    String(String),
    Number(f64),
    Boolean(bool),
    StringList(Vec<String>),
    SegmentRef(SegmentId),
}

impl AttributeValue {
    /// Returns the string value if applicable.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            AttributeValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the number value if applicable.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            AttributeValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Returns the boolean value if applicable.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            AttributeValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the string list if applicable.
    pub fn as_string_list(&self) -> Option<&[String]> {
        match self {
            AttributeValue::StringList(list) => Some(list),
            _ => None,
        }
    }

    /// Returns the segment reference if applicable.
    pub fn as_segment_ref(&self) -> Option<SegmentId> {
        match self {
            AttributeValue::SegmentRef(id) => Some(*id),
            _ => None,
        }
    }
}

impl From<String> for AttributeValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for AttributeValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<f64> for AttributeValue {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

impl From<i32> for AttributeValue {
    fn from(value: i32) -> Self {
        Self::Number(value as f64)
    }
}

impl From<i64> for AttributeValue {
    fn from(value: i64) -> Self {
        Self::Number(value as f64)
    }
}

impl From<bool> for AttributeValue {
    fn from(value: bool) -> Self {
        Self::Boolean(value)
    }
}

impl From<Vec<String>> for AttributeValue {
    fn from(value: Vec<String>) -> Self {
        Self::StringList(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_rule_with_conditions() {
        let rule = TargetingRule::new(1, FlagValue::Boolean(true))
            .with_condition(Condition::equals("plan", "pro"))
            .with_condition(Condition::in_list(
                "country",
                vec!["FR".to_string(), "BE".to_string()],
            ))
            .with_rollout(50);

        assert_eq!(rule.priority, 1);
        assert_eq!(rule.conditions.len(), 2);
        assert_eq!(rule.rollout_percentage, Some(50));
    }

    #[test]
    fn test_condition_builders() {
        let cond = Condition::ends_with("email", "@nubster.com");
        assert_eq!(cond.attribute, "email");
        assert_eq!(cond.operator, Operator::EndsWith);

        let cond = Condition::in_list("role", vec!["admin".to_string(), "moderator".to_string()]);
        assert_eq!(cond.operator, Operator::In);
    }

    #[test]
    fn test_attribute_value_conversions() {
        let str_val: AttributeValue = "test".into();
        assert_eq!(str_val.as_str(), Some("test"));

        let num_val: AttributeValue = 42.0.into();
        assert_eq!(num_val.as_number(), Some(42.0));

        let bool_val: AttributeValue = true.into();
        assert_eq!(bool_val.as_bool(), Some(true));
    }
}
