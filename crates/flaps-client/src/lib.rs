//! OpenFeature in-process provider for Rust backed by Flaps.
//!
//! Synchronizes the compiled flagd ruleset over HTTP and evaluates flags
//! locally through the flaps-eval engine. The provider guarantees that
//! evaluations never panic: any transient error or missing ruleset causes the
//! SDK to serve the caller-supplied default with reason `ERROR`.
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

mod coerce;
mod context_mapper;
mod reason_mapper;
mod sync;

pub mod provider;
pub mod status;

pub use provider::{FlapsProvider, FlapsProviderConfig};
pub use status::ProviderStatus;
