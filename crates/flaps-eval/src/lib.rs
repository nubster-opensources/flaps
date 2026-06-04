//! Evaluation engine for flagd compatible rulesets.
//!
//! Evaluates serialized flagd JSON rulesets: JsonLogic targeting rules,
//! deterministic fractional rollouts, semantic version and string operators.
//! This crate intentionally does not depend on the Flaps domain model; the
//! public boundary is the flagd format itself. The engine lands with the
//! v0.1.0 milestone.
