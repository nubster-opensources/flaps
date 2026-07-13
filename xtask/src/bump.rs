//! Version bump and CHANGELOG graduation.

use anyhow::Context as _;
use toml_edit::{DocumentMut, value};

/// Workspace crates that carry an internal `version` in `[workspace.dependencies]`.
/// `flapsd` is excluded: nothing depends on it, so no version to rewrite.
// Not yet called outside tests: `run_release` wires it in during Task 6. Remove this
// `allow` once that call site lands.
#[allow(dead_code)]
pub(crate) const INTERNAL_CRATES: [&str; 6] = [
    "flaps-domain",
    "flaps-eval",
    "flaps-compiler",
    "flaps-store",
    "flaps-client",
    "flaps-server",
];

/// Rewrites `[workspace.package] version` and every internal crate version in
/// `[workspace.dependencies]` to `version`, preserving comments and formatting.
// Not yet called outside tests: `run_release` wires it in during Task 6. Remove this
// `allow` once that call site lands.
#[allow(dead_code)]
pub(crate) fn rewrite_workspace_versions(
    cargo_toml: &str,
    version: &str,
) -> anyhow::Result<String> {
    let mut doc = cargo_toml
        .parse::<DocumentMut>()
        .context("root Cargo.toml is not valid TOML")?;
    doc["workspace"]["package"]["version"] = value(version);
    let deps = doc["workspace"]["dependencies"]
        .as_table_mut()
        .context("[workspace.dependencies] table is missing")?;
    for name in INTERNAL_CRATES {
        let entry = deps
            .get_mut(name)
            .with_context(|| format!("[workspace.dependencies] is missing `{name}`"))?;
        let inline = entry.as_inline_table_mut().with_context(|| {
            format!("`{name}` in [workspace.dependencies] is not an inline table")
        })?;
        inline.insert("version", version.into());
    }
    Ok(doc.to_string())
}

/// Runs the `release <version>` command. Implemented in Task 6.
pub(crate) fn run_release(_version: &str) -> anyhow::Result<()> {
    anyhow::bail!("release: not implemented yet")
}

/// Prints the CHANGELOG notes for `version`. Implemented in Task 3.
pub(crate) fn print_changelog_notes(_version: &str) -> anyhow::Result<()> {
    anyhow::bail!("changelog-notes: not implemented yet")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[workspace.package]
version = "0.1.0"

[workspace.dependencies]
# Intra-workspace crates
flaps-domain = { version = "0.1.0", path = "crates/flaps-domain" }
flaps-eval = { version = "0.1.0", path = "crates/flaps-eval" }
flaps-compiler = { version = "0.1.0", path = "crates/flaps-compiler" }
flaps-store = { version = "0.1.0", path = "crates/flaps-store" }
flaps-client = { version = "0.1.0", path = "crates/flaps-client" }
flaps-server = { version = "0.1.0", path = "crates/flaps-server" }
serde = { version = "1", features = ["derive"] }
"#;

    #[test]
    fn rewrites_package_and_internal_versions_only() {
        let out = rewrite_workspace_versions(SAMPLE, "0.2.0").unwrap();
        // workspace.package bumped.
        assert!(out.contains("[workspace.package]\nversion = \"0.2.0\""));
        // Each internal dep bumped, path preserved.
        assert!(
            out.contains(r#"flaps-domain = { version = "0.2.0", path = "crates/flaps-domain" }"#)
        );
        assert!(
            out.contains(r#"flaps-server = { version = "0.2.0", path = "crates/flaps-server" }"#)
        );
        // Comment preserved.
        assert!(out.contains("# Intra-workspace crates"));
        // Third-party dep untouched.
        assert!(out.contains(r#"serde = { version = "1", features = ["derive"] }"#));
    }

    #[test]
    fn errors_when_internal_dep_missing() {
        let broken =
            "[workspace.package]\nversion = \"0.1.0\"\n\n[workspace.dependencies]\nserde = \"1\"\n";
        let err = rewrite_workspace_versions(broken, "0.2.0").unwrap_err();
        assert!(err.to_string().contains("flaps-domain"));
    }
}
