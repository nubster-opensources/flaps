//! Idempotent, dependency-ordered crates.io publication.

/// Options for the `publish` command.
pub(crate) struct PublishOptions {
    /// Validate packaging without uploading.
    pub(crate) dry_run: bool,
}

/// Runs the `publish` command. Implemented in Task 5.
pub(crate) fn run_publish(opts: &PublishOptions) -> anyhow::Result<()> {
    let _ = opts.dry_run;
    anyhow::bail!("publish: not implemented yet")
}
