# Getting started

> Pre-alpha. Nothing below works end to end yet. This page illustrates the experience targeted by v0.1.0, which is headless: flags are managed through the admin REST API. The admin UI ships in v0.2.

## Run the server

```bash
# SQLite needs no external service; PostgreSQL is supported for production
flapsd --config flaps.toml
```

On first start `flapsd` bootstraps an admin account and prints its credentials once.

## Create a flag through the admin API

```bash
curl -X POST http://localhost:8080/api/admin/v1/projects/my-app/flags \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -d '{"key": "new-dashboard", "flag_type": "release", "value_type": "boolean", "variants": {"on": true, "off": false}}'
```

## Evaluate from any OpenFeature SDK (remote, OFREP)

Point the generic OFREP provider of your OpenFeature SDK at the Flaps server with an environment SDK key. No proprietary SDK is required.

## Evaluate in-process (Rust)

```rust
// flaps-client downloads the compiled ruleset, keeps it fresh over SSE
// and evaluates locally. See the crate documentation once published.
```

## Kill switch

Disabling a flag in the admin API propagates to connected in-process clients in under two seconds. Clients that miss the notification converge through their backup polling interval.
