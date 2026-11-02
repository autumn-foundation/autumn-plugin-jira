#!/usr/bin/env bash
# Runs the ecosystem example (examples/ecosystem). The example needs
# Postgres. When DATABASE_URL is not set, this script starts Postgres 16
# with Docker Compose (examples/ecosystem/compose.yaml) and uses it. The
# container keeps running after the script, so the next run is fast. Stop it
# with:
#
#   docker compose -f examples/ecosystem/compose.yaml down
#
# Run it from any directory:
#
#   bash scripts/run-ecosystem.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
compose_file="$root/examples/ecosystem/compose.yaml"

if [ -z "${DATABASE_URL:-}" ]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "run-ecosystem: set DATABASE_URL to a Postgres URL, or install Docker" >&2
    exit 1
  fi
  echo "run-ecosystem: DATABASE_URL is not set; starting Postgres with Docker Compose" >&2
  docker compose -f "$compose_file" up -d --wait
  export DATABASE_URL="postgres://jira:jira@127.0.0.1:5432/jira_test"
fi

exec cargo run --locked --manifest-path "$root/examples/ecosystem/Cargo.toml"
