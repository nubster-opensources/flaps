//! Maps flaps-eval [`Reason`] variants to OpenFeature [`EvaluationReason`].

use flaps_eval::Reason;
use open_feature::EvaluationReason;

/// Converts a flaps-eval resolution reason to an OpenFeature evaluation reason.
#[must_use]
pub(crate) fn map_reason(reason: Reason) -> EvaluationReason {
    match reason {
        Reason::Static => EvaluationReason::Static,
        Reason::TargetingMatch => EvaluationReason::TargetingMatch,
        Reason::Default => EvaluationReason::Default,
        Reason::Disabled => EvaluationReason::Disabled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_static() {
        assert_eq!(map_reason(Reason::Static), EvaluationReason::Static);
    }

    #[test]
    fn map_targeting_match() {
        assert_eq!(
            map_reason(Reason::TargetingMatch),
            EvaluationReason::TargetingMatch
        );
    }

    #[test]
    fn map_default() {
        assert_eq!(map_reason(Reason::Default), EvaluationReason::Default);
    }

    #[test]
    fn map_disabled() {
        assert_eq!(map_reason(Reason::Disabled), EvaluationReason::Disabled);
    }
}
