//! Evaluation engine for flagd compatible rulesets.
//!
//! Evaluates serialized flagd JSON rulesets: JsonLogic targeting rules,
//! deterministic fractional rollouts, semantic version and string operators.
//! This crate intentionally does not depend on the Flaps domain model; the
//! public boundary is the flagd format itself.
//!
//! # Targeted specification
//!
//! This crate targets the flagd flag definition schema **v0** as published at
//! `https://flagd.dev/schema/v0/flags.json` and
//! `https://flagd.dev/schema/v0/targeting.json` (JSON Schema draft-07),
//! including the custom operations `fractional`, `sem_ver`, `starts_with`
//! and `ends_with`, and the reusable targeting rules declared under
//! `$evaluators` (resolved and inlined at parse time).
//!
//! Disabled flags follow the upstream semantics: evaluation succeeds with
//! reason `DISABLED` and carries no value or variant, so the caller serves
//! its own code default.

mod error;
mod eval;
mod fractional;
mod logic;
mod model;
mod parse;
mod semver;
mod serialize;
mod string_comparison;
mod targeting;

pub use error::ParseError;
pub use eval::{EvaluationContext, EvaluationError, Reason, Resolution};
pub use model::{Flag, FlagSet, Metadata, MetadataValue, State, Variants};
pub use targeting::{Bucket, Literal, Rule, SemVerOp};
