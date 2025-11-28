//! Workspace API client integration.
//!
//! This module provides the client for interacting with the Nubster Workspace API.
//! Projects, tenants, and groups are managed by Workspace, not stored locally in Flaps.

mod client;

pub use client::HttpWorkspaceClient;
