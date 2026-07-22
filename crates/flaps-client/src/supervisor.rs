//! Background supervisor task: SSE connection + polling fallback.
//!
//! The task holds an `Arc<ProviderShared>` and keeps the ruleset fresh by:
//! 1. Opening `GET /sync/v1/events` (SSE stream).
//! 2. Fetching the ruleset immediately on (re)connect.
//! 3. Fetching again on each SSE notification.
//! 4. Falling back to a periodic poll every `poll_interval`, running
//!    unconditionally rather than only while SSE is connected. A client that
//!    cannot hold an SSE subscription (for instance because the server's
//!    concurrency quota permanently rejects it) must still degrade to
//!    polling instead of freezing on the ruleset last observed.
//! 5. Reconnecting with full-jitter backoff when the SSE stream drops or
//!    fails to open; the poll cadence runs independently of that backoff, so
//!    a slow reconnect schedule never delays a due poll.
//!
//! The task is stopped by `JoinHandle::abort()` from `Drop for FlapsProvider`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use tokio::task::JoinHandle;
use tokio::time::{Interval, MissedTickBehavior, interval};
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

    // The poll interval is created once, outside the connect/reconnect loop,
    // and ticked unconditionally: whether SSE is connected, disconnected, or
    // permanently rejected by the server has no bearing on it. Hoisting it
    // out of the `Ok` connect arm is what lets a quota-rejected client (see
    // issue #111) still observe ruleset changes.
    let mut poll_tick = poll_interval(config.poll_interval);
    // The first tick fires immediately; skip it to avoid a double fetch right
    // after the initial fetch above.
    poll_tick.tick().await;

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

        // Backoff before the next reconnect attempt. The poll fallback keeps
        // ticking on its own cadence while this wait elapses (`select!`
        // below), so a client stuck in this arm - for instance because the
        // server durably rejects its SSE subscription - still refreshes the
        // ruleset on schedule instead of freezing until reconnection
        // succeeds. The wait duration itself is untouched: the CONNECT
        // backoff and the poll cadence are driven independently.
        let delay = backoff.next_delay();
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => break,
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
}

/// Builds the polling-fallback interval used by [`run_supervisor`].
///
/// Sets [`MissedTickBehavior::Delay`], overriding `tokio::time::interval`'s
/// default of `Burst`. This interval is a periodic refresh fallback that
/// wants "fetch at most every `period`", never a catch-up burst: `Burst`
/// would fire every missed tick back-to-back once the supervisor is blocked
/// for longer than one `period` (a hung server holding a request up to
/// `request_timeout`, a long connect attempt), producing a burst of
/// sequential fetches instead of a single one. Unreachable at the shipped
/// default (`poll_interval` 300s vs `request_timeout` 10s), but reachable for
/// an operator who configures a poll interval shorter than the request
/// timeout.
fn poll_interval(period: Duration) -> Interval {
    let mut poll_tick = interval(period);
    poll_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    poll_tick
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

    let response = response.error_for_status()?;

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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::FutureExt;

    use super::poll_interval;

    /// Proves `poll_interval` sets `MissedTickBehavior::Delay`, not the
    /// `tokio::time::interval` default of `Burst`. After the supervisor is
    /// stalled for more than one period, `Burst` would make several `.tick()`
    /// calls resolve immediately in a row (a catch-up burst); `Delay` must
    /// make only the first one resolve immediately, and the following one
    /// must still be pending.
    #[tokio::test(start_paused = true)]
    async fn poll_interval_does_not_burst_after_a_long_stall() {
        let period = Duration::from_millis(100);
        let mut tick = poll_interval(period);

        // The first tick fires immediately regardless of missed-tick
        // behavior; consume it so the assertion below is about the SECOND
        // tick's readiness.
        tick.tick().await;

        // Simulate the supervisor being blocked for over three periods (a
        // hung request, a long connect attempt).
        tokio::time::advance(period * 3 + Duration::from_millis(10)).await;

        // One tick is due and ready immediately.
        tick.tick().now_or_never().expect(
            "at least one tick must be immediately ready after the simulated stall elapsed",
        );

        // With `Delay`, the tick after that is rescheduled `period` from the
        // one just consumed, so it must NOT also be immediately ready. With
        // the default `Burst`, this call would resolve `Ready` too,
        // producing the catch-up burst this test guards against.
        assert!(
            tick.tick().now_or_never().is_none(),
            "a second tick must not be immediately ready after a stall: \
             MissedTickBehavior::Delay must be set, not the default Burst"
        );
    }
}
