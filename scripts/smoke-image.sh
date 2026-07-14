#!/usr/bin/env bash
# Acceptance smoke test for the flapsd container image.
# Usage: bash scripts/smoke-image.sh <image-tag>
#
# Verifies the three acceptance criteria of issue #28:
#   1. the image runs as a non-root user (Config.User == flaps)
#   2. the OCI version and description labels are set
#   3. docker run with a mounted TOML starts flapsd on SQLite (HTTP responds)
set -euo pipefail

IMAGE="${1:?usage: smoke-image.sh <image-tag>}"
CONTAINER="flaps-smoke-$$"
WORKDIR="$(mktemp -d)"
VOLUME="flaps-smoke-data-$$"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  docker volume rm "$VOLUME" >/dev/null 2>&1 || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

# --- criterion 1: non-root user ---
USER_CFG="$(docker inspect --format '{{.Config.User}}' "$IMAGE")"
if [ "$USER_CFG" != "flaps" ]; then
  echo "FAIL: image user is '${USER_CFG}', expected 'flaps'"
  exit 1
fi
echo "OK: non-root user '${USER_CFG}'"

# --- criterion 2: OCI labels ---
VERSION_LABEL="$(docker inspect --format '{{index .Config.Labels "org.opencontainers.image.version"}}' "$IMAGE")"
DESC_LABEL="$(docker inspect --format '{{index .Config.Labels "org.opencontainers.image.description"}}' "$IMAGE")"
if [ -z "$VERSION_LABEL" ] || [ -z "$DESC_LABEL" ]; then
  echo "FAIL: missing OCI labels (version='${VERSION_LABEL}', description='${DESC_LABEL}')"
  exit 1
fi
echo "OK: labels version='${VERSION_LABEL}' description='${DESC_LABEL}'"

# --- criterion 3: runs on SQLite with a mounted TOML ---
cat > "$WORKDIR/flapsd.toml" <<'TOML'
database_url = "sqlite:///var/lib/flaps/flaps.db"
bind_addr    = "0.0.0.0:8080"
TOML

docker run -d --name "$CONTAINER" \
  -e FLAPS_HMAC_PEPPER=smoke-test-pepper-32-bytes-long! \
  -v "$WORKDIR/flapsd.toml:/etc/flaps/flapsd.toml:ro" \
  -v "$VOLUME:/var/lib/flaps" \
  -p 18080:8080 \
  "$IMAGE" >/dev/null

# Poll the HTTP surface for up to 30s. Any HTTP status (not 000) proves the
# server booted and is serving on the SQLite-backed store.
CODE="000"
for _ in $(seq 1 30); do
  CODE="$(curl -s -o /dev/null -w '%{http_code}' -X POST http://localhost:18080/login)" || CODE="000"
  if [ "$CODE" != "000" ]; then
    break
  fi
  sleep 1
done

if [ "$CODE" = "000" ]; then
  echo "FAIL: flapsd did not serve HTTP within 30s"
  docker logs "$CONTAINER" || true
  exit 1
fi
echo "OK: flapsd responded on SQLite (HTTP ${CODE})"

echo "SMOKE OK: ${IMAGE}"
