# Roadmap

This document tracks the planned and delivered milestones of Flaps.
Dates are not committed; the table reflects current priority order only.

| Version | Theme | Status |
|---------|-------|--------|
| M0 | Foundations | Shipped |
| v0.1.0 | Headless MVP | In progress |
| v0.2.0 | Admin UI and OIDC | Planned |
| v0.3.0 | Ecosystem and Integration | Planned |
| v0.4.0 | Lifecycle and Operability | Planned |

## M0: Foundations (shipped)

Public repository, Cargo workspace with seven crates plus xtask, CI (fmt, clippy, tests,
supply chain, MSRV), dual MIT OR Apache-2.0 licensing, public documentation set.

## v0.1.0: Headless MVP (in progress)

Everything needed to manage and evaluate flags through the API, with no UI:

- `flaps-eval`: flagd ruleset evaluation engine with a public conformance corpus.
- `flaps-domain`, `flaps-store`, `flaps-compiler`: rich model, SQLite and PostgreSQL
  persistence with a transactional audit log, compilation to canonical per-environment
  rulesets.
- `flaps-server` and `flapsd`: admin REST API with local accounts and SDK keys, OFREP
  endpoints, ruleset sync with SSE notifications.
- `flaps-client`: OpenFeature in-process provider for Rust, end-to-end kill switch
  propagation under two seconds.
- Release: crates.io publication, container image.

## v0.2.0: Admin UI and OIDC (planned)

Embedded server-rendered admin UI (deactivatable by configuration), generic OIDC discovery
for human sign-in, feedback from first production usage.

## v0.3.0: Ecosystem and Integration (planned)

Connect Flaps to the wider ecosystem without lock-in:

- Outbound change events as CloudEvents and webhooks.
- Flags-as-code: declarative flag definitions with GitOps reconciliation.
- Additional language SDKs over OFREP.

## v0.4.0: Lifecycle and Operability (planned)

Manage flags over time:

- Scheduled activation and expiration.
- Stale flag detection built on flag types.
- Change history and insights.
- Change approvals.

## Backlog

Beyond v0.4: fine-grained RBAC, A/B testing and experimentation metrics.
