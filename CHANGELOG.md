# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
This project adheres to [Semantic Versioning](docs/SEMVER_POLICY.md).

## [Unreleased]

### Added

- `flaps-domain`: rich flag model covering projects, environments, flags, variants, segments,
  targeting rules, and SDK keys.
- `flaps-eval`: evaluation engine for compiled flagd rulesets with JsonLogic targeting,
  fractional rollouts, semver operators, and a public conformance corpus.
- `flaps-compiler`: compiles the domain model into one versioned, hashed flagd ruleset per
  environment; remote and in-process evaluation cannot diverge by construction.
- `flaps-store`: multi-backend persistence (SQLite, PostgreSQL) via SQLx with schema migrations
  and an append-only transactional audit log.
- `flaps-server`: admin REST API, OFREP endpoints, ruleset sync with server-sent events,
  token-bucket rate limiting, and SDK key management.
- `flapsd`: server daemon with TOML configuration, first-admin bootstrap, and structured logging.
- `flaps-client`: OpenFeature in-process provider for Rust with HTTP sync, SSE notifications,
  local evaluation, disk snapshot fallback, and staleness metrics.
- `xtask`: release automation (version bump and idempotent crates.io publish).
- Flag and flag-set metadata carried end to end through to OFREP evaluation responses.
- Admin read endpoints now require an authenticated admin session.
- HTTP API reference (OpenAPI plus guide) with a CI coverage guard.
- CI: fmt, clippy, tests (Ubuntu, macOS, Windows), PostgreSQL integration, MSRV check,
  supply-chain audit.
- Dual MIT OR Apache-2.0 licensing, `deny.toml` supply-chain policy.

## M0: Foundations

### Added

- Public Cargo workspace: seven library/binary crates plus xtask.
- CI pipeline: format, Clippy, tests (3-OS matrix), supply-chain check (cargo-deny), MSRV.
- Dual MIT OR Apache-2.0 licensing.
- Public documentation set: architecture, interoperability, ADR FLAPS-001.
- `deny.toml` supply-chain policy.

[Unreleased]: https://github.com/nubster-opensources/flaps/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nubster-opensources/flaps/releases/tag/v0.1.0
