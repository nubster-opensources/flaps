# syntax=docker/dockerfile:1

# =============================================================================
# Stage chef: shared base with cargo-chef installed
# =============================================================================
FROM rust:1.94-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# =============================================================================
# Stage planner: derive the dependency recipe from the whole workspace
# =============================================================================
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# =============================================================================
# Stage builder: cook cached dependencies, then build the flapsd binary
# =============================================================================
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --bin flapsd

# =============================================================================
# Stage runtime: minimal debian-slim, non-root
# =============================================================================
FROM debian:bookworm-slim AS runtime

ARG VERSION=0.1.0
LABEL org.opencontainers.image.title="flaps"
LABEL org.opencontainers.image.description="Sovereign feature flag server (flapsd)"
LABEL org.opencontainers.image.version="${VERSION}"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --system --create-home --shell /usr/sbin/nologin flaps
RUN mkdir -p /etc/flaps /var/lib/flaps \
    && chown -R flaps:flaps /etc/flaps /var/lib/flaps

COPY --from=builder /app/target/release/flapsd /usr/local/bin/flapsd

USER flaps
WORKDIR /home/flaps

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/flapsd"]
CMD ["--config", "/etc/flaps/flapsd.toml"]
