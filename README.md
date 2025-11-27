# Nubster Flaps

  **Feature flags. Simplified. Sovereign.**

  Open-source feature flags platform for progressive rollouts and A/B testing.
  Built in Rust. GDPR-native by design.

  [Features](#features) • [Quick Start](#quick-start) • [Documentation](#documentation) • [Contributing](#contributing)

  ![License](https://img.shields.io/badge/license-BSL--1.1-blue)
  ![Rust](https://img.shields.io/badge/rust-1.80+-orange)
  ![Status](https://img.shields.io/badge/status-alpha-yellow)

  ---

## Why Flaps?

  **Flaps** (like aircraft control surfaces) gives you precise control over feature deployment. Ship confidently with gradual rollouts, instant kill switches, and powerful targeting.

- **Local evaluation** — SDKs evaluate flags locally for sub-millisecond latency
- **Real-time sync** — Server-Sent Events keep all instances in sync
- **Self-hosted or cloud** — Deploy anywhere: on-premise, private cloud, or managed SaaS
- **Compliance-ready** — Designed for GDPR, SOC 2, and European data sovereignty

## Features

### Feature Flags

- **Boolean flags** — Simple on/off toggles
- **String variants** — A/B testing with multiple variants
- **Kill switch** — Instant emergency disable with cooldown protection
- **Scheduled changes** — Plan flag changes in advance

### Targeting

- **User attributes** — Target by plan, country, version, or custom attributes
- **Segments** — Reusable groups with inclusion/exclusion lists
- **Percentage rollout** — Gradual rollout with stable bucketing (murmur3)
- **Rules priority** — Ordered rules with first-match semantics

### Environments

- **Multi-environment** — dev, staging, prod with independent configs
- **Approval workflow** — Require approval for production changes
- **Environment comparison** — Diff flags across environments
- **Preview environments** — Auto-provision for branch deployments

### Operations

- **Audit log** — Track every change with user attribution
- **Rollback** — One-click revert to any previous state
- **Stale flag detection** — Identify flags that should be cleaned up
- **Import/Export** — Bulk operations and migration tools

## Quick Start

### Using Docker

  ```bash
  docker run -d --name flaps \
    -p 8300:8300 \
    -e FLAPS_DEV_MODE=true \
    nubster/flaps:latest
  ```

### Using the CLI

  ```bash
  # Create a project
  flaps project create my-app

  # Create a flag
  flaps flag create new-checkout --type boolean --project my-app

  # Enable in dev
  flaps flag toggle new-checkout --env dev --enable

  # Evaluate a flag
  flaps eval new-checkout --env dev --user-id user-123
  ```

### Using the SDK (Rust)

  ```rust
  use flaps_sdk::{FlapsClient, Config, EvaluationContext};

  #[tokio::main]
  async fn main() {
      let client = FlapsClient::new(Config {
          api_key: "your-api-key".to_string(),
          environment: "prod".to_string(),
          ..Default::default()
      }).await.unwrap();

      let context = EvaluationContext::with_user_id("user-123")
          .set("plan", "pro")
          .set("country", "FR");

      if client.is_enabled("new-checkout", &context).await {
          // New checkout flow
      } else {
          // Old checkout flow
      }
  }
  ```

## SDKs

  Official SDKs for seamless integration:

  | Language   | Package                | Status       |
  |------------|------------------------|--------------|
  | Rust       | `flaps-sdk`            | In progress  |
  | .NET       | `Nubster.Flaps.SDK`    | Coming soon  |
  | TypeScript | `@nubster/flaps`       | Coming soon  |
  | Python     | `flaps-sdk`            | Coming soon  |
  | Go         | `github.com/nubster/flaps-go` | Coming soon  |

## Deployment Options

  | Mode              | Description                                                        |
  |-------------------|--------------------------------------------------------------------|
  | **Flaps Cloud**   | Managed SaaS at [flaps.nubster.com](https://flaps.nubster.com)     |
  | **Self-hosted**   | Deploy on your infrastructure (Docker, Kubernetes, bare metal)     |
  | **Nubster Platform** | Integrated with [Nubster Workspace](https://www.nubster.com)    |

## Architecture

  ``` text
                      ┌─────────────────────┐
                      │    REST API + SSE   │
                      └──────────┬──────────┘
                                 │
      ┌──────────────────────────┼──────────────────────────┐
      │                    FLAPS SERVER                      │
      │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐ │
      │  │  Flags  │  │Segments │  │ Rules   │  │ Audit   │ │
      │  │ Engine  │  │ Engine  │  │ Engine  │  │ Engine  │ │
      │  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘ │
      │       └────────────┴────────────┴────────────┘      │
      │                         │                           │
      │                  ┌──────┴──────┐                    │
      │                  │ Evaluation  │                    │
      │                  │   Engine    │                    │
      │                  └─────────────┘                    │
      └──────────────────────────┬──────────────────────────┘
                                 │
                      ┌──────────┴──────────┐
                      │   Storage Backend   │
                      │ (PostgreSQL + Redis)│
                      └─────────────────────┘
  ```

## Workspace Structure

  ``` text
  Tenant (Organization)
     └── Groups (optional)
           └── Projects
                 └── Environments (dev, staging, prod)
                       └── Flags
  ```

## Pricing

  | Plan          | Price           | Features                                    |
  |---------------|-----------------|---------------------------------------------|
  | **Community** | Free            | Unlimited flags, 3 environments, 1 project |
  | **Pro**       | €10/seat/month  | Unlimited projects, SSO, audit log          |
  | **Enterprise**| Custom          | On-premise, SLA, dedicated support          |
  | **Workspace Add-on** | +€2/seat/month | For existing Nubster Workspace users  |

## Documentation

- [Getting Started](docs/getting-started.md)
- [Architecture Overview](docs/architecture/overview.md)
- [API Reference](docs/api/README.md)
- [SDK Guide](docs/sdk/README.md)
- [Targeting Rules](docs/targeting/README.md)

## Contributing

  We welcome contributions! Please read our [Contributing Guide](CONTRIBUTING.md) before submitting a pull request.

## License

  Nubster Flaps is licensed under the [Business Source License 1.1](LICENSE).

- **Permitted**: Internal use, development, testing, non-commercial use
- **Not Permitted**: Offering as a commercial managed service without a license
- **Change Date**: 4 years from release
- **Change License**: Apache License 2.0

## About Nubster

  Flaps is part of the [Nubster](https://www.nubster.com) ecosystem — a GDPR-native, AI-powered development suite for European teams.

  ---

  Made with love in France
