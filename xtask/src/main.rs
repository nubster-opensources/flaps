//! Flaps repository tooling.
//!
//! Internal `cargo xtask` binary providing the maintenance commands the
//! release process needs. The crate is marked `publish = false` and never
//! ships to crates.io.

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
    /// Runs the release pre-flight checks.
    ReleaseCheck,
}

#[allow(clippy::unnecessary_wraps)]
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::ReleaseCheck => {
            println!("xtask: release-check is a placeholder until the v0.1.0 release batch.");
        }
    }
    Ok(())
}
