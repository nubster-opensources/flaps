//! OpenFeature in-process provider for Rust backed by Flaps.
//!
//! Synchronizes the compiled flagd ruleset over HTTP, listens for change
//! notifications over server-sent events and evaluates flags locally through
//! the flaps-eval engine. The provider lands with the v0.1.0 milestone.
