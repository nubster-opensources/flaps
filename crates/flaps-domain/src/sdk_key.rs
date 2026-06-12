//! SDK key: an opaque credential that grants access to flag evaluation.

use serde::{Deserialize, Serialize};

/// Distinguishes server-side from client-side SDK keys.
///
/// Server keys carry full flag data; client keys receive a filtered,
/// client-safe subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdkKeyKind {
    /// For server-side runtimes with full flag access.
    Server,
    /// For client-side runtimes with filtered flag access.
    Client,
}

/// An opaque SDK credential.
///
/// The domain carries the key value; it does not generate or hash secrets.
/// Secret generation is the responsibility of the application layer.
///
/// The `Debug` implementation redacts the secret value to prevent accidental
/// exposure in logs or panic messages.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkKey {
    /// The raw key value (treated as opaque by the domain).
    pub value: String,
    /// Whether this key is intended for a server or client SDK.
    pub kind: SdkKeyKind,
}

impl std::fmt::Debug for SdkKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SdkKey")
            .field("kind", &self.kind)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_server_and_client() {
        let server = SdkKey {
            value: "s-abc123".into(),
            kind: SdkKeyKind::Server,
        };
        let client = SdkKey {
            value: "c-xyz789".into(),
            kind: SdkKeyKind::Client,
        };
        assert_eq!(server.kind, SdkKeyKind::Server);
        assert_eq!(client.kind, SdkKeyKind::Client);
    }

    #[test]
    fn serde_round_trip_server() {
        let key = SdkKey {
            value: "s-abc123".into(),
            kind: SdkKeyKind::Server,
        };
        let json = serde_json::to_string(&key).unwrap();
        let back: SdkKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn serde_round_trip_client() {
        let key = SdkKey {
            value: "c-xyz789".into(),
            kind: SdkKeyKind::Client,
        };
        let json = serde_json::to_string(&key).unwrap();
        let back: SdkKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, key);
    }

    #[test]
    fn kind_serializes_to_snake_case() {
        let json = serde_json::to_string(&SdkKeyKind::Server).unwrap();
        assert_eq!(json, r#""server""#);
        let json = serde_json::to_string(&SdkKeyKind::Client).unwrap();
        assert_eq!(json, r#""client""#);
    }

    #[test]
    fn debug_redacts_value_and_shows_kind() {
        let key = SdkKey {
            value: "s-abc123".into(),
            kind: SdkKeyKind::Server,
        };
        let debug = format!("{key:?}");
        assert!(
            !debug.contains("s-abc123"),
            "debug must not expose the secret value"
        );
        assert!(debug.contains("redacted"), "debug must mention redacted");
        assert!(debug.contains("Server"), "debug must show the kind");
    }
}
