//! Audit trail entry: an immutable record of a domain-level action.

use serde::{Deserialize, Serialize};

/// A single immutable audit record.
///
/// `occurred_at` is caller-supplied (ISO-8601 string recommended). The domain
/// does not read the system clock; it is pure and has no I/O.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Principal who performed the action (user ID, service account, etc.).
    pub actor: String,
    /// Machine-readable action identifier (e.g. `"flag.created"`).
    pub action: String,
    /// Identifier of the resource that was acted upon.
    pub target: String,
    /// When the action occurred, as a caller-supplied string.
    pub occurred_at: String,
    /// Optional structured details about the action.
    pub details: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry() -> AuditEntry {
        AuditEntry {
            actor: "user:alice".into(),
            action: "flag.created".into(),
            target: "flag:my-flag".into(),
            occurred_at: "2026-06-12T10:00:00Z".into(),
            details: Some(serde_json::json!({"env": "production"})),
        }
    }

    #[test]
    fn serde_round_trip_with_details() {
        let entry = make_entry();
        let json = serde_json::to_string(&entry).unwrap();
        let back: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn details_optional_none() {
        let entry = AuditEntry {
            actor: "svc:deployer".into(),
            action: "env.toggled".into(),
            target: "env:production".into(),
            occurred_at: "2026-06-12T11:00:00Z".into(),
            details: None,
        };
        assert!(entry.details.is_none());
        let json = serde_json::to_string(&entry).unwrap();
        let back: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn all_fields_present_in_json() {
        let entry = make_entry();
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("actor"));
        assert!(json.contains("action"));
        assert!(json.contains("target"));
        assert!(json.contains("occurred_at"));
    }
}
