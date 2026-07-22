//! OpenFeature in-process provider for Rust backed by Flaps.
//!
//! Synchronizes the compiled flagd ruleset over HTTP and SSE, evaluates flags
//! locally through the flaps-eval engine, and survives server outages by
//! serving the last-known-good ruleset. An optional disk snapshot enables
//! warm-start even when the server is unreachable at startup.
//!
//! # Quick start
//!
//! ```no_run
//! use flaps_client::{FlapsProvider, FlapsProviderConfig};
//!
//! let config = FlapsProviderConfig::new("https://flaps.internal", "sdk-key-here");
//! let provider = FlapsProvider::new(config);
//! // Register with the OpenFeature API:
//! // open_feature::OpenFeature::singleton().set_provider(provider).await
//! ```

mod backoff;
mod coerce;
mod context_mapper;
mod metadata_mapper;
mod reason_mapper;
mod shared;
mod snapshot;
mod sse;
mod supervisor;
mod sync;

pub mod provider;
pub mod status;

pub use provider::{FlapsProvider, FlapsProviderConfig};
pub use status::SyncStatus;
