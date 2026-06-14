//! Atomic disk snapshot for warm-start resilience.
//!
//! Writes `{ "version": <u64 | null>, "document": "<flagd json>" }` to
//! `<path>.tmp` then renames it to `<path>` (atomic within the same file
//! system). Errors are logged as warnings and never propagate to the caller.
//!
//! The resolve hot-path always reads from the in-memory [`ArcSwap`]; the
//! snapshot is only used at provider startup for a warm-start when the server
//! is unreachable.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::warn;

use flaps_eval::FlagSet;

use crate::shared::ProviderShared;

/// On-disk representation of a ruleset snapshot.
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotFile {
    /// Ruleset version, if known.
    version: Option<u64>,
    /// Raw flagd JSON document.
    document: String,
}

/// Writes `version` and `document` to `path` atomically (tmp + rename).
///
/// Errors are logged as warnings; this function never panics.
pub(crate) async fn write_snapshot(path: &Path, version: Option<u64>, document: &str) {
    let snapshot = SnapshotFile {
        version,
        document: document.to_owned(),
    };

    let json = match serde_json::to_string(&snapshot) {
        Ok(j) => j,
        Err(err) => {
            warn!(error = %err, "failed to serialize ruleset snapshot");
            return;
        }
    };

    // Derive a sibling `.tmp` path.
    let tmp_path = {
        let mut p = path.to_path_buf();
        let file_name = p.file_name().map_or_else(
            || std::ffi::OsString::from("snapshot.tmp"),
            |n| {
                let mut s = n.to_os_string();
                s.push(".tmp");
                s
            },
        );
        p.set_file_name(file_name);
        p
    };

    if let Err(err) = tokio::fs::write(&tmp_path, json.as_bytes()).await {
        warn!(error = %err, path = %tmp_path.display(), "failed to write snapshot tmp file");
        return;
    }

    if let Err(err) = tokio::fs::rename(&tmp_path, path).await {
        warn!(error = %err, "failed to rename snapshot tmp file to final path");
        // Best-effort cleanup.
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }
}

/// Loads a snapshot from `path` into `shared`.
///
/// On success the ruleset [`arc_swap::ArcSwap`] is populated and `loaded_from_snapshot`
/// is set to `true`. Errors are logged as warnings; the provider falls back to
/// `None` ruleset (`NotReady`) until the first successful network sync.
pub(crate) async fn load_snapshot(path: &Path, shared: &Arc<ProviderShared>) {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(err) => {
            warn!(error = %err, path = %path.display(), "failed to read snapshot file");
            return;
        }
    };

    let snapshot: SnapshotFile = match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(err) => {
            warn!(error = %err, "failed to parse snapshot file");
            return;
        }
    };

    let flag_set = match FlagSet::from_json(&snapshot.document) {
        Ok(fs) => fs,
        Err(err) => {
            warn!(error = %err, "snapshot document failed to parse as flagd ruleset");
            return;
        }
    };

    shared.ruleset.store(Arc::new(Some(Arc::new(flag_set))));

    let mut state = shared
        .sync_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    state.version = snapshot.version;
    state.loaded_from_snapshot = true;
    // `last_successful_sync` remains `None`: the snapshot is a warm-start hint,
    // not evidence of a successful network sync this session.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a unique path under `std::env::temp_dir()` for test isolation.
    fn tmp_path(name: &str) -> std::path::PathBuf {
        // Use thread id + name for uniqueness across parallel test runs.
        let id = std::thread::current().id();
        std::env::temp_dir().join(format!("flaps_snapshot_test_{name}_{id:?}"))
    }

    #[tokio::test]
    async fn round_trip_write_then_read() {
        let path = tmp_path("round_trip");

        let document = r#"{"flags":{}}"#;
        write_snapshot(&path, Some(42), document).await;

        assert!(path.exists(), "snapshot file must exist after write");

        let bytes = std::fs::read(&path).expect("read snapshot");
        let parsed: SnapshotFile = serde_json::from_slice(&bytes).expect("parse snapshot");
        assert_eq!(parsed.version, Some(42));
        assert_eq!(parsed.document, document);

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn round_trip_none_version() {
        let path = tmp_path("none_version");

        let document = r#"{"flags":{}}"#;
        write_snapshot(&path, None, document).await;

        let bytes = std::fs::read(&path).expect("read snapshot");
        let parsed: SnapshotFile = serde_json::from_slice(&bytes).expect("parse snapshot");
        assert_eq!(parsed.version, None);

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn load_populates_shared_state() {
        let path = tmp_path("load_populated");

        // Write a minimal valid flagd document.
        let document = r#"{"flags":{"my-flag":{"state":"ENABLED","defaultVariant":"on","variants":{"on":true,"off":false}}}}"#;
        write_snapshot(&path, Some(7), document).await;

        let shared = Arc::new(ProviderShared::new());
        load_snapshot(&path, &shared).await;

        let state = shared.sync_state.lock().expect("lock");
        assert_eq!(state.version, Some(7));
        assert!(state.loaded_from_snapshot);
        assert!(state.last_successful_sync.is_none());
        drop(state);

        let guard = shared.ruleset.load();
        assert!(guard.is_some(), "ruleset must be populated after load");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn load_missing_file_leaves_shared_unchanged() {
        let path = tmp_path("missing_file_xyz_nonexistent_abc");

        let shared = Arc::new(ProviderShared::new());
        load_snapshot(&path, &shared).await;

        let guard = shared.ruleset.load();
        assert!(guard.is_none(), "ruleset must remain None on missing file");
    }

    #[tokio::test]
    async fn load_corrupt_json_leaves_shared_unchanged() {
        let path = tmp_path("corrupt_json");
        std::fs::write(&path, b"not-valid-json").expect("write");

        let shared = Arc::new(ProviderShared::new());
        load_snapshot(&path, &shared).await;

        let guard = shared.ruleset.load();
        assert!(
            guard.is_none(),
            "ruleset must remain None on corrupt snapshot"
        );

        let _ = std::fs::remove_file(&path);
    }
}
