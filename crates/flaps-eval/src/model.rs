//! Serde model of the flagd flag definition format.

use std::collections::BTreeMap;

use crate::error::ParseError;
use crate::targeting::Rule;

/// A parsed flagd flag set: the root document consumed by evaluators.
///
/// Reusable targeting rules declared under `$evaluators` are resolved and
/// inlined during parsing; the in-memory model never contains references.
/// Round-tripping is canonical: serializing a parsed flag set produces an
/// equivalent document with references inlined and keys ordered.
#[derive(Debug, Clone, PartialEq)]
pub struct FlagSet {
    /// All flags of the set, keyed by flag key.
    pub flags: BTreeMap<String, Flag>,
    /// Flag set level metadata, merged into flag metadata at evaluation time
    /// with flag level entries taking priority.
    pub metadata: Metadata,
}

impl FlagSet {
    /// Parses a flagd JSON document into a flag set.
    ///
    /// Performs strict validation: unknown operators, malformed arguments,
    /// mixed variant types and unresolved or cyclic `$evaluators` references
    /// are rejected with a structured [`ParseError`].
    pub fn from_json(document: &str) -> Result<Self, ParseError> {
        let value: serde_json::Value = serde_json::from_str(document)?;
        crate::parse::flag_set(&value)
    }

    /// Serializes the flag set back to canonical flagd JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        crate::serialize::flag_set_value(self).to_string()
    }
}

/// A single feature flag definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    /// Whether the flag participates in evaluation.
    pub state: State,
    /// The named values this flag can resolve to, homogeneous in type.
    pub variants: Variants,
    /// Variant served when targeting is absent, returns `null` or is missing.
    ///
    /// When absent and no variant is resolved from targeting, providers
    /// revert to the caller supplied code default.
    pub default_variant: Option<String>,
    /// Optional targeting rule selecting a variant from the context.
    pub targeting: Option<Rule>,
    /// Flag level metadata.
    pub metadata: Metadata,
}

/// Operational state of a flag.
///
/// A disabled flag evaluates successfully with reason `DISABLED` and no
/// value or variant; the caller serves its own code default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The flag is evaluated normally.
    Enabled,
    /// The flag is short-circuited; no rule is evaluated.
    Disabled,
}

/// Variant maps, homogeneous in value type by construction.
///
/// The flagd schema requires all variant values of a flag to share one JSON
/// type. That invariant is enforced at parse time, so evaluation code can
/// rely on it without re-checking.
#[derive(Debug, Clone, PartialEq)]
pub enum Variants {
    /// Boolean variants, e.g. `{ "on": true, "off": false }`.
    Boolean(BTreeMap<String, bool>),
    /// String variants.
    String(BTreeMap<String, String>),
    /// Numeric variants.
    Number(BTreeMap<String, f64>),
    /// Structured JSON variants.
    Object(BTreeMap<String, serde_json::Map<String, serde_json::Value>>),
}

/// Metadata attached to a flag or flag set.
///
/// Keys are strings; values are restricted to booleans, strings and numbers
/// as mandated by the flagd schema.
pub type Metadata = BTreeMap<String, MetadataValue>;

/// A single metadata value.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataValue {
    /// A boolean metadata value.
    Bool(bool),
    /// A string metadata value.
    String(String),
    /// A numeric metadata value.
    Number(f64),
}
