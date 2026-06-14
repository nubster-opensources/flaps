//! HTTP sync logic: fetches the compiled ruleset from the Flaps server.

use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::warn;

use flaps_eval::FlagSet;

use crate::status::SyncState;

/// Endpoint path for the ruleset sync.
const RULESET_PATH: &str = "/sync/v1/ruleset";

/// Header carrying the ruleset version.
const VERSION_HEADER: &str = "X-Flaps-Version";

/// Fetches the ruleset from `base_url` using `sdk_key` as Bearer token.
///
/// On success stores the parsed [`FlagSet`] in `ruleset` and updates
/// `sync_state`. On any network, HTTP, or parse error the function logs a
/// warning and leaves `ruleset` unchanged, so callers continue to serve the
/// last-known-good ruleset (or `None` on first call).
pub(crate) async fn fetch_and_store(
    client: &reqwest::Client,
    base_url: &str,
    sdk_key: &str,
    ruleset: &ArcSwap<Option<Arc<FlagSet>>>,
    sync_state: &std::sync::Mutex<SyncState>,
) {
    let url = format!("{base_url}{RULESET_PATH}");

    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {sdk_key}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(err) => {
            warn!(error = %err, "ruleset sync request failed");
            return;
        }
    };

    let version = response
        .headers()
        .get(VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    if !response.status().is_success() {
        warn!(status = %response.status(), "ruleset sync returned non-2xx status");
        return;
    }

    let body = match response.text().await {
        Ok(b) => b,
        Err(err) => {
            warn!(error = %err, "failed to read ruleset response body");
            return;
        }
    };

    let flag_set = match FlagSet::from_json(&body) {
        Ok(fs) => fs,
        Err(err) => {
            warn!(error = %err, "failed to parse ruleset document");
            return;
        }
    };

    ruleset.store(Arc::new(Some(Arc::new(flag_set))));

    if let Ok(mut state) = sync_state.lock() {
        state.version = version;
        state.last_successful_sync = Some(std::time::Instant::now());
    }
}
