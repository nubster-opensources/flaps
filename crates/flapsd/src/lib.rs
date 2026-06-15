//! Internal library for the `flapsd` daemon.
//!
//! Exposes the boot primitives (`config`, `bootstrap`) as testable units.
//! The `main` binary wires them together and delegates all orchestration here.

pub mod bootstrap;
pub mod config;
