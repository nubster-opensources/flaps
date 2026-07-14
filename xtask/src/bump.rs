//! Version bump and CHANGELOG graduation.

use anyhow::Context as _;
use std::process::Command;
use time::OffsetDateTime;
use time::macros::format_description;
use toml_edit::{DocumentMut, value};

/// Workspace crates that carry an internal `version` in `[workspace.dependencies]`.
/// `flapsd` is excluded: nothing depends on it, so no version to rewrite.
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

/// Repository base URL for CHANGELOG link references.
const REPO_URL: &str = "https://github.com/nubster-opensources/flaps";

/// Moves the `## [Unreleased]` body under a new `## [version] - date` section,
/// leaves `## [Unreleased]` empty, and refreshes the link references.
pub(crate) fn graduate_changelog(
    changelog: &str,
    version: &str,
    date: &str,
) -> anyhow::Result<String> {
    const UNRELEASED: &str = "## [Unreleased]";
    let start = changelog
        .find(UNRELEASED)
        .context("CHANGELOG.md is missing the `## [Unreleased]` section")?;
    let head = &changelog[..start];
    let rest = &changelog[start + UNRELEASED.len()..];
    let next = rest
        .find("\n## ")
        .context("CHANGELOG.md has no section after `## [Unreleased]`")?;
    let body = rest[..next].trim();
    anyhow::ensure!(
        !body.is_empty(),
        "`## [Unreleased]` is empty; nothing to graduate"
    );
    let tail = &rest[next + 1..];
    let graduated = format!("{head}## [Unreleased]\n\n## [{version}] - {date}\n\n{body}\n\n{tail}");
    update_link_refs(&graduated, version)
}

