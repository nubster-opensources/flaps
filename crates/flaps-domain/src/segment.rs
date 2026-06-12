//! Segments: reusable targeting predicates with recursive boolean composition.

use serde::{Deserialize, Serialize};

use crate::key::SegmentKey;

/// Comparison operator applied to a context attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchOperator {
    /// Attribute equals one of the values.
    Equals,
    /// Attribute does not equal any of the values.
    NotEquals,
    /// Attribute is contained in the value list.
    In,
    /// Attribute is not contained in the value list.
    NotIn,
    /// Attribute starts with the value.
    StartsWith,
    /// Attribute ends with the value.
    EndsWith,
    /// Attribute contains the value as a substring.
    Contains,
    /// SemVer equality.
    SemVerEq,
    /// SemVer inequality.
    SemVerNeq,
    /// SemVer strictly less than.
    SemVerLt,
    /// SemVer less than or equal.
    SemVerLte,
    /// SemVer strictly greater than.
    SemVerGt,
    /// SemVer greater than or equal.
    SemVerGte,
    /// SemVer caret range (compatible).
    SemVerCaret,
    /// SemVer tilde range (patch-level compatible).
    SemVerTilde,
}

/// A single attribute comparison against a list of reference values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Predicate {
    /// Name of the evaluation context attribute to test.
    pub attribute: String,
    /// Comparison operator.
    pub operator: MatchOperator,
    /// Reference values used by the operator.
    pub values: Vec<serde_json::Value>,
}

/// A recursive boolean expression over [`Predicate`]s.
///
/// Mirrors flagd's targeting rule structure so that the compiler can
/// translate a [`Segment`] into flagd JSON without loss.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentMatch {
    /// All sub-expressions must match (logical AND).
    And(Vec<SegmentMatch>),
    /// At least one sub-expression must match (logical OR).
    Or(Vec<SegmentMatch>),
    /// The sub-expression must not match (logical NOT).
    Not(Box<SegmentMatch>),
    /// A leaf predicate.
    Predicate(Predicate),
}

/// A named, reusable targeting segment.
///
/// Flags reference segments by [`SegmentKey`]; predicates live here, not
/// inline in flag rules. This keeps the flag model clean and segments
/// independently reusable across multiple flags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// Unique identifier within the project.
    pub key: SegmentKey,
    /// Human-readable display name.
    pub name: String,
    /// Boolean expression that determines membership.
    pub match_expr: SegmentMatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::SegmentKey;

    fn pred(attr: &str) -> SegmentMatch {
        SegmentMatch::Predicate(Predicate {
            attribute: attr.into(),
            operator: MatchOperator::Equals,
            values: vec![serde_json::json!("beta")],
        })
    }

    #[test]
    fn builds_recursive_and_or_not() {
        let expr = SegmentMatch::And(vec![
            SegmentMatch::Or(vec![pred("tier"), pred("plan")]),
            SegmentMatch::Not(Box::new(pred("blocked"))),
        ]);
        // Ensure the tree is well-formed (no panic on construction)
        assert!(matches!(expr, SegmentMatch::And(_)));
    }

    #[test]
    fn serde_round_trip_nested_expression() {
        let expr = SegmentMatch::And(vec![
            SegmentMatch::Or(vec![pred("tier"), pred("plan")]),
            SegmentMatch::Not(Box::new(pred("blocked"))),
        ]);
        let json = serde_json::to_string(&expr).unwrap();
        let back: SegmentMatch = serde_json::from_str(&json).unwrap();
        assert_eq!(back, expr);
    }

    #[test]
    fn segment_serde_round_trip() {
        let segment = Segment {
            key: SegmentKey::new("beta-users").unwrap(),
            name: "Beta users".into(),
            match_expr: pred("tier"),
        };
        let json = serde_json::to_string(&segment).unwrap();
        let back: Segment = serde_json::from_str(&json).unwrap();
        assert_eq!(back, segment);
    }

    #[test]
    fn all_operators_serialize() {
        let ops = [
            MatchOperator::Equals,
            MatchOperator::NotEquals,
            MatchOperator::In,
            MatchOperator::NotIn,
            MatchOperator::StartsWith,
            MatchOperator::EndsWith,
            MatchOperator::Contains,
            MatchOperator::SemVerEq,
            MatchOperator::SemVerNeq,
            MatchOperator::SemVerLt,
            MatchOperator::SemVerLte,
            MatchOperator::SemVerGt,
            MatchOperator::SemVerGte,
            MatchOperator::SemVerCaret,
            MatchOperator::SemVerTilde,
        ];
        for op in ops {
            let json = serde_json::to_string(&op).unwrap();
            let back: MatchOperator = serde_json::from_str(&json).unwrap();
            assert_eq!(back, op);
        }
    }
}
