.PHONY: all build check test lint fmt clean release docker help
.DEFAULT_GOAL := help

# ============================================================================
# Variables
# ============================================================================

CARGO := cargo
DOCKER := docker
DOCKER_COMPOSE := docker compose

# Build configuration
RELEASE_FLAGS := --release --locked
TARGET_DIR := target

# Docker configuration
DOCKER_IMAGE := nubster/flaps
DOCKER_TAG := latest

# ============================================================================
# Development
# ============================================================================

## Build all crates in debug mode
build:
	$(CARGO) build

## Check code compiles without building
check:
	$(CARGO) check --all-targets

## Run all tests
test:
	$(CARGO) test --all-features

## Run tests with output
test-verbose:
	$(CARGO) test --all-features -- --nocapture

## Run clippy linter
lint:
	$(CARGO) clippy --all-targets --all-features -- -D warnings

## Format code
fmt:
	$(CARGO) fmt --all

## Check formatting
fmt-check:
	$(CARGO) fmt --all -- --check

## Run all quality checks (format, lint, test, audit)
ci: fmt-check lint test audit deny

# ============================================================================
# Release
# ============================================================================

## Build release binaries
release:
	$(CARGO) build $(RELEASE_FLAGS)

## Build server binary only
release-server:
	$(CARGO) build $(RELEASE_FLAGS) -p flaps-server

## Build CLI binary only
release-cli:
	$(CARGO) build $(RELEASE_FLAGS) -p flaps-cli

## Build SDK only
release-sdk:
	$(CARGO) build $(RELEASE_FLAGS) -p flaps-sdk

# ============================================================================
# Security
# ============================================================================

## Run security audit
audit:
	$(CARGO) audit

## Check dependencies (advisories, bans, sources)
deny:
	$(CARGO) deny check advisories bans sources

## Run all security checks
security: audit deny

## Install security tools
security-install:
	$(CARGO) install cargo-audit --locked
	$(CARGO) install cargo-deny@0.18.3 --locked

# ============================================================================
# Documentation
# ============================================================================

## Generate documentation
doc:
	$(CARGO) doc --all-features --no-deps

## Generate and open documentation
doc-open:
	$(CARGO) doc --all-features --no-deps --open

# ============================================================================
# Docker
# ============================================================================

## Build Docker image
docker-build:
	$(DOCKER) build -t $(DOCKER_IMAGE):$(DOCKER_TAG) .

## Run with Docker Compose (development)
docker-dev:
	$(DOCKER_COMPOSE) -f deploy/docker/docker-compose.dev.yml up --build

## Run with Docker Compose (production)
docker-prod:
	$(DOCKER_COMPOSE) -f deploy/docker/docker-compose.yml up -d

## Stop Docker Compose
docker-stop:
	$(DOCKER_COMPOSE) down

## Clean Docker resources
docker-clean:
	$(DOCKER_COMPOSE) down -v --rmi local

# ============================================================================
# Database
# ============================================================================

## Run database migrations
db-migrate:
	$(CARGO) sqlx migrate run

## Create a new migration
db-migration:
	@read -p "Migration name: " name; \
	$(CARGO) sqlx migrate add $$name

## Reset database
db-reset:
	$(CARGO) sqlx database reset -y

## Prepare offline sqlx data
db-prepare:
	$(CARGO) sqlx prepare --workspace

# ============================================================================
# Server
# ============================================================================

## Run the server in development mode
run:
	$(CARGO) run -p flaps-server

## Run the server with hot reload
run-watch:
	$(CARGO) watch -x 'run -p flaps-server'

## Run the CLI
cli:
	$(CARGO) run -p flaps-cli -- $(ARGS)

# ============================================================================
# Utilities
# ============================================================================

## Clean build artifacts
clean:
	$(CARGO) clean

## Update dependencies
update:
	$(CARGO) update

## Show dependency tree
deps:
	$(CARGO) tree

