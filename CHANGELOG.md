# Changelog

All notable changes to Nubster Flaps will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial project structure
- Core domain types and evaluation engine (`flaps-core`)
  - Feature flags with Boolean and String variants
  - Targeting rules with multiple operators
  - Segment-based targeting with inclusion/exclusion lists
  - Murmur3-based stable rollout percentage
  - Multi-environment configuration
  - Evaluation context with user attributes
- Storage abstraction layer (`flaps-storage`)
- HTTP server skeleton with Axum (`flaps-server`)
- CLI tool structure (`flaps-cli`)
- Rust SDK with offline mode support (`flaps-sdk`)

### Technical
- Rust workspace with 5 crates
- 26 unit tests passing
- UUIDv7 for time-ordered identifiers
