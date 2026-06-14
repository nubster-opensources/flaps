//! HTTP sync logic: fetches the compiled ruleset from the Flaps server.

use std::sync::Arc;

use tracing::warn;

use flaps_eval::FlagSet;

use crate::shared::ProviderShared;

/// Endpoint path for the ruleset sync.
const RULESET_PATH: &str = "/sync/v1/ruleset";

/// Header carrying the ruleset version.
const VERSION_HEADER: &str = "X-Flaps-Version";

/// Fetches the ruleset from `base_url` using `sdk_key` as Bearer token.
///
/// Sends `If-None-Match` with the stored ETag when available. On 304 the
/// ruleset is unchanged but `last_successful_sync` is refreshed. On 200 the
/// ruleset, version, and ETag are stored. On any other non-2xx, network, or
/// parse error the function logs a warning and leaves `shared` unchanged, so
/// callers continue to serve the last-known-good ruleset.
///
/// Returns `true` when a 200 or 304 was received (i.e. the server is reachable
/// and the key is valid), `false` on error.
pub(crate) async fn fetch_and_store(
    client: &reqwest::Client,
    base_url: &str,
    sdk_key: &str,
    shared: &Arc<ProviderShared>,
    snapshot_path: Option<&std::path::Path>,
) -> bool {
    let url = format!("{base_url}{RULESET_PATH}");

    // Retrieve the current ETag under lock (short critical section).
    let etag = {
        let state = shared
            .sync_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.etag.clone()
    };

    let mut request = client
        .get(&url)
        .header("Authorization", format!("Bearer {sdk_key}"));

    if let Some(ref tag) = etag {
        request = request.header("If-None-Match", tag.as_str());
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(err) => {
            warn!(error = %err, "ruleset sync request failed");
            return false;
        }
    };

    let status = response.status();

    // 304 Not Modified: ruleset unchanged, but refresh the sync timestamp.
    if status == reqwest::StatusCode::NOT_MODIFIED {
        let mut state = shared
            .sync_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.last_successful_sync = Some(std::time::Instant::now());
        state.loaded_from_snapshot = false;
        return true;
    }

    // Extract version before consuming the response.
    let version = response
        .headers()
        .get(VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    // Extract ETag before consuming the response.
    let new_etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);

    if !status.is_success() {
        warn!(%status, "ruleset sync returned non-2xx status");
        return false;
    }

    let body = match response.text().await {
        Ok(b) => b,
        Err(err) => {
            warn!(error = %err, "failed to read ruleset response body");
            return false;
        }
    };

    let flag_set = match FlagSet::from_json(&body) {
        Ok(fs) => fs,
        Err(err) => {
            warn!(error = %err, "failed to parse ruleset document");
            return false;
        }
    };

    shared.ruleset.store(Arc::new(Some(Arc::new(flag_set))));

    {
        let mut state = shared
            .sync_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.version = version;
        state.last_successful_sync = Some(std::time::Instant::now());
        state.etag = new_etag;
        state.loaded_from_snapshot = false;
    }

    // Write snapshot if configured.
    if let Some(path) = snapshot_path {
        crate::snapshot::write_snapshot(path, version, &body).await;
    }

    true
}
