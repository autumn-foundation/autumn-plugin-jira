#!/usr/bin/env bash
# Boots the real `autumn_app` example binary and checks it over HTTP, in the
# dev and prod profiles: GET /issues/OPS-1, a signed webhook POST (openssl
# HMAC) gives 200, a copy gives 409, a bad signature gives 401, and
# /actuator/health shows jira. Then it stops the app with SIGTERM. It also
# checks that a missing secret env var stops the boot and that the error
# names the variable. CI runs this script. Run it from any directory:
#
#   bash scripts/smoke-autumn-example.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
app_dir="$root/crates/autumn-plugin-jira/examples/autumn_app"
work=$(mktemp -d)
pid=""

cleanup() {
  if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    kill -KILL "$pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# Random secrets for this run. The checks make sure that the log does not
# show them. The webhook secret has 64 chars: the prod profile needs 32
# bytes or more. The prod profile also needs a signing secret and trusted
# hosts.
hook_secret=$(openssl rand -hex 32)
signing=$(openssl rand -hex 32)

# mask TEXT: TEXT with each secret of this run replaced by ***.
mask() {
  local text=$1
  text=${text//"$hook_secret"/***}
  text=${text//"$signing"/***}
  printf '%s\n' "$text"
}

fail() {
  mask "FAIL: $*" >&2
  if [ -f "$work/app.log" ]; then
    echo "--- app log ---" >&2
    mask "$(cat "$work/app.log")" >&2
  fi
  exit 1
}

cargo build --locked --quiet -p autumn-plugin-jira --features testing --example autumn_app
target=$(cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
bin="$target/debug/examples/autumn_app"

free_port() {
  python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])'
}

# start PROFILE [VAR=VALUE...]: starts the app with a clean env.
start() {
  local profile=$1
  shift
  port=$(free_port)
  base="http://127.0.0.1:$port"
  env -i PATH="$PATH" HOME="$HOME" \
    AUTUMN_ENV="$profile" \
    AUTUMN_MANIFEST_DIR="$app_dir" \
    AUTUMN_SERVER__PORT="$port" \
    AUTUMN_SERVER__PRESTOP_GRACE_SECS=0 \
    "$@" \
    "$bin" >"$work/app.log" 2>&1 &
  pid=$!
}

# healthy: waits up to 60 s for GET /. Returns 1 when the app stops.
healthy() {
  for _ in $(seq 1 600); do
    kill -0 "$pid" 2>/dev/null || return 1
    if curl -fsS --max-time 2 --noproxy '*' "$base/" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  fail "no reply in 60 s"
}

# boot PROFILE [VAR=VALUE...]: starts the app and waits for it. Another
# process can take the free port first, so it tries up to 3 ports.
boot() {
  for _ in 1 2 3; do
    start "$@"
    if healthy; then
      return 0
    fi
    pid=""
    grep -q "Address already in use" "$work/app.log" || fail "the app stopped at boot"
  done
  fail "no free port in 3 tries"
}

# expect STATUS CURL_ARGS... URL: sends one request and checks the status.
# The message shows only the URL, because the headers can hold a signature.
expect() {
  local want=$1
  shift
  local url=${*: -1}
  local got
  got=$(curl -s --max-time 10 -o "$work/body" -w '%{http_code}' --noproxy '*' "$@") \
    || fail "curl error for $url"
  [ "$got" = "$want" ] || fail "want $want, got $got for $url: $(cat "$work/body")"
}

# sign FILE: the X-Hub-Signature value of the body in FILE.
sign() {
  local hex
  hex=$(openssl dgst -sha256 -hmac "$hook_secret" -r "$1" | cut -d' ' -f1)
  printf 'sha256=%s' "$hex"
}

# stop_cleanly: sends SIGTERM and wants exit code 0 within 15 s.
stop_cleanly() {
  kill -TERM "$pid" 2>/dev/null || fail "the app stopped before SIGTERM"
  for _ in $(seq 1 150); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  kill -0 "$pid" 2>/dev/null && fail "no stop within 15 s of SIGTERM"
  local code=0
  wait "$pid" || code=$?
  pid=""
  [ "$code" -eq 0 ] || fail "exit code $code after SIGTERM"
}

no_secrets_in_log() {
  if grep -qF -e "$hook_secret" -e "$signing" "$work/app.log"; then
    fail "the log shows a secret"
  fi
}

for profile in dev prod; do
  echo "autumn_app: profile $profile"
  boot "$profile" JIRA_WEBHOOK_SECRET="$hook_secret" \
    AUTUMN_SECURITY__SIGNING_SECRET="$signing" \
    AUTUMN_SECURITY__TRUSTED_HOSTS__HOSTS=127.0.0.1
  expect 200 "$base/"
  grep -q "autumn_app" "$work/body" || fail "GET / does not name the app"
  expect 200 -H 'accept: application/json' "$base/issues/OPS-1"
  grep -q "Disk full" "$work/body" || fail "GET /issues/OPS-1 has no summary"
  expect 404 -H 'accept: application/json' "$base/issues/OPS-999"
  expect 400 -H 'accept: application/json' "$base/issues/not-a-key"

  # A signed delivery, as Jira sends it.
  now_ms=$(python3 -c 'import time; print(int(time.time() * 1000))')
  printf '{"timestamp":%s,"webhookEvent":"jira:issue_updated","user":{"accountId":"acct-ann"},"issue":{"id":"10000","key":"OPS-1","fields":{"summary":"Disk full on db-1"}}}' \
    "$now_ms" >"$work/hook.json"
  signature=$(sign "$work/hook.json")
  id="smoke-$profile-$now_ms"
  hook=(-H 'content-type: application/json' -H "X-Atlassian-Webhook-Identifier: $id" --data-binary "@$work/hook.json")
  expect 200 -H "X-Hub-Signature: $signature" "${hook[@]}" "$base/jira/webhook"
  expect 409 -H "X-Hub-Signature: $signature" "${hook[@]}" "$base/jira/webhook"
  expect 401 -H "X-Hub-Signature: sha256=$(printf '%064d' 0)" "${hook[@]}" "$base/jira/webhook"
  grep -q "jira event" "$work/app.log" || fail "the log sink did not log the event"

  expect 200 "$base/actuator/health"
  grep -q '"jira"' "$work/body" || fail "the health report has no jira"
  stop_cleanly
  no_secrets_in_log
done

# boot_fails NAME VAR=VALUE...: starts the real path (JIRA_SITE is set)
# with no API token. The boot must stop before any call to Jira with a
# non-zero exit, and the error must name the env var NAME.
boot_fails() {
  local name=$1
  shift
  start dev JIRA_WEBHOOK_SECRET="$hook_secret" JIRA_SITE=acme JIRA_EMAIL=smoke@example.com "$@"
  for _ in $(seq 1 300); do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  kill -0 "$pid" 2>/dev/null && fail "the app runs with no API token"
  local code=0
  wait "$pid" || code=$?
  pid=""
  [ "$code" -ne 0 ] || fail "exit code 0 with no API token"
  grep -q "$name" "$work/app.log" || fail "the error does not name the variable $name"
  no_secrets_in_log
}

echo "autumn_app: a missing JIRA_API_TOKEN stops the boot"
boot_fails JIRA_API_TOKEN

# The real path reads the profile file and the AUTUMN_JIRA__* env vars,
# as the fake path does. So the error names the env var of the override.
echo "autumn_app: the real path reads the AUTUMN_JIRA__ env vars"
boot_fails SMOKE_JIRA_TOKEN AUTUMN_JIRA__API_TOKEN__TOKEN_ENV=SMOKE_JIRA_TOKEN

# AUTUMN_ENV selects the profile file of the real path, as it selects the
# profile of autumn. A copy of the app dir gets autumn-staging.toml, which
# names another token env var. So the error must name that env var.
echo "autumn_app: AUTUMN_ENV selects the profile file of the real path"
cp -R "$app_dir" "$work/staging"
printf '[jira.api_token]\ntoken_env = "STAGING_JIRA_TOKEN"\n' >"$work/staging/autumn-staging.toml"
boot_fails STAGING_JIRA_TOKEN AUTUMN_ENV=staging AUTUMN_MANIFEST_DIR="$work/staging"

echo "autumn_app: OK"
