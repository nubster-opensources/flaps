//! Background supervisor task: SSE connection + polling fallback.
//!
//! The task holds an `Arc<ProviderShared>` and keeps the ruleset fresh by:
//! 1. Opening `GET /sync/v1/events` (SSE stream).
//! 2. Fetching the ruleset immediately on (re)connect.
//! 3. Fetching again on each SSE notification.
//! 4. Falling back to a periodic poll every `poll_interval` when SSE is connected.
//! 5. Reconnecting with full-jitter backoff when the SSE stream drops.
//!
//! The task is stopped by `JoinHandle::abort()` from `Drop for FlapsProvider`.

use std::sync::Arc;

use bytes::Bytes;
use futures_util::StreamExt;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::warn;

use crate::backoff::Backoff;
use crate::provider::FlapsProviderConfig;
use crate::shared::ProviderShared;
use crate::sse::SseDecoder;
use crate::sync::fetch_and_store;

/// SSE endpoint path.
const EVENTS_PATH: &str = "/sync/v1/events";

/// Spawns the supervisor task and returns its [`JoinHandle`].
pub(crate) fn spawn_supervisor(
    client: reqwest::Client,
    config: FlapsProviderConfig,
    shared: Arc<ProviderShared>,
) -> JoinHandle<()> {
    tokio::spawn(run_supervisor(client, config, shared))
}

/// Runs the supervisor loop until aborted.
async fn run_supervisor(
    client: reqwest::Client,
    config: FlapsProviderConfig,
    shared: Arc<ProviderShared>,
) {
    let snapshot_path = config.snapshot_path.as_deref();
    let mut backoff = Backoff::new(config.backoff_base, config.backoff_max);

    // Initial fetch before opening SSE: ensures the ruleset is available even
    // when the SSE endpoint is unavailable. This also runs after each backoff
    // reconnect attempt so the ruleset stays fresh regardless of SSE health.
    fetch_and_store(
        &client,
        &config.base_url,
        &config.sdk_key,
        &shared,
        snapshot_path,
    )
    .await;

    loop {
        // Attempt to open the SSE stream.
        match open_event_stream(&client, &config.base_url, &config.sdk_key).await {
            Ok(mut stream) => {
                backoff.reset();

                // Fetch again immediately after (re)connecting to catch any events
                // that arrived during the disconnected window.
                fetch_and_store(
                    &client,
                    &config.base_url,
                    &config.sdk_key,
                    &shared,
                    snapshot_path,
                )
                .await;

                let mut poll_tick = interval(config.poll_interval);
                // The first tick fires immediately; skip it to avoid a double fetch.
                poll_tick.tick().await;

                let mut decoder = SseDecoder::new();

                loop {
                    tokio::select! {
                        chunk = stream.next() => {
                            match chunk {
                                Some(Ok(bytes)) => {
                                    let notifs = decoder.push(&bytes);
                                    if !notifs.is_empty() {
                                        // One fetch per batch of notifications.
                                        fetch_and_store(
                                            &client,
                                            &config.base_url,
                                            &config.sdk_key,
                                            &shared,
                                            snapshot_path,
                                        )
                                        .await;
                                    }
                                }
                                Some(Err(err)) => {
                                    warn!(error = %err, "SSE stream error; reconnecting");
                                    break;
                                }
                                None => {
                                    // Stream ended cleanly; reconnect.
                                    break;
                                }
                            }
                        }
                        _ = poll_tick.tick() => {
                            fetch_and_store(
                                &client,
                                &config.base_url,
                                &config.sdk_key,
                                &shared,
                                snapshot_path,
                            )
                            .await;
                        }
                    }
                }
            }
            Err(err) => {
                warn!(error = %err, "failed to open SSE stream; backing off");
            }
        }

        // Backoff before next reconnect attempt.
        let delay = backoff.next_delay();
        tokio::time::sleep(delay).await;
    }
}

/// Opens `GET /sync/v1/events` and returns the raw byte stream.
async fn open_event_stream(
    client: &reqwest::Client,
    base_url: &str,
    sdk_key: &str,
) -> Result<impl futures_util::Stream<Item = Result<Bytes, reqwest::Error>>, reqwest::Error> {
    let url = format!("{base_url}{EVENTS_PATH}");
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {sdk_key}"))
        .header("Accept", "text/event-stream")
        .send()
        .await?;

    if !response.status().is_success() {
        // Convert a non-2xx into an error by materialising a response error.
        // We return a "status" error by forcing a text decode on an error status.
        let status = response.status();
        warn!(%status, "SSE endpoint returned non-2xx");
        return Err(
            // reqwest has no public constructor for status errors; simulate by
            // sending a second request that we know will fail, OR use a simpler
            // approach: return a custom error via a failed `.error_for_status()`.
            response.error_for_status().unwrap_err(),
        );
    }

    Ok(response.bytes_stream())
}

/// Drives a single SSE notification through `fetch_and_store`.
///
/// Exposed for direct unit-testing without a live SSE HTTP connection.
#[allow(dead_code)]
pub(crate) async fn on_notification(
    client: &reqwest::Client,
    base_url: &str,
    sdk_key: &str,
    shared: &Arc<ProviderShared>,
    snapshot_path: Option<&std::path::Path>,
) {
    fetch_and_store(client, base_url, sdk_key, shared, snapshot_path).await;
}
