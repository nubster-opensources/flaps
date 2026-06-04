# Roadmap

This document tracks the milestones of the repository issue tracker.

## M0: Foundations (current)

Public repository, Cargo workspace with seven crates plus xtask, CI (fmt, clippy, tests, supply chain, MSRV), dual MIT OR Apache-2.0 licensing, public documentation set.

## v0.1.0: Headless MVP

Everything needed to manage and evaluate flags through the API, with no UI:

- `flaps-eval`: flagd ruleset evaluation engine with a public conformance corpus.
- `flaps-domain`, `flaps-store`, `flaps-compiler`: rich model, SQLite and PostgreSQL persistence with a transactional audit log, compilation to canonical per-environment rulesets.
- `flaps-server` and `flapsd`: admin REST API with local accounts and SDK keys, OFREP endpoints, ruleset sync with SSE notifications.
- `flaps-client`: OpenFeature in-process provider for Rust, end-to-end kill switch propagation under two seconds.
- Release: crates.io publication, container image.

## v0.2.0: Admin UI and OIDC

Embedded server-rendered admin UI (deactivatable by configuration), generic OIDC discovery for human sign-in, feedback from first production usage.

## Later

Stale flag detection built on flag types, flag scheduling, outbound events, flags-as-code reconciliation.
