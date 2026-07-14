# Release process

Flaps ships to crates.io in two steps: a local version bump that opens a PR, and a tag that triggers an automated, human-gated publish. All logic lives in the `xtask` binary so it is testable and replayable off CI.

## 1. Bump the version (local)

On an up-to-date `main` with a clean working tree:

```sh
cargo xtask release 0.2.0
```

This command:

1. Refuses to run unless you are on `main` with a clean tree.
2. Rewrites `[workspace.package] version` and every internal crate version in `[workspace.dependencies]` to the target.
3. Graduates `CHANGELOG.md`: moves the `[Unreleased]` body under a new `[<version>] - <date>` section and refreshes the link references.
4. Refreshes `Cargo.lock`.
5. Creates `chore/release-<version>`, commits, pushes, and opens a PR via `gh`.

Review the PR and merge it with `--no-ff`. `main` now reflects the release.

## 2. Tag to publish

After the release PR is merged:

```sh
git switch main
git pull origin main
git tag -a v0.2.0 -m "v0.2.0"
git push origin v0.2.0
```

The tag triggers [`.github/workflows/release.yml`](../.github/workflows/release.yml):

- The `verify` job checks the tag matches the workspace version, then builds, tests, and runs `cargo xtask publish --dry-run`.
- The `publish` job pauses on the protected `crates-io` GitHub Environment until a maintainer approves. On approval it runs `cargo xtask publish` (idempotent) and creates the GitHub release from the `[<version>]` CHANGELOG section.

Tagging and approving are deliberately manual: the human who reviewed the PR is the one who releases the crates.

## Publication order

Crates publish in dependency order, each after its internal dependencies are visible on the crates.io index:

`flaps-domain -> flaps-eval -> flaps-compiler -> flaps-store -> flaps-client -> flaps-server -> flapsd`

`xtask` is internal and never published. `cargo install flapsd` is a supported install path.

## Idempotence and recovery

`cargo xtask publish` skips any crate whose exact name and version is already on crates.io. crates.io rate-limits the creation of new crate names in bursts; if publication is interrupted mid-batch, re-approve or re-run the `publish` job. It resumes at the first unpublished crate. crates.io is append-only: there is no rollback, only `cargo yank`.

## Dry-run

`cargo xtask publish --dry-run` validates packaging without uploading. On the very first release, no internal dependency is on crates.io yet, so downstream crates are validated with `--no-verify` (packaging only); the leaf crates (`flaps-domain`, `flaps-eval`) always get a full dry-run.

## One-time ops setup

Before the first real tag:

- Repository secret `CARGO_REGISTRY_TOKEN`: a crates.io token scoped to publish-new and publish-update.
- GitHub Environment `crates-io` with a required reviewer.

## Failure modes

- `release must run on main`: switch to `main` and retry.
- `working tree must be clean`: commit or stash local changes first.
- `[Unreleased] is empty`: add entries under `## [Unreleased]` before bumping.
- Publish stops on a rate limit: re-run the `publish` job; it resumes idempotently.
- `gh` auth error: check `gh auth status`; in CI the token is provided automatically.
