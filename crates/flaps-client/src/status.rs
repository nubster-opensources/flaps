//! Provider freshness metrics exposed via [`FlapsProvider::status`].

use std::time::{Duration, Instant};

/// Internal mutable state updated after each successful sync.
#[derive(Debug, Default)]
pub(crate) struct SyncState {
    /// Version tag received from the server (`X-Flaps-Version` header).
    pub(crate) version: Option<u64>,
    /// Monotonic instant of the last successful sync.
    pub(crate) last_successful_sync: Option<Instant>,
    /// ETag value (quoted) received from the server, sent as `If-None-Match` on
    /// subsequent requests.
    pub(crate) etag: Option<String>,
    /// `true` when the ruleset was loaded from a disk snapshot and has not yet
    /// been confirmed by a successful network sync this session.
    pub(crate) loaded_from_snapshot: bool,
}

/// Snapshot of provider freshness metrics.
///
/// Returned by [`FlapsProvider::status`] and computed from the internal
/// [`SyncState`] at call time. All fields are `None` until the first
/// successful sync completes.
#[derive(Debug, Clone)]
pub struct SyncStatus {
    /// Version of the currently loaded ruleset, if known.
    pub version: Option<u64>,
    /// Monotonic instant of the last successful ruleset sync.
    pub last_successful_sync: Option<Instant>,
    /// Age of the currently loaded ruleset, computed at call time.
    pub ruleset_age: Option<Duration>,
}

impl SyncStatus {
    /// Creates a [`SyncStatus`] from the current [`SyncState`].
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
