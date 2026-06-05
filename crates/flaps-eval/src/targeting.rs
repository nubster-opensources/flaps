//! Typed AST of flagd targeting rules.
//!
//! Targeting rules are JsonLogic extended with the flagd custom operations.
//! The AST covers exactly the operators admitted by the upstream targeting
//! schema; anything else is rejected at parse time with a structured error.

use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

/// A single targeting rule node.
///
/// A rule evaluates against an evaluation context and resolves to a JSON
/// value. When used as the `targeting` of a flag, the resolved value selects
/// a variant: strings index the variants map, booleans are coerced to the
/// `"true"` and `"false"` keys, and `null` exits to the default variant.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// A literal JSON value.
    Literal(Literal),

    /// An array of expressions, evaluated element by element.
    ///
    /// JsonLogic evaluates arrays recursively, so `["a", {"var": "x"}]` is a
    /// valid expression producing a two element array.
    Array(Vec<Rule>),

    /// `var`: reads an attribute from the evaluation context.
    ///
    /// Supports dotted paths (`"user.email"`) and the flagd injected
    /// attributes `$flagd.flagKey` and `$flagd.timestamp`. The optional
    /// second argument is returned when the attribute is absent.
    Var {
        /// Context attribute path; an empty path yields the whole context.
        path: String,
        /// Fallback value when the attribute is absent.
        default: Option<Literal>,
    },

    /// `missing`: returns the listed context keys that are absent.
    Missing(Vec<Rule>),

    /// `missing_some`: requires at least `min` of `keys` to be present.
    MissingSome {
        /// Minimum number of keys that must be present.
        min: u64,
        /// Context keys to probe.
        keys: Vec<Rule>,
    },

    /// `if`: condition and outcome pairs followed by an optional else.
    If(Vec<Rule>),

    /// `and`: logical conjunction, returns the first falsy operand.
    And(Vec<Rule>),

    /// `or`: logical disjunction, returns the first truthy operand.
    Or(Vec<Rule>),

    /// `!`: logical negation.
    Not(Box<Rule>),

    /// `!!`: cast to boolean.
    Truthy(Box<Rule>),

    /// `==`: equality with type coercion.
    Eq(Box<Rule>, Box<Rule>),

    /// `===`: strict equality without coercion.
    StrictEq(Box<Rule>, Box<Rule>),

    /// `!=`: inequality with type coercion.
    Neq(Box<Rule>, Box<Rule>),

    /// `!==`: strict inequality without coercion.
    StrictNeq(Box<Rule>, Box<Rule>),

    /// `>`: numeric greater-than.
    Gt(Box<Rule>, Box<Rule>),

    /// `>=`: numeric greater-than-or-equal.
    Gte(Box<Rule>, Box<Rule>),

    /// `<`: numeric less-than; the ternary form tests betweenness.
    Lt(Vec<Rule>),

    /// `<=`: numeric less-than-or-equal; the ternary form tests betweenness.
    Lte(Vec<Rule>),

    /// `+`: numeric addition; unary form casts to a number.
    Add(Vec<Rule>),

    /// `-`: numeric subtraction; unary form negates.
    Sub(Vec<Rule>),

    /// `*`: numeric multiplication.
    Mul(Vec<Rule>),

    /// `/`: numeric division.
    Div(Box<Rule>, Box<Rule>),

    /// `%`: numeric modulo.
    Mod(Box<Rule>, Box<Rule>),

    /// `min`: smallest operand.
    Min(Vec<Rule>),

    /// `max`: largest operand.
    Max(Vec<Rule>),

    /// `cat`: string concatenation.
    Cat(Vec<Rule>),

    /// `substr`: substring by start position and optional length.
    Substr(Vec<Rule>),

    /// `in`: membership in a string or an array.
    In(Box<Rule>, Box<Rule>),

    /// `merge`: flattens arrays into a single array.
    Merge(Vec<Rule>),

    /// `map`: applies a rule to each element of an array.
    Map(Box<Rule>, Box<Rule>),

    /// `filter`: keeps the elements for which the rule is truthy.
    Filter(Box<Rule>, Box<Rule>),

    /// `reduce`: folds an array with an accumulator.
    Reduce(Box<Rule>, Box<Rule>, Box<Rule>),

    /// `all`: the rule holds for every element.
    All(Box<Rule>, Box<Rule>),

    /// `none`: the rule holds for no element.
    None(Box<Rule>, Box<Rule>),

    /// `some`: the rule holds for at least one element.
    Some(Box<Rule>, Box<Rule>),

    /// `starts_with`: the string attribute starts with the given prefix.
    StartsWith(Box<Rule>, Box<Rule>),

    /// `ends_with`: the string attribute ends with the given suffix.
    EndsWith(Box<Rule>, Box<Rule>),

    /// `sem_ver`: semantic version comparison, e.g. `["1.1.2", ">=", "1.0.0"]`.
    SemVer {
        /// The version under test, usually read from the context.
        value: Box<Rule>,
        /// The comparison to apply.
        op: SemVerOp,
        /// The version to compare against.
        version: Box<Rule>,
    },

    /// `fractional`: deterministic weighted variant distribution.
    ///
    /// The bucketing value is hashed with murmur3 and mapped onto the weight
    /// distribution using pure integer arithmetic, matching the flagd
    /// reference implementation so cross-language evaluation agrees.
    Fractional {
        /// Expression producing the bucketing value. When absent, flagd
        /// concatenates the flag key and the targeting key.
        bucket_by: Option<Box<Rule>>,
        /// Weighted variant buckets; weights are relative.
        buckets: Vec<Bucket>,
    },

    /// `$ref`: reference to a reusable rule declared under `$evaluators`.
    ///
    /// References are resolved and inlined by [`FlagSet::from_json`]; this
    /// variant never appears in a successfully parsed flag set and evaluates
    /// to an error elsewhere.
    ///
    /// [`FlagSet::from_json`]: crate::model::FlagSet::from_json
    Ref(String),
}

impl<'de> Deserialize<'de> for Rule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        crate::parse::rule("$", &value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Rule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let _ = serializer;
        todo!()
    }
}

/// A literal JSON value embedded in a rule.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// The JSON `null` value.
    Null,
    /// A JSON boolean.
    Bool(bool),
    /// A JSON number.
    Number(f64),
    /// A JSON string.
    String(String),
}

/// A semantic version comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemVerOp {
    /// `=`: exact match.
    Eq,
    /// `!=`: mismatch.
    Neq,
    /// `<`: strictly lower.
    Lt,
    /// `<=`: lower or equal.
    Lte,
    /// `>`: strictly greater.
    Gt,
    /// `>=`: greater or equal.
    Gte,
    /// `^`: same major version.
    CaretMatch,
    /// `~`: same major and minor version.
    TildeMatch,
}

/// One weighted bucket of a `fractional` distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    /// Variant served to targeting keys landing in this bucket.
    pub variant: String,
    /// Relative weight of the bucket; defaults to 1 when omitted.
    pub weight: u32,
}
