//! User segments for reusable targeting.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::flag::UserId;
use crate::project::ProjectId;
use crate::rule::{AttributeValue, Operator};

/// Unique identifier for a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SegmentId(pub Uuid);

impl SegmentId {
    /// Creates a new random segment ID.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Creates a segment ID from an existing UUID.
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

impl Default for SegmentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SegmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A reusable segment of users for targeting.
///
/// Segments define groups of users based on rules or explicit inclusion.
/// They can be referenced in targeting rules across multiple flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    /// Unique identifier.
    pub id: SegmentId,
    /// Machine-readable key (e.g., "beta-testers", "premium-users").
    pub key: String,
    /// Display name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Rules that define segment membership (OR logic between rules).
    pub rules: Vec<SegmentRule>,
    /// Explicitly included user IDs.
    pub included_users: Vec<String>,
    /// Explicitly excluded user IDs (takes precedence over rules and inclusions).
    pub excluded_users: Vec<String>,
    /// Project this segment belongs to.
    pub project_id: ProjectId,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
    /// User who created this segment.
    pub created_by: UserId,
}

impl Segment {
    /// Creates a new segment.
    pub fn new(
        key: impl Into<String>,
        name: impl Into<String>,
        project_id: ProjectId,
        created_by: UserId,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: SegmentId::new(),
            key: key.into(),
            name: name.into(),
            description: None,
            rules: Vec::new(),
            included_users: Vec::new(),
            excluded_users: Vec::new(),
            project_id,
            created_at: now,
            updated_at: now,
            created_by,
        }
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds a rule to this segment.
    pub fn with_rule(mut self, rule: SegmentRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Adds an included user.
    pub fn with_included_user(mut self, user_id: impl Into<String>) -> Self {
        self.included_users.push(user_id.into());
        self
    }

    /// Adds an excluded user.
    pub fn with_excluded_user(mut self, user_id: impl Into<String>) -> Self {
        self.excluded_users.push(user_id.into());
        self
    }

    /// Checks if a user is explicitly excluded.
    pub fn is_excluded(&self, user_id: &str) -> bool {
        self.excluded_users.iter().any(|id| id == user_id)
    }

    /// Checks if a user is explicitly included.
    pub fn is_included(&self, user_id: &str) -> bool {
        self.included_users.iter().any(|id| id == user_id)
    }
}

/// A rule that defines segment membership.
///
/// Multiple conditions within a rule use AND logic.
/// Multiple rules within a segment use OR logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentRule {
    /// Conditions that must ALL match for this rule (AND logic).
    pub conditions: Vec<SegmentCondition>,
}

impl SegmentRule {
    /// Creates a new empty segment rule.
    pub fn new() -> Self {
        Self {
            conditions: Vec::new(),
        }
    }

    /// Adds a condition to this rule.
    pub fn with_condition(mut self, condition: SegmentCondition) -> Self {
        self.conditions.push(condition);
        self
    }

    /// Creates a rule with a single condition.
    pub fn single(condition: SegmentCondition) -> Self {
        Self {
            conditions: vec![condition],
        }
    }
}

impl Default for SegmentRule {
    fn default() -> Self {
        Self::new()
    }
}

/// A condition for segment membership.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentCondition {
    /// Attribute to check.
    pub attribute: String,
    /// Comparison operator.
    pub operator: Operator,
    /// Value to compare against.
    pub value: AttributeValue,
}

impl SegmentCondition {
    /// Creates a new segment condition.
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

    /// Creates an "ends with" condition (useful for email domains).
    pub fn ends_with(attribute: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(attribute, Operator::EndsWith, AttributeValue::String(value.into()))
    }

    /// Creates an "in list" condition.
    pub fn in_list(attribute: impl Into<String>, values: Vec<String>) -> Self {
        Self::new(attribute, Operator::In, AttributeValue::StringList(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_segment() {
        let segment = Segment::new(
            "beta-testers",
            "Beta Testers",
            ProjectId::new(),
            UserId::new("user-1"),
        )
        .with_description("Users who opted into beta testing")
        .with_rule(
            SegmentRule::new()
                .with_condition(SegmentCondition::ends_with("email", "@nubster.com"))
        )
        .with_included_user("special-user-1")
        .with_excluded_user("banned-user-1");

        assert_eq!(segment.key, "beta-testers");
        assert_eq!(segment.rules.len(), 1);
        assert!(segment.is_included("special-user-1"));
        assert!(segment.is_excluded("banned-user-1"));
        assert!(!segment.is_included("random-user"));
    }

    #[test]
    fn test_segment_rule_conditions() {
        let rule = SegmentRule::new()
            .with_condition(SegmentCondition::equals("plan", "enterprise"))
            .with_condition(SegmentCondition::in_list(
                "country",
                vec!["FR".to_string(), "DE".to_string()],
            ));

        assert_eq!(rule.conditions.len(), 2);
    }
}
