//! # Flaps Core
//!
//! Core domain logic for Nubster Flaps - a European feature flags platform.
//!
//! This crate provides the fundamental types and evaluation engine for feature flags,
//! without any I/O dependencies. It can be used in both server and client contexts.
//!
//! ## Key Components
//!
//! - [`Flag`] - A feature flag with targeting rules
//! - [`Segment`] - Reusable user segments for targeting
//! - [`EvaluationContext`] - User context for flag evaluation
//! - [`Evaluator`] - The flag evaluation engine
//!
//! ## Example
//!
//! ```rust
//! use flaps_core::{Flag, EvaluationContext, Evaluator};
//!
//! let evaluator = Evaluator::new();
//! let context = EvaluationContext::with_user_id("user-123")
//!     .set("plan", "pro")
//!     .set("country", "FR");
//!
//! // let result = evaluator.evaluate(&flag, "prod", &context);
//! ```

pub mod context;
pub mod environment;
pub mod errors;
pub mod evaluation;
pub mod flag;
pub mod project;
pub mod rule;
pub mod segment;

// Re-exports for convenience
pub use context::EvaluationContext;
pub use environment::{Environment, EnvironmentConfig, EnvironmentId};
pub use errors::FlapsError;
pub use evaluation::{EvaluationReason, EvaluationResult, Evaluator};
pub use flag::{Flag, FlagId, FlagKey, FlagType, FlagValue};
pub use project::{Group, GroupId, Project, ProjectId, TenantId};
pub use rule::{AttributeValue, Condition, Operator, RuleId, TargetingRule};
pub use segment::{Segment, SegmentId, SegmentRule};

/// Result type for Flaps operations
pub type Result<T> = std::result::Result<T, FlapsError>;