## Show outdated dependencies
outdated:
	$(CARGO) outdated -R

## Generate Cargo.lock
lock:
	$(CARGO) generate-lockfile

## Install development tools
tools:
	$(CARGO) install cargo-audit --locked
	$(CARGO) install cargo-deny@0.18.3 --locked
	$(CARGO) install cargo-watch --locked
	$(CARGO) install cargo-outdated --locked
	$(CARGO) install sqlx-cli --locked

## Watch and rebuild on changes
watch:
	$(CARGO) watch -x check

## Watch and run tests on changes
watch-test:
	$(CARGO) watch -x test

# ============================================================================
# SDK Development
# ============================================================================

## Run SDK tests only
test-sdk:
	$(CARGO) test -p flaps-sdk --all-features

## Run core tests only
test-core:
	$(CARGO) test -p flaps-core --all-features

## Check SDK documentation
doc-sdk:
	$(CARGO) doc -p flaps-sdk --all-features --no-deps --open

# ============================================================================
# Benchmarks (future)
# ============================================================================

## Run benchmarks
bench:
	$(CARGO) bench

## Run evaluation engine benchmarks
bench-eval:
	$(CARGO) bench -p flaps-core -- evaluate

# ============================================================================
# Help
# ============================================================================

## Show this help message
help:
	@echo "Nubster Flaps - Makefile Commands"
	@echo ""
	@echo "Usage: make [target]"
	@echo ""
	@echo "Development:"
	@echo "  build          Build all crates in debug mode"
	@echo "  check          Check code compiles without building"
	@echo "  test           Run all tests"
	@echo "  test-verbose   Run tests with output"
	@echo "  test-core      Run core crate tests only"
	@echo "  test-sdk       Run SDK crate tests only"
	@echo "  lint           Run clippy linter"
	@echo "  fmt            Format code"
	@echo "  fmt-check      Check formatting"
	@echo "  ci             Run all CI checks (format, lint, test, audit, deny)"
	@echo ""
	@echo "Release:"
	@echo "  release        Build release binaries"
	@echo "  release-server Build server binary only"
	@echo "  release-cli    Build CLI binary only"
	@echo "  release-sdk    Build SDK only"
	@echo ""
	@echo "Security:"
	@echo "  audit          Run security audit (vulnerabilities)"
	@echo "  deny           Check dependencies (advisories, bans, sources)"
	@echo "  security       Run all security checks"
	@echo "  security-install Install security tools"
	@echo ""
	@echo "Documentation:"
	@echo "  doc            Generate documentation"
	@echo "  doc-open       Generate and open documentation"
	@echo "  doc-sdk        Generate and open SDK documentation"
	@echo ""
	@echo "Server:"
	@echo "  run            Run the server in development mode"
	@echo "  run-watch      Run the server with hot reload"
	@echo "  cli            Run the CLI (use ARGS='...' for arguments)"
	@echo ""
	@echo "Database:"
	@echo "  db-migrate     Run database migrations"
	@echo "  db-migration   Create a new migration"
	@echo "  db-reset       Reset database"
	@echo "  db-prepare     Prepare offline sqlx data"
	@echo ""
	@echo "Docker:"
	@echo "  docker-build   Build Docker image"
	@echo "  docker-dev     Run with Docker Compose (development)"
	@echo "  docker-prod    Run with Docker Compose (production)"
	@echo "  docker-stop    Stop Docker Compose"
	@echo "  docker-clean   Clean Docker resources"
	@echo ""
	@echo "Utilities:"
	@echo "  clean          Clean build artifacts"
	@echo "  update         Update dependencies"
	@echo "  deps           Show dependency tree"
	@echo "  outdated       Show outdated dependencies"
	@echo "  lock           Generate Cargo.lock"
	@echo "  tools          Install development tools"
	@echo "  watch          Watch and rebuild on changes"
	@echo "  watch-test     Watch and run tests on changes"
	@echo ""
