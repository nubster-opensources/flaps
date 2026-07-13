//! Flaps repository tooling.
//!
//! Internal `cargo xtask` binary providing the release automation the
//! project needs: a local version bump that opens a release PR, and the
//! publish engine driven by the tag workflow. The crate is marked
//! `publish = false` and never ships to crates.io.

mod bump;
mod crates_io;
mod publish;

use clap::{Parser, Subcommand};

/// Top-level CLI for the `xtask` binary.
#[derive(Debug, Parser)]
#[command(name = "xtask", about = "Flaps repository tooling.")]
struct Cli {
    /// Subcommand to execute.
    #[command(subcommand)]
    command: Command,
}

/// Available subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Bumps the workspace to `version`, graduates the CHANGELOG, and opens the release PR.
    Release {
        /// Target semantic version, for example `0.2.0`.
        version: String,
    },
    /// Publishes the workspace crates to crates.io in dependency order (idempotent).
    Publish {
        /// Validate packaging without uploading.
        #[arg(long)]
        dry_run: bool,
    },
    /// Prints the CHANGELOG release notes for `version` to stdout.
    ChangelogNotes {
        /// Version whose notes to extract, for example `0.2.0`.
        version: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Release { version } => bump::run_release(&version),
        Command::Publish { dry_run } => publish::run_publish(&publish::PublishOptions { dry_run }),
        Command::ChangelogNotes { version } => bump::print_changelog_notes(&version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_release_with_version() {
        let cli = Cli::parse_from(["xtask", "release", "0.2.0"]);
        assert!(matches!(cli.command, Command::Release { version } if version == "0.2.0"));
    }

    #[test]
    fn parses_publish_dry_run() {
        let cli = Cli::parse_from(["xtask", "publish", "--dry-run"]);
        assert!(matches!(cli.command, Command::Publish { dry_run: true }));
    }

    #[test]
    fn parses_changelog_notes() {
        let cli = Cli::parse_from(["xtask", "changelog-notes", "0.2.0"]);
        assert!(matches!(cli.command, Command::ChangelogNotes { version } if version == "0.2.0"));
    }
}
