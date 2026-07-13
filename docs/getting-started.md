# Getting started

> Pre-alpha: not released yet, build `flapsd` from source (see the root README Quick start). Flags are managed entirely through the admin REST API in v0.1.0; the admin UI ships in v0.2.

## Run the server

```bash
export FLAPS_HMAC_PEPPER=<long-random-secret>
```

```toml
# flapsd.toml
database_url = "sqlite://flaps.db"
bind_addr    = "127.0.0.1:8080"
```

```bash
# SQLite needs no external service; PostgreSQL is supported for production
flapsd --config flapsd.toml
```

On first start `flapsd` bootstraps an admin account and prints its credentials once.

## Create a flag through the admin API

Log in with the printed credentials to get a session token, create the project the flag lives in, then create the flag itself.

```bash
curl -s -X POST http://localhost:8080/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "<printed-password>"}'
# -> {"token": "...", "expires_at": "..."}; export it below.

export ADMIN_TOKEN=<token from the response above>

curl -X PUT http://localhost:8080/projects/my-app \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"key": "my-app", "name": "My App", "managed_by": "local"}'

curl -X PUT http://localhost:8080/projects/my-app/flags/new-dashboard \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"key": "new-dashboard", "name": "New dashboard", "flag_type": "release", "value_type": "boolean", "variants": {"value_type": "boolean", "entries": {"on": {"bool": true}, "off": {"bool": false}}}}'
```

See [the HTTP API reference](spec/api-v1.md) for the full authentication model, ETag semantics and error format.

## Evaluate from any OpenFeature SDK (remote, OFREP)

Point the generic OFREP provider of your OpenFeature SDK at the Flaps server with an environment SDK key. No proprietary SDK is required.

## Evaluate in-process (Rust)

```rust
// flaps-client downloads the compiled ruleset, keeps it fresh over SSE
// and evaluates locally. See the crate documentation once published.
```

## Kill switch

Disabling a flag in the admin API propagates to connected in-process clients in under two seconds. Clients that miss the notification converge through their backup polling interval.
