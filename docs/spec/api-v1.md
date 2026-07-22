# flaps HTTP API reference

This guide is the human-readable companion to [`openapi.json`](./openapi.json)
(OpenAPI 3.1.0). It covers what a machine-readable contract does not express well:
the authentication model, the SSE contract, ETag/conditional-request semantics,
custom response headers, and the error format. For the exact shape of every
request and response, use `openapi.json`; this document points at it rather
than duplicating it.

The contract and this guide describe the current surface of `build_router`
(`crates/flaps-server/src/lib.rs`). A dedicated backlog issue will decide the
versioning scheme of the admin surface (prefix, media type, or header) before
1.0; `/ofrep/v1` and `/sync/v1` already carry a `/v1` prefix because that is
what their respective external protocols (OFREP, and flaps's own sync channel)
require, not because of an admin API versioning decision.

## 1. Overview

flaps-server exposes three families of routes:

- **Public**: `POST /login`. No authentication.
- **Admin**: everything under `/projects/**`, including SDK key management.
  Requires a session bearer token minted by `POST /login`.
- **SDK (data plane)**: `GET /sdk/whoami`, the OFREP evaluation endpoints, and
  the `/sync/v1/*` routes. Requires an SDK key bearer token.

The admin surface is a straightforward CRUD API over four aggregates (Project,
Environment, Flag, Segment) plus a fifth join aggregate (FlagEnvConfig: a
flag's targeting rules and rollout weights within one environment) and SDK key
management. The data plane is read-only and evaluation-only: SDK keys can
never mutate anything.

## 2. Authentication: two separate worlds

flaps hosts two credential systems in the same server, and they are
intentionally not interchangeable:

| | Admin session | SDK key |
|---|---|---|
| Minted by | `POST /login` (username + password) | `POST /projects/{project}/environments/{env}/keys` (an admin action) |
| Carried as | `Authorization: Bearer <session-token>` | `Authorization: Bearer <sdk-key>` |
| Scope | The whole instance (subject to the account) | One `(project, environment)` pair |
| Grants | Full CRUD on `/projects/**` | Read-only evaluation and sync |
| OpenAPI security scheme | `adminSession` | `sdkKey` |

A session token never works on an SDK route and vice versa: each is resolved
against a different store lookup (`resolve_session` vs `find_sdk_key`), and a
mismatched token is rejected as `401 Unauthorized` rather than silently
downgraded.

### 2.1 SDK key kinds: server vs client

An SDK key additionally carries a `kind`: `server` or `client`. Client keys are
meant for SDKs embedded in untrusted runtimes (browsers, mobile apps); server
keys are meant for backend runtimes. `openapi.json` models both under the
single `sdkKey` security scheme because OpenAPI has no first-class way to
express "same scheme, but only one sub-kind is accepted here". The actual rule
is:

- `GET /sdk/whoami` and both OFREP evaluation endpoints accept **either** kind.
- `GET /sync/v1/ruleset` and `GET /sync/v1/events` accept **server keys only**.
  A client-kind key on either sync route gets `403 Forbidden` with a
  `problem+json` body explaining the requirement.

## 3. The sync contract: notify-then-fetch over SSE

`GET /sync/v1/events` is a server-sent events (`text/event-stream`) endpoint,
which OpenAPI 3.1 does not model well (no first-class streaming media type),
so its exact frame contract lives here instead of in the schema.

### 3.1 Notify-then-fetch, not push-the-data

Each SSE frame is a JSON-encoded `EventPayload`:

```json
{ "environment": "production", "version": 42 }
```

That is the entire payload: no flag data, no ruleset content, nothing beyond
which environment changed and its new version number. A subscriber that wants
the actual compiled ruleset must call `GET /sync/v1/ruleset` after receiving
the notification. This is a deliberate simplification of the transport: SSE
carries a cheap "something changed" signal, and the bulkier document travels
over an ordinary cacheable GET with ETag support.

### 3.2 Ordering invariant

Every event is emitted **after** the corresponding ruleset has been written to
the in-memory cache (inside `install_in_cache`). Concretely: the write to the
cache and the broadcast happen in that order, never the reverse. This gives a
subscriber a hard guarantee: if you receive an event announcing version `N`
and immediately call `GET /sync/v1/ruleset`, you will observe a ruleset whose
version is `>= N`. You can never observe a stale ruleset that is older than
the version just announced.

The reverse race is not eliminated, and does not need to be: a client that
calls `GET /sync/v1/ruleset` *before* subscribing to `/sync/v1/events` may miss
the notification for a version it already has (or briefly precede a
notification for a newer one). This is normal for any pull-then-subscribe
sequence and is why clients should also re-sync periodically, independent of
the event stream.

### 3.3 Keep-alive, filtering, and lag

- The connection sends periodic keep-alive comments so intermediaries do not
  time it out.
- Events are filtered server-side to the `(project, environment)` scope bound
  to the caller's SDK key; a subscriber never sees another environment's
  notifications.
- The event channel is a bounded broadcast buffer. A slow subscriber that
  falls behind has its lagged ticks silently skipped rather than the
  connection being torn down; skipping is not fatal because the client's
  periodic re-sync via `GET /sync/v1/ruleset` will catch up regardless of how
  many intermediate versions it missed.

### 3.4 Concurrency quota

`GET /sync/v1/events` opens a long-lived connection, so it is bounded
separately from the ordinary per-request token bucket, which only limits the
rate of new requests and does not bound resources held for the lifetime of a
stream: a compromised key, a reconnect storm, or a defective client could
otherwise hold an unbounded number of open connections.

Two ceilings apply, checked in this order:

1. A global ceiling, shared across every SDK key.
2. A per-key ceiling, scoped to the caller's own SDK key prefix.

Both are non-blocking: an over-quota request never queues, it is rejected
immediately with `429 Too Many Requests`, the same status, `Retry-After`
header, and `problem+json` body shape used by the ordinary rate limiter (see
section 5 and 6.1). This endpoint does not additionally apply the token
bucket, so a `429` from `/sync/v1/events` always means the concurrency quota,
never the request rate.

Clients should treat a `429` here as a signal to back off (e.g. full-jitter
exponential backoff on the reconnect loop) rather than reconnect immediately;
reconnecting in a tight loop only prolongs the quota being exhausted. A slot
frees as soon as any held connection closes, for any reason (client
disconnect, client-initiated cancellation, or server shutdown).

## 4. ETag and conditional requests

flaps uses strong ETags computed as the hex SHA-256 of the canonical
(key-sorted) JSON serialization of a resource. Canonical ordering matters
because some resources (`Flag.variants`, for instance) are backed by a
`HashMap` internally; without sorting, two serializations of the same logical
value could hash differently.

Three independent mechanisms use this ETag, and they are not applied
everywhere: optimistic-concurrency writes (4.1), a create-only guard (4.2),
and conditional reads on the data plane (4.3).

### 4.1 Optimistic concurrency: `If-Match` on writes

`PUT` and `DELETE` on Project, Environment, Flag, Segment and FlagEnvConfig all
accept an optional `If-Match` request header, evaluated per
[RFC 7232 §3.1](https://www.rfc-editor.org/rfc/rfc7232#section-3.1). When
present, the server compares it against the current resource's ETag
**atomically with the write** (see 4.4) before writing:

- Missing `If-Match`: no precondition, the write proceeds unconditionally.
- A single ETag that matches the current one: the write proceeds.
- A comma-separated list of ETags (e.g. `If-Match: "a", "b", "c"`): the write
  proceeds if *any* listed value matches the current ETag.
- `If-Match: *`: the write proceeds only if the resource currently exists.
- No match (including `*` against a resource that does not exist): `412
  Precondition Failed`, nothing is written. This applies even when the
  resource does not exist at all: an `If-Match` header (specific value or
  `*`) against a missing resource is a `412`, not a `404`.

All comparisons use the **strong** comparison function: every ETag this API
emits or accepts is strong (see the intro to section 4), so a weak-tagged
value (`W/"..."`) is never treated specially and simply never matches. Quoted
(`"abc123"`) and unquoted (`abc123`) forms are both accepted; surrounding
quotes are stripped before comparison.

This is the only conditional mechanism the admin CRUD routes support for
existing resources. Note in particular: the single-resource `GET` routes
(`GET /projects/{project}` and its siblings) always return `200` with an
`ETag` response header; they do **not** support `If-None-Match` / `304`.
Conditional reads are only implemented where the payload is large and polled
frequently (see 4.3).

### 4.2 Create-only guard: `If-None-Match: *` on writes

`PUT` on Project, Environment, Flag, Segment and FlagEnvConfig additionally
accepts an optional `If-None-Match` request header, evaluated per
[RFC 7232 §3.2](https://www.rfc-editor.org/rfc/rfc7232#section-3.2). Only the
`*` value is supported here (the general listed-ETags form of
`If-None-Match` exists to make `GET` conditional, which this write-side guard
has no use for):

- `If-None-Match: *`: the write proceeds only if the resource does **not**
  currently exist (a "create, never overwrite" guard). If it already exists,
  the request fails with `412 Precondition Failed` and nothing is written.
- Any other value, or the header absent: no precondition from this guard.

This is independent of `If-Match` (4.1): a request may carry either, both, or
neither. Carrying both is unusual but well-defined, since both are evaluated
against the same current-ETag lookup taken atomically with the write.

### 4.3 Conditional reads: `If-None-Match` on the data plane

`POST /ofrep/v1/evaluate/flags` (bulk evaluation) and `GET /sync/v1/ruleset`
both accept an optional `If-None-Match` request header, compared against the
current ruleset's content hash. An exact match short-circuits to
`304 Not Modified` with no body, which matters because SDK clients typically
poll these endpoints frequently and the compiled document can be large; a 304
avoids re-serializing and re-transferring it.

### 4.4 Atomicity and serialization of writes

Every admin `PUT`/`DELETE` above serializes against every other admin
`PUT`/`DELETE` targeting the same project (an in-process, per-project lock
held for the whole mutation: read the current resource, evaluate 4.1/4.2,
write, recompile affected environments, install into the ruleset cache).
Two consequences follow directly from this:

- **Atomic preconditions (4.1, 4.2)**: the ETag comparison and the write
  happen without any other in-scope mutation for the same project able to
  run in between, so two concurrent writes racing on the same `If-Match`
  value can never both succeed (a lost update): exactly one observes the
  ETag it expects and proceeds, the other observes the now-changed ETag and
  gets `412`.
- **A cache that is never stale relative to an acknowledged write**: the
  ruleset served by `GET /sync/v1/ruleset` and the OFREP evaluation routes is
  always recompiled from the store state committed by the last acknowledged
  write, never from a snapshot a differently-timed concurrent write could
  make stale. Two concurrent writes to different resources within the same
  environment (for example, two different flags' configs) both end up
  reflected in the ruleset once both requests complete.

This is an in-process guarantee: it holds across concurrent requests handled
by one `flapsd` instance, which is the only supported topology today (each
instance owns its own cache and event stream). It does not coordinate writes
issued by two separate `flapsd` processes sharing one database; that is a
tracked follow-up (database-level compare-and-swap) for a future
multi-instance deployment.

## 5. Custom response headers

| Header | Where | Meaning |
|---|---|---|
| `ETag` | Admin single-resource GET/PUT 200/201; OFREP bulk 200; sync ruleset 200 | Strong ETag of the returned resource, see section 4. |
| `X-Flaps-Version` | Sync ruleset 200 | Monotone version counter of the compiled ruleset, matches the `version` field a subsequent SSE `EventPayload` would announce. |
| `X-Flaps-Warning` | Project/Environment PUT 200/201, only when `managed_by` is `federated` | Warns that the edit may be overwritten by the next federation sync; Flag, Segment and FlagEnvConfig carry no `managed_by` field and never set this header. |
| `Retry-After` | Any `429` response | Seconds to wait before retrying: computed by the token-bucket rate limiter, or a fixed documented value for the `/sync/v1/events` concurrency quota (see 3.4). |

## 6. Errors

Two distinct error body shapes exist, depending on which world produced them.

### 6.1 Admin and sync errors: RFC 9457 `problem+json`

Every admin route and both `/sync/v1/*` routes report errors as
`application/problem+json`:

```json
{
  "type": "https://flaps.dev/problems/not-found",
  "title": "Resource not found",
  "status": 404,
  "detail": "The addressed resource does not exist."
}
```

All four fields are always present. `type` is a stable URI suffix identifying
the error category (`unauthorized`, `forbidden`, `invalid-body`,
`validation-error`, `not-found`, `conflict`, `precondition-failed`,
`too-many-requests`, `internal-error`); see `openapi.json`'s `Problem` schema
and each operation's declared response codes for which categories a given
route can produce.

Two categories are worth calling out because they are easy to conflate:

- `422 invalid-body`: the request failed structural or key-format validation
  (a path key is not valid kebab-case, or a path key does not match the body's
  key). This never touches the database.
- `400 validation-error`: the request is well-formed, but applying it would
  produce a ruleset that fails to compile (for example, a targeting rule
  referencing a segment key that does not exist). flaps validates every
  mutation by compiling it *before* writing, so a `400` here means the write
  was refused, not that a partially-applied change is sitting in the store.

### 6.2 OFREP errors: the OFREP 0.3.0 error shape

The two OFREP evaluation endpoints intentionally do **not** use
`problem+json`: they follow the [OFREP 0.3.0 protocol](https://github.com/open-feature/protocol)
error format instead, so that OFREP-compliant client SDKs (which expect this
shape) work against flaps unmodified. Single-flag evaluation errors carry the
evaluated `key` alongside `errorCode` / `errorDetails`; bulk evaluation
failures (which are not about one specific flag) omit `key`. See
`SingleErrorResponse` and `EvaluationFailureResponse` in `openapi.json`.

One consequence worth documenting explicitly: on the two OFREP endpoints, an
authentication failure is always reported as `401` regardless of its
underlying cause (a genuinely missing/invalid key, or an internal store
error while resolving it). This differs from every other authenticated route,
where an internal error while resolving credentials is reported as `500`
rather than folded into `401`. This is deliberate for OFREP: a third-party
OFREP client should never need to distinguish those cases.

## 7. Flag and flag-set metadata

`Flag` and `Environment` both carry an optional `metadata` object: arbitrary
keys mapping to a bare boolean, string or number (see the `Metadata` schema in
`openapi.json`). The field is optional on every admin request and response;
omitting it is equivalent to an empty map, and an empty map is never
serialized back (the field is absent, not `{}`).

At evaluation time, the two levels are merged into a single `metadata` object
on the OFREP response: flag-set (environment) metadata is the base, and flag
metadata is applied on top, so a key present at both levels resolves to the
flag's value. This mirrors flagd's own flag-set/flag metadata model. The
merged `metadata` field on `SingleSuccessResponse` (single and bulk
evaluation, since `BulkFlagEntry::Success` wraps `SingleSuccessResponse`) is
omitted entirely when the merge is empty, never emitted as `{}`.

## 8. Versioning note

The admin surface (`/projects/**`, `/sdk/whoami`, SDK key management) carries
no version prefix today. `/ofrep/v1` and `/sync/v1` carry `/v1` because that is
what OFREP and flaps's own sync channel expect, not because of an admin API
versioning decision. A dedicated backlog issue will settle how the admin
surface itself gets versioned (path prefix, a media-type parameter, or a
dedicated header) ahead of 1.0. Until that is decided, treat the current shape
of every admin endpoint as subject to change without a version bump.
