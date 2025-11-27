//! Evaluation context for flag evaluation.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::rule::AttributeValue;

/// Context for evaluating feature flags.
///
/// The evaluation context contains information about the current user
/// and any custom attributes that can be used in targeting rules.
///
/// # Example
///
/// ```rust
/// use flaps_core::EvaluationContext;
///
/// let context = EvaluationContext::with_user_id("user-123")
///     .set("plan", "pro")
///     .set("country", "FR")
///     .set("beta_tester", true);
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvaluationContext {
    /// Unique identifier for the user (used for rollout percentage calculation).
    pub user_id: Option<String>,
    /// Custom attributes for targeting.
    #[serde(default)]
    pub attributes: HashMap<String, AttributeValue>,
}

impl EvaluationContext {
    /// Creates a new empty evaluation context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a context with a user ID.
    pub fn with_user_id(user_id: impl Into<String>) -> Self {
        Self {
            user_id: Some(user_id.into()),
            attributes: HashMap::new(),
        }
    }

    /// Sets the user ID.
    pub fn user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Sets an attribute value.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<AttributeValue>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Sets an attribute value (mutable reference version).
    pub fn set_mut(
        &mut self,
        key: impl Into<String>,
        value: impl Into<AttributeValue>,
    ) -> &mut Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    /// Gets an attribute value.
    pub fn get(&self, key: &str) -> Option<&AttributeValue> {
        self.attributes.get(key)
    }

    /// Gets an attribute as a string.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.attributes.get(key).and_then(|v| v.as_str())
    }

    /// Gets an attribute as a number.
    pub fn get_number(&self, key: &str) -> Option<f64> {
        self.attributes.get(key).and_then(|v| v.as_number())
    }

    /// Gets an attribute as a boolean.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.attributes.get(key).and_then(|v| v.as_bool())
    }

    /// Checks if an attribute exists.
    pub fn has(&self, key: &str) -> bool {
        self.attributes.contains_key(key)
    }

    /// Removes an attribute.
    pub fn remove(&mut self, key: &str) -> Option<AttributeValue> {
        self.attributes.remove(key)
    }

    /// Returns the effective user ID for rollout calculation.
    ///
    /// Falls back to a hash of attributes if no user ID is set.
    pub fn effective_user_id(&self) -> String {
        if let Some(ref user_id) = self.user_id {
            user_id.clone()
        } else {
            // Generate a stable ID from attributes
            let mut parts: Vec<String> = self
                .attributes
                .iter()
                .map(|(k, v)| format!("{}:{:?}", k, v))
                .collect();
            parts.sort();
            format!("anonymous:{}", parts.join(","))
        }
    }

    /// Merges another context into this one.
    ///
    /// Values from `other` take precedence.
    pub fn merge(mut self, other: EvaluationContext) -> Self {
        if other.user_id.is_some() {
            self.user_id = other.user_id;
        }
        for (key, value) in other.attributes {
            self.attributes.insert(key, value);
        }
        self
    }
}

/// Builder for creating evaluation contexts fluently.
pub struct ContextBuilder {
    context: EvaluationContext,
}

impl ContextBuilder {
    /// Creates a new context builder.
    pub fn new() -> Self {
        Self {
            context: EvaluationContext::new(),
        }
    }

    /// Sets the user ID.
    pub fn user_id(mut self, user_id: impl Into<String>) -> Self {
        self.context.user_id = Some(user_id.into());
        self
    }

    /// Sets the email attribute.
    pub fn email(self, email: impl Into<String>) -> Self {
        self.attribute("email", email.into())
    }

    /// Sets the plan attribute.
    pub fn plan(self, plan: impl Into<String>) -> Self {
        self.attribute("plan", plan.into())
    }

    /// Sets the country attribute.
    pub fn country(self, country: impl Into<String>) -> Self {
        self.attribute("country", country.into())
    }

    /// Sets a custom attribute.
    pub fn attribute(mut self, key: impl Into<String>, value: impl Into<AttributeValue>) -> Self {
        self.context.attributes.insert(key.into(), value.into());
        self
    }

    /// Builds the evaluation context.
    pub fn build(self) -> EvaluationContext {
        self.context
    }
}

impl Default for ContextBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_context() {
        let context = EvaluationContext::with_user_id("user-123")
            .set("plan", "pro")
            .set("country", "FR")
            .set("beta_tester", true);

        assert_eq!(context.user_id, Some("user-123".to_string()));
        assert_eq!(context.get_str("plan"), Some("pro"));
        assert_eq!(context.get_str("country"), Some("FR"));
        assert_eq!(context.get_bool("beta_tester"), Some(true));
    }

    #[test]
    fn test_context_builder() {
        let context = ContextBuilder::new()
            .user_id("user-456")
            .email("user@example.com")
            .plan("enterprise")
            .country("DE")
            .attribute("custom_field", 42.0)
            .build();

        assert_eq!(context.user_id, Some("user-456".to_string()));
        assert_eq!(context.get_str("email"), Some("user@example.com"));
        assert_eq!(context.get_number("custom_field"), Some(42.0));
    }

    #[test]
    fn test_effective_user_id() {
        let with_id = EvaluationContext::with_user_id("user-123");
        assert_eq!(with_id.effective_user_id(), "user-123");

        let without_id = EvaluationContext::new().set("session", "abc123");
        assert!(without_id.effective_user_id().starts_with("anonymous:"));
    }

    #[test]
    fn test_merge_contexts() {
        let base = EvaluationContext::with_user_id("user-1")
            .set("plan", "free")
            .set("country", "FR");

        let override_ctx = EvaluationContext::new()
            .set("plan", "pro")
            .set("new_attr", "value");

        let merged = base.merge(override_ctx);
        assert_eq!(merged.user_id, Some("user-1".to_string()));
        assert_eq!(merged.get_str("plan"), Some("pro")); // Overridden
        assert_eq!(merged.get_str("country"), Some("FR")); // Kept
        assert_eq!(merged.get_str("new_attr"), Some("value")); // Added
    }
}