/// Rewrites the `[Unreleased]` link reference and ensures a `[version]` one exists.
fn update_link_refs(doc: &str, version: &str) -> anyhow::Result<String> {
    let unreleased_ref = format!("[Unreleased]: {REPO_URL}/compare/v{version}...HEAD");
    let version_ref = format!("[{version}]: {REPO_URL}/releases/tag/v{version}");
    let version_prefix = format!("[{version}]:");
    let mut lines: Vec<String> = doc.lines().map(str::to_owned).collect();
    let mut saw_version_ref = false;
    for line in &mut lines {
        if line.starts_with("[Unreleased]:") {
            unreleased_ref.clone_into(line);
        } else if line.starts_with(&version_prefix) {
            version_ref.clone_into(line);
            saw_version_ref = true;
        }
    }
    if !saw_version_ref {
        let pos = lines
            .iter()
            .position(|l| l.starts_with("[Unreleased]:"))
            .context("CHANGELOG.md is missing the `[Unreleased]` link reference")?;
        lines.insert(pos + 1, version_ref);
    }
    let mut out = lines.join("\n");
    if doc.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Returns the body of the `## [version]` section, trimmed.
pub(crate) fn extract_release_notes(changelog: &str, version: &str) -> anyhow::Result<String> {
    let heading = format!("## [{version}]");
    let start = changelog
        .find(&heading)
        .with_context(|| format!("CHANGELOG.md has no `{heading}` section"))?;
    // Skip to end of the heading line.
    let after_heading = changelog[start..]
        .find('\n')
        .map_or(changelog.len(), |i| start + i + 1);
    let rest = &changelog[after_heading..];
    let end = rest.find("\n## ").map_or(rest.len(), |i| i);
    Ok(rest[..end].trim().to_owned())
}

/// OSS commit author for release commits.
const RELEASE_AUTHOR: &str = "Pierrick Fonquerne <pierrick.fonquerne@gmail.com>";

/// Returns today's UTC date as `YYYY-MM-DD`.
fn today() -> anyhow::Result<String> {
    let now = OffsetDateTime::now_utc();
    let fmt = format_description!("[year]-[month]-[day]");
    now.format(&fmt).context("failed to format today's date")
}

/// Runs a git/gh command, returning an error with captured stderr on failure.
fn run(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{cmd} {}`", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "`{cmd} {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

/// Captures stdout of a command, trimmed.
fn capture(cmd: &str, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `{cmd} {}`", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "`{cmd} {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)
        .context("command produced non-UTF-8 output")?
        .trim()
        .to_owned())
}

/// Bumps the workspace, graduates the CHANGELOG, and opens the release PR.
pub(crate) fn run_release(version: &str) -> anyhow::Result<()> {
    semver::Version::parse(version)
        .with_context(|| format!("`{version}` is not a valid semantic version"))?;

    // Guard: on `main`, clean tree, and not behind origin/main.
    let branch = capture("git", &["rev-parse", "--abbrev-ref", "HEAD"])?;
    anyhow::ensure!(
        branch == "main",
        "release must run on `main`, not `{branch}`"
    );
    let dirty = capture("git", &["status", "--porcelain"])?;
    anyhow::ensure!(
        dirty.is_empty(),
        "working tree must be clean before a release bump"
    );
    run("git", &["fetch", "origin"])?;
    let behind = capture("git", &["rev-list", "--count", "main..origin/main"])?;
    anyhow::ensure!(
        behind == "0",
        "local `main` is {behind} commit(s) behind `origin/main`; pull before releasing"
    );

    // Create the release branch BEFORE mutating any file, so a failure never
    // leaves `main` dirty.
    let release_branch = format!("chore/release-{version}");
    run("git", &["switch", "-c", &release_branch])?;

    // Apply the version bump and CHANGELOG graduation on the release branch.
    let cargo_toml = std::fs::read_to_string("Cargo.toml").context("cannot read Cargo.toml")?;
    let bumped = rewrite_workspace_versions(&cargo_toml, version)?;
    std::fs::write("Cargo.toml", bumped).context("cannot write Cargo.toml")?;

    let changelog = std::fs::read_to_string("CHANGELOG.md").context("cannot read CHANGELOG.md")?;
    let graduated = graduate_changelog(&changelog, version, &today()?)?;
    std::fs::write("CHANGELOG.md", graduated).context("cannot write CHANGELOG.md")?;

    // Refresh Cargo.lock so the bumped versions are recorded.
    run("cargo", &["update", "--workspace"])?;

    // Commit (OSS author), push, open the PR.
    run(
        "git",
        &[
            "commit",
            "-am",
            &format!("chore(release): v{version}"),
            "--author",
            RELEASE_AUTHOR,
        ],
    )?;
    run("git", &["push", "-u", "origin", &release_branch])?;

    let notes = extract_release_notes(
        &std::fs::read_to_string("CHANGELOG.md").context("cannot re-read CHANGELOG.md")?,
        version,
    )?;
    run(
        "gh",
        &[
            "pr",
            "create",
            "--title",
            &format!("chore(release): v{version}"),
            "--body",
            &notes,
            "--base",
            "main",
        ],
    )?;
    println!("Opened release PR for v{version} on branch {release_branch}");
    Ok(())
}

/// Prints the CHANGELOG notes for `version` to stdout (used by the release workflow).
pub(crate) fn print_changelog_notes(version: &str) -> anyhow::Result<()> {
    let changelog = std::fs::read_to_string("CHANGELOG.md").context("cannot read CHANGELOG.md")?;
    let notes = extract_release_notes(&changelog, version)?;
    println!("{notes}");
    Ok(())
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

    const CHANGELOG: &str = "\
# Changelog

The format follows Keep a Changelog.

## [Unreleased]

### Added

- Feature one.
- Feature two.

## M0: Foundations

### Added

- Bootstrap.

[Unreleased]: https://github.com/nubster-opensources/flaps/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nubster-opensources/flaps/releases/tag/v0.1.0
";

    #[test]
    fn graduates_unreleased_under_new_version() {
        let out = graduate_changelog(CHANGELOG, "0.2.0", "2026-07-13").unwrap();
        assert!(
            out.contains(
                "## [Unreleased]\n\n## [0.2.0] - 2026-07-13\n\n### Added\n\n- Feature one."
            )
        );
        // Unreleased is now empty (heading immediately followed by the new version heading).
        // M0 section survives untouched.
        assert!(out.contains("## M0: Foundations"));
        // Link references updated.
        assert!(out.contains(
            "[Unreleased]: https://github.com/nubster-opensources/flaps/compare/v0.2.0...HEAD"
        ));
        assert!(
            out.contains(
                "[0.2.0]: https://github.com/nubster-opensources/flaps/releases/tag/v0.2.0"
            )
        );
    }

    #[test]
    fn refuses_to_graduate_empty_unreleased() {
        let empty = "# Changelog\n\n## [Unreleased]\n\n## M0: Foundations\n\n- x.\n";
        let err = graduate_changelog(empty, "0.2.0", "2026-07-13").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn extracts_notes_for_version() {
        let graduated = graduate_changelog(CHANGELOG, "0.2.0", "2026-07-13").unwrap();
        let notes = extract_release_notes(&graduated, "0.2.0").unwrap();
        assert!(notes.contains("- Feature one."));
        assert!(notes.contains("- Feature two."));
        assert!(!notes.contains("Foundations"));
    }
}
