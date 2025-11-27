//! # Flaps SDK
//!
//! Rust SDK for integrating Nubster Flaps feature flags into your application.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use flaps_sdk::{FlapsClient, Config};
//!
//! #[tokio::main]
//! async fn main() {
//!     let client = FlapsClient::new(Config {
//!         api_key: "your-api-key".to_string(),
//!         environment: "prod".to_string(),
//!         ..Default::default()
//!     }).await.unwrap();
//!
//!     let context = client.context()
//!         .user_id("user-123")
//!         .set("plan", "pro");
//!
//!     if client.is_enabled("new-checkout", &context) {
//!         // New checkout flow
//!     } else {
//!         // Old checkout flow
//!     }
//! }
//! ```

mod client;
mod config;

pub use client::FlapsClient;
pub use config::Config;
pub use flaps_core::{EvaluationContext, EvaluationResult, FlagValue};
