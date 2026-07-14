//! Minimal crates.io client and index parsing.

use std::time::{Duration, Instant};

use anyhow::Context as _;

/// crates.io JSON API host.
const API_HOST: &str = "https://crates.io";
/// crates.io sparse-index host.
const INDEX_HOST: &str = "https://index.crates.io";
/// User-Agent required by the crates.io API (requests without one are rejected).
const USER_AGENT: &str = "flaps-xtask (https://nubster.com)";

/// Sparse-index path for `name` following the crates.io directory layout.
pub(crate) fn index_path(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.len() {
        1 => format!("1/{lower}"),
        2 => format!("2/{lower}"),
        3 => format!("3/{}/{lower}", &lower[..1]),
        _ => format!("{}/{}/{lower}", &lower[..2], &lower[2..4]),
    }
}

/// Returns true if the sparse-index body already lists `version`.
pub(crate) fn index_has_version(index_body: &str, version: &str) -> bool {
    index_body.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|v| {
                v.get("vers")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .is_some_and(|vers| vers == version)
    })
}

/// Returns true if publish output indicates a crates.io rate limit.
pub(crate) fn is_rate_limit_error(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("429") || lower.contains("too many")
}

/// Delay before the next publish attempt: honour `retry_after`, else exponential (cap 300s).
pub(crate) fn backoff_delay(attempt: u32, retry_after: Option<u64>) -> Duration {
    match retry_after {
        Some(secs) => Duration::from_secs(secs),
        None => Duration::from_secs(2u64.saturating_pow(attempt).min(300)),
    }
}

/// Builds a blocking HTTP client with the required User-Agent.
pub(crate) fn client() -> anyhow::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build the crates.io HTTP client")
}

/// Returns true if `name` `version` is already on crates.io.
pub(crate) fn is_published(
    client: &reqwest::blocking::Client,
    name: &str,
    version: &str,
) -> anyhow::Result<bool> {
    let url = format!("{API_HOST}/api/v1/crates/{name}/{version}");
    let status = client
        .get(&url)
        .send()
        .with_context(|| format!("crates.io request failed for {name} {version}"))?
        .status();
    match status.as_u16() {
        200 => Ok(true),
        404 => Ok(false),
        other => anyhow::bail!("crates.io returned unexpected status {other} for {name} {version}"),
    }
}

/// Polls the sparse index until `version` of `name` is visible or `timeout` elapses.
pub(crate) fn wait_for_index(
    client: &reqwest::blocking::Client,
    name: &str,
    version: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let url = format!("{INDEX_HOST}/{}", index_path(name));
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(resp) = client.get(&url).send() {
            if let Ok(body) = resp.text() {
                if index_has_version(&body, version) {
                    return Ok(());
                }
            }
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "timed out waiting for {name} {version} to appear on the crates.io index"
        );
        std::thread::sleep(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn index_path_uses_cratesio_layout() {
        assert_eq!(index_path("flaps-domain"), "fl/ap/flaps-domain");
        assert_eq!(index_path("flapsd"), "fl/ap/flapsd");
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
    }

    #[test]
    fn detects_version_in_sparse_index() {
        let body = "{\"name\":\"flaps-domain\",\"vers\":\"0.1.0\",\"deps\":[]}\n\
                    {\"name\":\"flaps-domain\",\"vers\":\"0.2.0\",\"deps\":[]}\n";
        assert!(index_has_version(body, "0.2.0"));
        assert!(!index_has_version(body, "0.3.0"));
    }

    #[test]
    fn recognises_rate_limit_output() {
        assert!(is_rate_limit_error(
            "error: failed to publish: 429 Too Many Requests"
        ));
        assert!(is_rate_limit_error(
            "You have published too many new crates"
        ));
        assert!(!is_rate_limit_error("error: crate version already exists"));
    }

    #[test]
    fn backoff_honours_retry_after_then_falls_back_to_exponential() {
        assert_eq!(backoff_delay(3, Some(42)), Duration::from_secs(42));
        assert_eq!(backoff_delay(0, None), Duration::from_secs(1));
        assert_eq!(backoff_delay(3, None), Duration::from_secs(8));
        assert_eq!(backoff_delay(20, None), Duration::from_secs(300)); // capped
    }
}
