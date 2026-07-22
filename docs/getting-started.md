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

## Configuration

`flapsd.toml` accepts a few optional keys beyond `database_url` and `bind_addr`:

| Key | Default | Notes |
|-----|---------|-------|
| `admin_username` | `"admin"` | first-boot admin account |
| `rate_limit_per_minute` | `60` | SDK request budget per key, applied to both storage backends |
| `session_ttl_secs` | `86400` (24h) | admin session lifetime, minted by `POST /login` |
| `max_sse_subscriptions_per_key` | `5` | ceiling on concurrent `GET /sync/v1/events` subscriptions for a single SDK key |
| `max_sse_subscriptions_global` | `1000` | ceiling on concurrent `GET /sync/v1/events` subscriptions across every SDK key |

```toml
# flapsd.toml
database_url                    = "sqlite://flaps.db"
bind_addr                        = "127.0.0.1:8080"
rate_limit_per_minute            = 120
session_ttl_secs                 = 3600
max_sse_subscriptions_per_key   = 10
max_sse_subscriptions_global    = 2000
```

`rate_limit_per_minute`, `session_ttl_secs`, `max_sse_subscriptions_per_key`
and `max_sse_subscriptions_global` must all be greater than zero when set;
omit them to keep the defaults. A zero value fails configuration validation
at startup, before `flapsd` connects to the store. The effective values are
logged at startup; the database URL and HMAC pepper are not.

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

## Run with Docker

`flapsd` ships as a container image on Docker Hub (`nubster/flaps`). The image
runs as a non-root user and reads its configuration from a mounted TOML file.

Create a `flapsd.toml` that binds to all interfaces and stores its SQLite
database under the data volume:

```toml
database_url = "sqlite:///var/lib/flaps/flaps.db"
bind_addr    = "0.0.0.0:8080"
```

Then start the container, passing the HMAC pepper as an environment variable and
mounting the configuration read-only:

```bash
docker run --rm \
  -e FLAPS_HMAC_PEPPER=change-me-to-a-long-random-secret \
  -v "$PWD/flapsd.toml:/etc/flaps/flapsd.toml:ro" \
  -v flaps-data:/var/lib/flaps \
  -p 8080:8080 \
  nubster/flaps:latest
```

The daemon listens on port 8080. The `flaps-data` volume persists the SQLite
database across restarts. `FLAPS_HMAC_PEPPER` is required: the daemon refuses to
start without it.

## Local dev loop (Postgres)

SQLite stays the default and needs no external service: the SQLite setup shown
under "Run the server" above is still the shortest path, and nothing on this page
is required to build or test flaps. The two options below are optional
conveniences for contributors who want to run against PostgreSQL, the way CI
does.

Both start the same stack: PostgreSQL 16 and `flapsd` built from the local
Dockerfile, with the daemon reachable on `http://localhost:8080`.

### With Docker Compose

```bash
docker compose up --build
```

The first build compiles a release binary and takes a few minutes. Later runs
reuse the cache.

### With LightShuttle

[LightShuttle](https://github.com/nubster-opensources/lightshuttle) is an
optional dev orchestrator. Install it with `cargo install lightshuttle`, then
`lightshuttle.yml` starts the same stack in one command and adds its dashboard:

```bash
lightshuttle up
```

Both files pass `FLAPS_HMAC_PEPPER=dev-insecure-pepper-change-me`. This is a
placeholder for local development only. Never reuse it outside your machine:
production deployments must supply a long random secret of their own.

On first start `flapsd` prints the generated admin credentials once. Use them
with the admin API as shown above.
