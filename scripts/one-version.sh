#!/usr/bin/env bash
# Fails when the lock of a manifest holds two versions of a shared crate.
# Two versions give types that do not match. CI runs this script on the
# committed locks, and again after the weekly `cargo update`.
#
#   bash scripts/one-version.sh [<Cargo.toml>...]
#
# With no argument, it checks the root Cargo.toml.
set -euo pipefail

shared='^(autumn-web|san-mateo|autumn-harvest|camel-api|camel-core|camel-component-api) v'
status=0
for manifest in "${@:-Cargo.toml}"; do
  out=$(cargo tree --locked -e normal -d --manifest-path "$manifest")
  if printf '%s\n' "$out" | grep -E "$shared"; then
    echo "::error::$manifest: two versions of a crate above"
    status=1
  fi
done
exit "$status"
