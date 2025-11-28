//! # Flaps Storage
//!
//! Storage abstraction layer for Nubster Flaps.
//!
//! This crate provides traits and implementations for persisting
//! flags, segments, and related data.
//!
//! ## Backends
//!
//! - PostgreSQL (production)
//! - SQLite (development, on-prem single node)
//! - Redis (caching layer)

pub mod traits;

// Re-exports
pub use traits::*;
