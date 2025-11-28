//! # Flaps Server
//!
//! HTTP API server for Nubster Flaps.

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Flaps server...");

    // TODO: Initialize storage
    // TODO: Initialize routes
    // TODO: Start server

    tracing::info!("Flaps server started on http://0.0.0.0:8080");

    // Keep the server running
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl-c");

    tracing::info!("Shutting down...");
}
