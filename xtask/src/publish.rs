//! Idempotent, dependency-ordered crates.io publication.

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::Duration;

use anyhow::Context as _;

use crate::crates_io;

/// Options for the `publish` command.
pub(crate) struct PublishOptions {
    /// Validate packaging without uploading.
    pub(crate) dry_run: bool,
}

/// Publication order: every crate is published after all of its internal dependencies.
pub(crate) const PUBLISH_ORDER: [&str; 7] = [
    "flaps-domain",
    "flaps-eval",
    "flaps-compiler",
    "flaps-store",
    "flaps-client",
    "flaps-server",
    "flapsd",
];

/// Max publish attempts per crate before giving up (a re-run resumes idempotently).
const MAX_PUBLISH_ATTEMPTS: u32 = 6;
/// How long to wait for a freshly published crate to appear on the index.
const INDEX_TIMEOUT: Duration = Duration::from_secs(180);

/// Fails if `order` is not a valid topological order of the internal dependency
/// graph described by `metadata_json`, or if it names a crate absent from the workspace.
pub(crate) fn validate_topo_order(order: &[&str], metadata_json: &str) -> anyhow::Result<()> {
    let meta: serde_json::Value =
        serde_json::from_str(metadata_json).context("cargo metadata is not valid JSON")?;
    let packages = meta["packages"]
        .as_array()
        .context("cargo metadata has no packages array")?;
    let position: HashMap<&str, usize> = order.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let present: HashSet<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
    for name in order {
        anyhow::ensure!(
            present.contains(name),
            "publish order lists `{name}` which is not in the workspace"
        );
    }
    for pkg in packages {
        let Some(name) = pkg["name"].as_str() else {
            continue;
        };
        let Some(&idx) = position.get(name) else {
            continue;
        };
        let deps = pkg["dependencies"]
            .as_array()
            .map_or(&[][..], Vec::as_slice);
        for dep in deps {
            // Dev-dependencies are stripped from published crates and do not constrain
            // publish order. Normal (kind null) and build dependencies still count.
            if dep["kind"].as_str() == Some("dev") {
                continue;
            }
            let Some(dep_name) = dep["name"].as_str() else {
                continue;
            };
            if let Some(&dep_idx) = position.get(dep_name) {
                anyhow::ensure!(
                    dep_idx < idx,
                    "publish order invalid: `{name}` depends on `{dep_name}` but is not published after it"
                );
            }
        }
    }
    Ok(())
}

/// Runs `cargo metadata --no-deps --format-version 1` and returns its stdout.
fn cargo_metadata_json() -> anyhow::Result<String> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()
        .context("failed to run `cargo metadata`")?;
    anyhow::ensure!(out.status.success(), "`cargo metadata` failed");
    String::from_utf8(out.stdout).context("`cargo metadata` produced non-UTF-8 output")
}

/// Reads `[workspace.package] version` from the root Cargo.toml.
fn workspace_version() -> anyhow::Result<String> {
    let text = std::fs::read_to_string("Cargo.toml").context("cannot read root Cargo.toml")?;
    let doc: toml_edit::DocumentMut = text.parse().context("root Cargo.toml is not valid TOML")?;
    doc["workspace"]["package"]["version"]
        .as_str()
        .map(str::to_owned)
        .context("[workspace.package] version is missing")
}

/// Returns the internal (in-workspace) dependency names of `name`.
fn internal_deps<'a>(name: &str, metadata_json: &'a serde_json::Value) -> Vec<&'a str> {
    let order: HashSet<&str> = PUBLISH_ORDER.iter().copied().collect();
    metadata_json["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"].as_str() == Some(name))
        .and_then(|p| p["dependencies"].as_array())
        .map(|deps| {
            deps.iter()
                .filter(|d| d["kind"].as_str() != Some("dev")) // exclude dev-dependencies only
                .filter_map(|d| d["name"].as_str())
                .filter(|d| order.contains(d))
                .collect()
        })
        .unwrap_or_default()
}

/// Publishes (or dry-run validates) every crate in dependency order, idempotently.
pub(crate) fn run_publish(opts: &PublishOptions) -> anyhow::Result<()> {
    let metadata_text = cargo_metadata_json()?;
    validate_topo_order(&PUBLISH_ORDER, &metadata_text)?;
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata_text).context("cargo metadata is not valid JSON")?;
    let version = workspace_version()?;
    let client = crates_io::client()?;

    for name in PUBLISH_ORDER {
        if crates_io::is_published(&client, name, &version)? {
            println!("{name} {version}: already published, skipping");
            continue;
        }
        if opts.dry_run {
            let deps_ready =
                internal_deps(name, &metadata)
                    .into_iter()
                    .try_fold(true, |ready, dep| {
                        Ok::<bool, anyhow::Error>(
                            ready && crates_io::is_published(&client, dep, &version)?,
                        )
                    })?;
            dry_run_crate(name, deps_ready)?;
            continue;
        }
        publish_with_retry(name)?;
        crates_io::wait_for_index(&client, name, &version, INDEX_TIMEOUT)?;
    }
    Ok(())
}

