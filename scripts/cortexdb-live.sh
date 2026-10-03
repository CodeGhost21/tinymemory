#!/usr/bin/env bash
# Boots the pinned CortexDB server (integration/cortexdb/) and runs the
# `cortexdb` engine's live tests against it, then tears it down.
#
#   ./scripts/cortexdb-live.sh            # boot, test, tear down
#   KEEP=1 ./scripts/cortexdb-live.sh     # leave the server running after
#   CORTEXDB_VERSION=v0.10.4 ./scripts/cortexdb-live.sh

set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
compose=(docker compose --project-name tinymemory-cortexdb -f "$root/integration/cortexdb/docker-compose.yml")
port="${CORTEXDB_PORT:-3141}"
url="http://127.0.0.1:$port"

cleanup() {
  result=$?
  if [ "$result" -ne 0 ]; then
    "${compose[@]}" logs cortex mock-inference | tail -80 || true
  fi
  if [ -z "${KEEP:-}" ]; then
    "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
  fi
  exit "$result"
}
trap cleanup EXIT

"${compose[@]}" up -d --build --wait mock-inference >/dev/null
"${compose[@]}" up -d cortex >/dev/null
for _ in $(seq 1 120); do
  if curl --fail --silent "$url/v1/admin/ready" >/dev/null; then
    break
  fi
  sleep 1
done
curl --fail --silent "$url/v1/admin/ready" >/dev/null || {
  echo "CortexDB did not become ready at $url" >&2
  exit 1
}
echo "CortexDB $(curl --silent "$url/v1/admin/health") at $url"

TINYMEMORY_LIVE_CORTEXDB_URL="$url" cargo test -p tinymemory-cortex --test live_cortexdb -- --nocapture
