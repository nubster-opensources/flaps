# Architecture

## The compiled ruleset pivot

The rich flag model (projects, environments, flags, variants, reusable segments, targeting rules) lives in the database and is edited through the admin API. A compiler transforms it into one canonical flagd compatible ruleset per environment. That compiled artifact is the only thing ever evaluated.

    +-- database (rich model: flags, segments, environments) --+
    |                  admin REST API                          |
    +---------------------------+------------------------------+
                                | compilation (on every mutation)
                                v
          canonical flagd ruleset (per environment, versioned, content hashed)
               |                                  |
    +----------+-----------+        +-------------+------------+
    | evaluated BY THE     |        | distributed TO clients   |
    | SERVER via flaps-eval|        | in-process (sync + SSE)  |
    | (OFREP endpoints)    |        |                          |
    +----------------------+        +--------------------------+

Remote and local evaluation use the same artifact and the same engine (`flaps-eval`), so they cannot diverge. The expressiveness of targeting rules is deliberately bounded by the flagd format (JsonLogic plus fractional, semantic version and string operators).

## Crate dependency rules

- `flaps-eval` does not depend on `flaps-domain`. It evaluates serialized flagd JSON. The public boundary of the product is a stable external standard, not the internal model.
- The server and `flaps-client` embed the same `flaps-eval` engine: parity by construction.
- `flaps-eval` and `flaps-client` are published on crates.io and are usable against any flagd compatible source.

## Data flow

1. A mutation goes through the admin API inside a database transaction: entity write plus audit entry. The commit is the source of truth.
2. Compilation runs synchronously after the commit, outside the transaction. Each affected environment gets a new ruleset with a monotonic version and a content hash (ETag). A crash between commit and compilation is harmless: compilation is idempotent and re-runs at boot.
3. The server stores the ruleset and swaps it atomically in memory. An invalid artifact is never swapped in.
4. A `ruleset.changed { env, version }` event is published over SSE. Changing a segment recompiles every affected environment but emits one event per environment.

## Evaluation paths

- **Remote (OFREP):** SDK key authentication resolves the environment, then `flaps-eval` evaluates against the in-memory ruleset. The database is never on the hot path. Bulk evaluation supports `If-None-Match` and returns 304 when unchanged.
- **In-process (server keys only):** clients fetch the full ruleset at boot, then listen on SSE. Events are notify-then-fetch: they carry only `{ env, version }` and the client re-fetches over its authenticated HTTP channel, so a missed event is a missed notification, never lost data. A configurable backup polling interval (default five minutes) bounds the worst case. Client keys never receive the ruleset: browsers and mobile apps use OFREP only, so targeting data such as segment definitions stays on the backend.

## Kill switch path

Toggle off, transaction (update plus audit), recompile, atomic in-memory swap, immediate OFREP effect plus SSE notification, client re-fetch. Target: under two seconds to connected in-process clients.

## Error handling

Fail-closed on writes, fail-safe on distribution.

- The admin API rejects invalid rules and segments with structured 400 errors. If compilation still fails, the last valid ruleset stays in memory and the failure is logged and measured. A broken artifact is never served.
- OFREP errors are normalized: `FLAG_NOT_FOUND` 404, `INVALID_CONTEXT` 400, 401 and 403 for keys, 429 with `Retry-After` under rate limiting.
- Admin updates use `If-Match` ETags against lost updates.
- An evaluation in `flaps-client` never fails (OpenFeature contract): in-memory ruleset, then optional disk snapshot, then the coded default with `reason: ERROR`. SSE reconnects with exponential backoff and jitter. Staleness metrics (ruleset age, last sync) expose when a client is flying blind.
