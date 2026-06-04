# FLAPS-001: Compiled ruleset as the single evaluation artifact

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-06-04 |
| **Issues** | |

## Context

Flaps needs both remote evaluation (OFREP, for any OpenFeature SDK) and in-process evaluation (for backend services that cannot afford a network hop or an outage of the flag server). The worst class of bug in this domain is divergence: a flag evaluating `on` remotely and `off` locally.

Three architectures were considered:

- **A. Compiled ruleset pivot.** The rich model (flags, reusable segments, per-environment overrides) lives in the database. A compiler produces one canonical flagd compatible ruleset per environment. That artifact is the only thing evaluated, both by the server for OFREP and by in-process clients.
- **B. Dual path.** The server evaluates the rich model directly; a flagd export exists for in-process clients. Parity between the two paths is maintained by tests.
- **C. Native flagd storage.** Store the flagd JSON directly and edit it through the API.

## Decision

Option A. The compiler inlines segments, resolves per-environment overrides and emits a versioned, content-hashed artifact. The server and the in-process client embed the same evaluation engine (`flaps-eval`), which deliberately does not depend on the domain model.

## Consequences

- Remote/local parity is structural, not test-maintained. Divergence is impossible by construction.
- Rule expressiveness is bounded by the flagd format (JsonLogic plus fractional, semantic version and string operators). This is an accepted constraint; the targeted feature set fits entirely.
- Option B was rejected because parity by test suite is a permanent risk. Option C was rejected because reusable segments and a rich admin experience are impossible on raw flagd JSON.
- Segment edits trigger recompilation of every affected environment, one SSE event per environment.
