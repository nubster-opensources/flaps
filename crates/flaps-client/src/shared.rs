//! Shared state between the provider and its background supervisor task.

use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use flaps_eval::FlagSet;

use crate::status::SyncState;

/// State shared by [`super::provider::FlapsProvider`] and the background
/// supervisor task via an [`Arc`].
///
/// The ruleset is stored in an [`ArcSwap`] for lock-free reads on the
/// evaluation hot path. The [`SyncState`] is protected by a [`Mutex`] and
/// updated only after successful network syncs or snapshot loads.
pub(crate) struct ProviderShared {
    /// Current compiled ruleset; `None` until the first successful sync.
    pub(crate) ruleset: ArcSwap<Option<Arc<FlagSet>>>,
    /// Metadata about the last sync (version, ETag, timestamps).
    pub(crate) sync_state: Mutex<SyncState>,
}

impl ProviderShared {
    /// Creates a new [`ProviderShared`] with no ruleset loaded.
    pub(crate) fn new() -> Self {
        Self {
            ruleset: ArcSwap::new(Arc::new(None)),
            sync_state: Mutex::new(SyncState::default()),
        }
    }
}
