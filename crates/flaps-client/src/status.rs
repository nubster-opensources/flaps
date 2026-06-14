//! Provider freshness metrics exposed via [`FlapsProvider::status`].

use std::time::{Duration, Instant};

/// Internal mutable state updated after each successful sync.
#[derive(Debug, Default)]
pub(crate) struct SyncState {
    /// Version tag received from the server (`X-Flaps-Version` header).
    pub(crate) version: Option<u64>,
    /// Monotonic instant of the last successful sync.
    pub(crate) last_successful_sync: Option<Instant>,
}

/// Snapshot of provider freshness metrics.
///
/// Returned by [`FlapsProvider::status`] and computed from the internal
/// [`SyncState`] at call time. All fields are `None` until the first
/// successful sync completes.
#[derive(Debug, Clone)]
pub struct ProviderStatus {
    /// Version of the currently loaded ruleset, if known.
    pub version: Option<u64>,
    /// Monotonic instant of the last successful ruleset sync.
    pub last_successful_sync: Option<Instant>,
    /// Age of the currently loaded ruleset, computed at call time.
    pub ruleset_age: Option<Duration>,
}

impl ProviderStatus {
    /// Creates a [`ProviderStatus`] from the current [`SyncState`].
    #[must_use]
    pub(crate) fn from_state(state: &SyncState) -> Self {
        let last_successful_sync = state.last_successful_sync;
        let ruleset_age = last_successful_sync.map(|t| t.elapsed());
        Self {
            version: state.version,
            last_successful_sync,
            ruleset_age,
        }
    }
}
