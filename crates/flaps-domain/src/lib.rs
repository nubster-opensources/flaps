//! Rich domain model for the Flaps feature flag platform.
//!
//! This crate defines the **editable source-of-truth** managed by the admin API
//! and consumed by the compiler. It is intentionally pure: no I/O, no async,
//! no dependency on the evaluation model (`flaps-eval`).
//!
//! # Structure
//!
//! | Module | Concepts |
//! |---|---|
//! | [`key`] | Validated kebab-case identifier newtypes |
//! | [`error`] | [`DomainError`] |
//! | [`federation`] | [`ExternalRef`], [`ManagedBy`] |
//! | [`project`] | [`Project`] |
//! | [`environment`] | [`Environment`] |
//! | [`flag`] | [`Flag`], [`FlagType`] |
//! | [`variant`] | [`ValueType`], [`VariantValue`], [`Variants`] |
//! | [`flag_env_config`] | [`FlagEnvConfig`], [`TargetingRule`], [`ServeTarget`], [`WeightedVariant`] |
//! | [`segment`] | [`Segment`], [`SegmentMatch`], [`Predicate`], [`MatchOperator`] |
//! | [`sdk_key`] | [`SdkKey`], [`SdkKeyKind`] |
//! | [`audit`] | [`AuditEntry`] |

pub mod audit;
pub mod environment;
pub mod error;
pub mod federation;
pub mod flag;
pub mod flag_env_config;
pub mod key;
pub mod project;
pub mod sdk_key;
pub mod segment;
pub mod variant;

// Convenience re-exports of the most frequently used types.
pub use audit::AuditEntry;
pub use environment::Environment;
pub use error::DomainError;
pub use federation::{ExternalRef, ManagedBy};
pub use flag::{Flag, FlagType};
pub use flag_env_config::{FlagEnvConfig, ServeTarget, TargetingRule, WeightedVariant};
pub use key::{EnvironmentKey, FlagKey, ProjectKey, SegmentKey, VariantKey};
pub use project::Project;
pub use sdk_key::{SdkKey, SdkKeyKind};
pub use segment::{MatchOperator, Predicate, Segment, SegmentMatch};
pub use variant::{ValueType, VariantValue, Variants};
