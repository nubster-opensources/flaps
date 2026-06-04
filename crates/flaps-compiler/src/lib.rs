//! Compiler from the Flaps domain model to canonical flagd rulesets.
//!
//! Produces one versioned, content-hashed flagd ruleset per environment:
//! reusable segments are inlined and per-environment overrides are resolved
//! at compile time. The compiler lands with the v0.1.0 milestone.
