# Flaps

> Feature flag management server and SDK in Rust: OFREP remote evaluation, in-process flagd rulesets, progressive rollouts and instant kill switches.

[![CI](https://github.com/nubster-opensources/flaps/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/nubster-opensources/flaps/actions/workflows/ci.yml)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-blue.svg)](./docs/MSRV_POLICY.md)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Status](https://img.shields.io/badge/status-pre--alpha-orange)](#status)
[![Made with Rust](https://img.shields.io/badge/made%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)

Flaps is a feature flag server and SDK written in Rust. A single server stores a rich flag model
(projects, environments, segments, targeting rules) and compiles it into one canonical
[flagd](https://flagd.dev) ruleset per environment. That compiled artifact is the only thing ever
evaluated: the server evaluates it to answer [OFREP](https://github.com/open-feature/protocol)
requests, and backend clients download the very same artifact to evaluate flags in-process with
sub-millisecond latency. Remote and local evaluation cannot diverge by construction.

Flaps is built for teams practising trunk-based development, where deployment and release are
decoupled: code ships continuously behind flags, releases are runtime decisions, and the kill
switch is the critical path. A flag turned off propagates to connected in-process clients in under
two seconds.

Flaps is sponsored by [Nubster](https://nubster.com).

## Status

Pre-alpha. The v0.1.0 milestone targets a headless MVP: admin REST API, OFREP endpoints, ruleset
sync with SSE, an OpenFeature in-process provider for Rust, SQLite and PostgreSQL storage, and a
transactional audit log. The admin UI ships in v0.2.

See [the roadmap](docs/explanation/roadmap.md) for the full version plan.

## Quick start

> The release tooling and container image are part of the v0.1.0 milestone; this section will be
> completed when those artifacts ship. In the meantime you can build and run flapsd from source:

```sh
git clone https://github.com/nubster-opensources/flaps
cd flaps
cargo build --release -p flapsd
./target/release/flapsd --help
```

See [Getting started](docs/getting-started.md) for the full walkthrough (configuration, first
admin bootstrap, SDK key creation, in-process provider setup).

## Why Flaps?

- **One artifact, two evaluation modes.** The compiled flagd ruleset is served over OFREP for any
  OpenFeature SDK and distributed to in-process providers. No proprietary SDK to adopt, no
  remote/local parity bugs.
- **Kill switch as a first-class path.** Disabling a flag recompiles, swaps atomically in memory,
  and notifies clients over server-sent events. Target: under two seconds end to end.
- **Progressive rollouts without reshuffling.** Deterministic fractional rollouts hash the
  targeting key: moving from 25% to 50% never reassigns a user who was already in.
- **Resilient by design.** Fail-closed on writes, fail-safe on distribution. Clients fall back
  from in-memory ruleset to disk snapshot to coded defaults, and expose staleness metrics.
- **No vendor lock-in.** OpenFeature and OFREP for consumption, the flagd format for in-process
  evaluation, plain HTTP and SSE for sync. Any OpenFeature SDK works out of the box.

## What Flaps is not

- **Not a full-featured managed service.** Flaps is a self-hosted server. It does not include
  billing, multi-tenancy at the network level, or a hosted SaaS tier in this release.
- **Not a general-purpose config store.** The flag model is intentionally opinionated: boolean and
  multivariate flags, targeting rules, and rollouts. Arbitrary JSON configuration belongs
  elsewhere.
- **Not a replacement for your OpenFeature SDK.** Flaps provides a provider for the Rust SDK;
  it does not reimplement the OpenFeature specification.

## Documentation

- [Getting started](docs/getting-started.md)
- [HTTP API reference](docs/spec/api-v1.md)
- [Architecture](docs/design/architecture.md)
- [Interoperability](docs/design/interop.md)
- [Roadmap](docs/explanation/roadmap.md)
- [ADR FLAPS-001: compiled ruleset pivot](docs/adr/FLAPS-001-compiled-ruleset-pivot.md)
- [Governance](docs/GOVERNANCE.md)
- [Release process](docs/RELEASE_PROCESS.md)
- [Semantic versioning policy](docs/SEMVER_POLICY.md)
- [MSRV policy](docs/MSRV_POLICY.md)

### Workspace

| Crate | Role |
|---|---|
| `flaps-domain` | Rich domain model: projects, environments, flags, variants, segments, rules, SDK keys |
| `flaps-eval` | Evaluation engine for flagd rulesets: JsonLogic targeting, fractional rollouts, semver and string operators |
| `flaps-compiler` | Compiles the domain model into one versioned, hashed flagd ruleset per environment |
| `flaps-store` | SQLx multi-backend persistence (SQLite, PostgreSQL), migrations, append-only audit log |
| `flaps-client` | OpenFeature in-process provider for Rust: HTTP sync, SSE notifications, local evaluation |
| `flaps-server` | Admin REST API, OFREP endpoints, ruleset sync and SSE distribution |
| `flapsd` | Server daemon: TOML configuration, first-admin bootstrap, wiring |
| `xtask` | Repository tooling: release pre-flight checks (not published) |

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull
request. For discussions and design questions, open an issue first.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT
license](LICENSE-MIT) at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