/// Runs `cargo publish -p name --dry-run`, adding `--no-verify` when internal deps are not yet published.
fn dry_run_crate(name: &str, deps_ready: bool) -> anyhow::Result<()> {
    let mut args = vec!["publish", "-p", name, "--dry-run"];
    if !deps_ready {
        // Downstream crate whose internal deps are not on the registry yet: the build
        // verification would fail resolving them, so validate packaging only.
        args.push("--no-verify");
    }
    println!(
        "{name}: dry-run ({})",
        if deps_ready { "full" } else { "packaging only" }
    );
    let status = Command::new("cargo")
        .args(&args)
        .status()
        .with_context(|| format!("failed to run cargo publish --dry-run for {name}"))?;
    anyhow::ensure!(status.success(), "dry-run failed for {name}");
    Ok(())
}

/// Runs `cargo publish -p name`, retrying with backoff on a crates.io rate limit.
fn publish_with_retry(name: &str) -> anyhow::Result<()> {
    for attempt in 0..MAX_PUBLISH_ATTEMPTS {
        println!("{name}: publishing (attempt {})", attempt + 1);
        let out = Command::new("cargo")
            .args(["publish", "-p", name])
            .output()
            .with_context(|| format!("failed to run cargo publish for {name}"))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::ensure!(
            crates_io::is_rate_limit_error(&stderr),
            "cargo publish failed for {name}:\n{stderr}"
        );
        let delay = crates_io::backoff_delay(attempt, None);
        if attempt + 1 < MAX_PUBLISH_ATTEMPTS {
            println!(
                "{name}: rate limited, waiting {}s before retry",
                delay.as_secs()
            );
            std::thread::sleep(delay);
        }
    }
    anyhow::bail!("{name}: exhausted publish retries; re-run the workflow to resume")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal `cargo metadata --no-deps` shape: compiler depends on domain.
    const META: &str = r#"{
        "packages": [
            {"name": "flaps-domain", "dependencies": []},
            {"name": "flaps-eval", "dependencies": [{"name": "flaps-domain"}]},
            {"name": "flaps-compiler", "dependencies": [{"name": "flaps-domain"}, {"name": "serde"}]},
            {"name": "flaps-store", "dependencies": [{"name": "flaps-domain"}]},
            {"name": "flaps-client", "dependencies": [{"name": "flaps-eval"}]},
            {"name": "flaps-server", "dependencies": [{"name": "flaps-store"}, {"name": "flaps-compiler"}]},
            {"name": "flapsd", "dependencies": [{"name": "flaps-server"}]}
        ]
    }"#;

    #[test]
    fn accepts_a_valid_topological_order() {
        validate_topo_order(&PUBLISH_ORDER, META).unwrap();
    }

    #[test]
    fn rejects_an_order_that_violates_a_dependency() {
        let bad = [
            "flaps-eval",
            "flaps-domain",
            "flaps-compiler",
            "flaps-store",
            "flaps-client",
            "flaps-server",
            "flapsd",
        ];
        let err = validate_topo_order(&bad, META).unwrap_err();
        assert!(err.to_string().contains("flaps-eval"));
        assert!(err.to_string().contains("flaps-domain"));
    }

    #[test]
    fn rejects_an_order_naming_a_crate_absent_from_the_workspace() {
        let meta = r#"{"packages":[{"name":"flaps-domain","dependencies":[]}]}"#;
        let err = validate_topo_order(&["flaps-domain", "ghost"], meta).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn skips_dev_dependencies_in_order_check() {
        // `alpha` is published before `omega`, but has a DEV-dependency on it.
        // Dev-deps are stripped from published crates, so order is not violated.
        let meta = r#"{"packages":[
            {"name":"alpha","dependencies":[{"name":"omega","kind":"dev"}]},
            {"name":"omega","dependencies":[]}
        ]}"#;
        validate_topo_order(&["alpha", "omega"], meta).unwrap();
    }

    #[test]
    fn enforces_build_dependencies_in_order_check() {
        // A BUILD-dependency ships inside the published crate and DOES constrain
        // order: `alpha` (published first) build-depends on `omega` (published
        // later), which must be rejected.
        let meta = r#"{"packages":[
            {"name":"alpha","dependencies":[{"name":"omega","kind":"build"}]},
            {"name":"omega","dependencies":[]}
        ]}"#;
        let err = validate_topo_order(&["alpha", "omega"], meta).unwrap_err();
        assert!(err.to_string().contains("alpha"));
        assert!(err.to_string().contains("omega"));
    }
}
