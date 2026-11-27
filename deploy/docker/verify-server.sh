#!/bin/sh
set -u

# In-container acceptance for the production image (M3 exit criteria).
#
# Runs as the container user (10001:10001), never as root, and exercises the real
# HTTP API end to end: health, login, forced password change, profile creation,
# a headful launch under Xvfb, stop, relaunch, final stop. Every step prints one
# `PASS`/`FAIL` line; the exit status is 0 only when nothing failed.
#
# It deliberately does not fake any result:
# - `/readyz` 503 is recorded as a value, not converted into a failure.
# - `isolation` is printed exactly as the API returns it.
# - The kernel process is proven from `/proc`, not only from the API state.
#
# A new profile is headful and direct, so the server must be started with
# `MIEN_ALLOW_DIRECT_EGRESS=1`; without it a start correctly returns
# `403 DIRECT_EGRESS_DISABLED`, and this script reports that instead of passing.
#
# Usage (inside the container):
#   verify-server.sh
# Environment:
#   MIEN_PUBLIC_URL          needed for write requests: a Cookie write whose
#                            Origin differs from this value is rejected.
#   MIEN_VERIFY_BASE         default http://127.0.0.1:8787
#   MIEN_VERIFY_NEW_PASSWORD default below; on a reused volume the stored
#                            password is already this value.

BASE="${MIEN_VERIFY_BASE:-http://127.0.0.1:${MIEN_HEALTHCHECK_PORT:-8787}}"
PUBLIC_URL="${MIEN_PUBLIC_URL:-}"
DATA_ROOT="${MIEN_DATA_ROOT:-/data}"
BOOTSTRAP_FILE="$DATA_ROOT/secrets/admin-bootstrap"
NEW_PASSWORD="${MIEN_VERIFY_NEW_PASSWORD:-verify-correct-horse-battery-staple}"
PROFILE_NAME="${MIEN_VERIFY_PROFILE_NAME:-acceptance-$$}"
START_TIMEOUT="${MIEN_VERIFY_START_TIMEOUT:-120}"

FAILURES=0
COOKIE=
CSRF=
PROFILE_ID=
RUNNING_PID=
RUNTIME_BODY='{}'

say() { printf '%s\n' "$*"; }
pass() { say "PASS $1"; }
fail() {
  say "FAIL $1"
  FAILURES=$((FAILURES + 1))
}

# Assert that `$2` equals `$3` and report accordingly.
expect_equal() {
  if [ "$2" = "$3" ]; then
    pass "$1: $2"
  else
    fail "$1: expected '$3', got '$2'"
  fi
}

need() {
  command -v "$1" >/dev/null 2>&1 || {
    fail "missing_tool: $1"
    say "RESULT FAIL (missing required tool $1)"
    exit 1
  }
}
need curl
need jq
need awk
need ps
need od

# HTTP status code for a plain GET; `000` when no response arrived.
status_of() {
  curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$1" 2>/dev/null || printf '000'
}

# Authenticated API call. `$1` method, `$2` path, `$3` optional JSON body.
# Output is the response body followed by a final status-code line.
api() {
  method=$1
  path=$2
  body=${3:-}
  set -- -sS --max-time 30 -X "$method" "$BASE$path" \
    -H "Cookie: mien_session=$COOKIE" \
    -H "x-csrf-token: $CSRF" \
    -H "Accept: application/json" \
    -w '\n%{http_code}'
  if [ -n "$body" ]; then
    set -- "$@" -H 'Content-Type: application/json' --data-binary "$body"
  fi
  if [ -n "$PUBLIC_URL" ]; then
    set -- "$@" -H "Origin: $PUBLIC_URL"
  fi
  curl "$@"
}

body_of() { printf '%s' "$1" | sed '$d'; }
status_of_response() { printf '%s' "$1" | tail -n 1; }

# `GET /runtime` -> JSON; empty when the request failed.
runtime_json() {
  api GET "/api/v1/profiles/$PROFILE_ID/runtime" | sed '$d'
}

# ---------------------------------------------------------------------------
# 1. Health: liveness 200, readiness 200 or 503 (both in the contract)
# ---------------------------------------------------------------------------
say '== health =='
expect_equal 'healthz_status' "$(status_of "$BASE/healthz")" '200'

readiness=$(status_of "$BASE/readyz")
case "$readiness" in
  200) pass 'readyz_status: 200 (ready)' ;;
  503) pass 'readyz_status: 503 (documented not-ready answer; not a probe failure)' ;;
  *) fail "readyz_status: expected 200 or 503, got '$readiness'" ;;
esac
say "readyz_body=$(curl -sS --max-time 10 "$BASE/readyz" 2>/dev/null || echo unavailable)"

# Static UI must be served by the same origin as the API.
expect_equal 'ui_index_status' "$(status_of "$BASE/")" '200'
if curl -sS --max-time 10 "$BASE/" 2>/dev/null | grep -q 'id="root"'; then
  pass 'ui_index_contains_mount_point'
else
  fail 'ui_index_contains_mount_point'
fi

# ---------------------------------------------------------------------------
# 2. Login, then rotate the bootstrap password
# ---------------------------------------------------------------------------
say '== auth =='
# Two legitimate states, both handled explicitly:
# - First start on an empty volume: the server generated `admin-bootstrap`, and
#   the forced change below rotates it.
# - Later start on the same volume: that file is gone because the change already
#   happened, so the rotated password is the only way in.
# Three legitimate states, each handled explicitly instead of guessed:
# - generated bootstrap file present -> forced change below;
# - no file, but the operator supplied MIEN_VERIFY_PASSWORD (already-rotated
#   volume) -> use it;
# - no file and no supplied password -> the volume was initialised by an earlier
#   run whose password nobody recorded. That is not a failure of the image, so
#   report the precise blocker and exit without claiming a pass or a fail.
SUPPLIED_PASSWORD="${MIEN_VERIFY_PASSWORD:-}"
if [ -f "$BOOTSTRAP_FILE" ]; then
  mode=$(stat -c '%a' "$BOOTSTRAP_FILE" 2>/dev/null || stat -f '%Lp' "$BOOTSTRAP_FILE")
  expect_equal 'bootstrap_secret_mode' "$mode" '600'
  CURRENT_PASSWORD=$(cat "$BOOTSTRAP_FILE")
  EXPECT_FORCED_CHANGE='true'
  say 'bootstrap_source=generated_file'
elif [ -n "$SUPPLIED_PASSWORD" ]; then
  CURRENT_PASSWORD="$SUPPLIED_PASSWORD"
  EXPECT_FORCED_CHANGE='false'
  say 'bootstrap_source=operator_supplied'
else
  say "BLOCKED bootstrap_secret_missing: $BOOTSTRAP_FILE"
  say 'BLOCKED detail=volume was initialised by an earlier run; its rotated password is unknown'
  say 'BLOCKED fix=run with MIEN_VERIFY_PASSWORD=<the password that run set>, or start on an empty volume'
  say 'RESULT BLOCKED (authentication cannot be established; no assertion was skipped)'
  exit 3
fi

# The session cookie is read from `Set-Cookie`, not from curl's cookie jar: in
# server mode the cookie is `Secure`, and a jar may refuse to store a Secure
# cookie for a plain `http://` origin. `api()` sends the value back explicitly.
login=$(curl -sS --max-time 30 -D /tmp/mien-verify-headers.txt -X POST "$BASE/api/v1/auth/login" \
  -H 'Content-Type: application/json' -H 'Accept: application/json' \
  -w '\n%{http_code}' \
  --data-binary "$(jq -nc --arg password "$CURRENT_PASSWORD" '{password: $password}')")
login_status=$(status_of_response "$login")
login_body=$(body_of "$login")
if [ "$login_status" = '200' ]; then
  pass 'login_status: 200'
else
  fail "login_status: expected 200, got '$login_status'"
  say "login_body=$login_body"
  say 'RESULT FAIL'
  exit 1
fi

COOKIE=$(tr -d '\r' </tmp/mien-verify-headers.txt \
  | awk '/^[Ss][Ee][Tt]-[Cc][Oo][Oo][Kk][Ii][Ee]: mien_session=/ { sub(/^[^=]*=/, ""); sub(/;.*/, ""); print }' \
  | tail -n 1)
CSRF=$(printf '%s' "$login_body" | jq -r '.csrf_token')
if [ -n "$COOKIE" ] && [ -n "$CSRF" ] && [ "$CSRF" != 'null' ]; then
  pass "session_cookie_and_csrf_received cookie_len=${#COOKIE} csrf_len=${#CSRF}"
else
  fail 'session_cookie_and_csrf_received'
  say 'RESULT FAIL'
  exit 1
fi
expect_equal 'login_user' "$(printf '%s' "$login_body" | jq -r '.user')" 'admin'
expect_equal 'must_change_password' \
  "$(printf '%s' "$login_body" | jq -r '.must_change_password')" "$EXPECT_FORCED_CHANGE"

if [ "$CURRENT_PASSWORD" = "$NEW_PASSWORD" ]; then
  # An earlier run already rotated to this value; demanding a change from that
  # value to itself would be rejected as "must differ from the current password".
  pass 'password_change_skipped_already_rotated: yes'
else
  change=$(api POST /api/v1/auth/password \
    "$(jq -nc --arg current "$CURRENT_PASSWORD" --arg new "$NEW_PASSWORD" \
      '{current_password: $current, new_password: $new}')")
  change_status=$(status_of_response "$change")
  expect_equal 'password_change_status' "$change_status" '204'
  if [ "$change_status" != '204' ]; then
    say "password_change_body=$(body_of "$change")"
    say 'RESULT FAIL'
    exit 1
  fi
fi

# The session survives the change, but `must_change_password` must now be false.
session_info=$(api GET /api/v1/auth/session)
expect_equal 'must_change_password_after_change' \
  "$(body_of "$session_info" | jq -r '.must_change_password')" 'false'

# ---------------------------------------------------------------------------
# 3. Create a profile with a complete fingerprint
# ---------------------------------------------------------------------------
say '== profile =='
fingerprint=$(cat <<'JSON'
{
  "schema_version": 1,
  "platform": "linux",
  "platform_version": "6.8.0",
  "brand": "Edge",
  "brand_version": "148.0.0.0",
  "hardware_concurrency": 12,
  "languages": ["en-US", "en"],
  "timezone": "UTC",
  "window": {"width": 1280, "height": 720},
  "webrtc": {"disable_non_proxied_udp": true},
  "network": {"doh_mode": "disabled", "ipv6_mode": "block"}
}
JSON
)
create_body=$(jq -nc --arg name "$PROFILE_NAME" --argjson fingerprint "$fingerprint" \
  '{name: $name, mode: "headful", fingerprint: $fingerprint}')
create=$(api POST /api/v1/profiles "$create_body")
create_status=$(status_of_response "$create")
create_json=$(body_of "$create")
if [ "$create_status" = '201' ]; then
  PROFILE_ID=$(printf '%s' "$create_json" | jq -r '.id')
  pass "profile_create_status: 201 id=$PROFILE_ID"
else
  fail "profile_create_status: expected 201, got '$create_status'"
  say "profile_create_body=$create_json"
  say 'RESULT FAIL'
  exit 1
fi
expect_equal 'profile_mode' "$(printf '%s' "$create_json" | jq -r '.mode')" 'headful'
expect_equal 'profile_isolation_before_start' \
  "$(printf '%s' "$create_json" | jq -r '.isolation')" 'unknown'

# ---------------------------------------------------------------------------
# 4/5. Start headful, wait for `running`, prove the kernel from /proc
# ---------------------------------------------------------------------------
# Poll the runtime until it reaches the wanted state or a terminal failure.
wait_for_state() {
  wanted=$1
  waited=0
  RUNTIME_BODY='{}'
  while [ "$waited" -lt "$START_TIMEOUT" ]; do
    RUNTIME_BODY=$(runtime_json)
    state=$(printf '%s' "$RUNTIME_BODY" | jq -r '.state // empty' 2>/dev/null)
    if [ "$state" = "$wanted" ]; then
      return 0
    fi
    case "$state" in
      failed | crashed)
        return 1
        ;;
    esac
    sleep 1
    waited=$((waited + 1))
  done
  return 1
}

start_once() {
  label=$1
  response=$(api POST "/api/v1/profiles/$PROFILE_ID/start" '')
  status=$(status_of_response "$response")
  body=$(body_of "$response")
  if [ "$status" = '202' ]; then
    pass "$label start_status: 202"
  else
    fail "$label start_status: expected 202, got '$status'"
    say "$label start_body=$body"
    return 1
  fi
  if wait_for_state running; then
    pass "$label runtime_state: running"
    return 0
  fi
  fail "$label runtime_state: $(printf '%s' "$RUNTIME_BODY" | jq -r '.state // "unknown"')"
  say "$label runtime_body=$RUNTIME_BODY"
  return 1
}

# Kernel pid from /proc: the process whose argv carries this runtime's id.
kernel_pid() {
  for cmdline in /proc/[0-9]*/cmdline; do
    pid=${cmdline#/proc/}
    pid=${pid%/cmdline}
    if tr '\0' ' ' <"$cmdline" 2>/dev/null | grep -q -- "--mien-runtime-id=rt-$PROFILE_ID"; then
      printf '%s' "$pid"
      return 0
    fi
  done
  return 1
}

# True while `$1` is still this runtime's kernel process.
kernel_alive() {
  [ -n "$1" ] || return 1
  tr '\0' ' ' <"/proc/$1/cmdline" 2>/dev/null | grep -q -- '--mien-runtime-id'
}

say '== runtime start #1 =='
if start_once 'start1'; then
  isolation=$(printf '%s' "$RUNTIME_BODY" | jq -r '.isolation')
  say "isolation_actual=$isolation"
  pass "start1 isolation_reported: $isolation"
  expect_equal 'start1 direct' "$(printf '%s' "$RUNTIME_BODY" | jq -r '.direct')" 'true'
  expect_equal 'start1 reduced_assurance' \
    "$(printf '%s' "$RUNTIME_BODY" | jq -r '.reduced_assurance')" 'true'

  if RUNNING_PID=$(kernel_pid); then
    exe=$(readlink "/proc/$RUNNING_PID/exe" 2>/dev/null || echo unknown)
    comm=$(tr -d '\0' <"/proc/$RUNNING_PID/comm" 2>/dev/null || echo unknown)
    pass "start1 kernel_process pid=$RUNNING_PID comm=$comm exe=$exe"
    # Sandbox evidence straight from the kernel tree: a descendant with a
    # seccomp filter of 2 that lives in a user namespace of its own.
    init_user=$(readlink /proc/1/ns/user 2>/dev/null || true)
    sandboxed=0
    for status_file in /proc/[0-9]*/status; do
      pid=${status_file#/proc/}
      pid=${pid%/status}
      [ "$pid" = "$RUNNING_PID" ] && continue
      [ "$(awk '/^Seccomp:/ { print $2 }' "$status_file" 2>/dev/null)" = '2' ] || continue
      ns_user=$(readlink "/proc/$pid/ns/user" 2>/dev/null || true)
      if [ -n "$ns_user" ] && [ "$ns_user" != "$init_user" ]; then
        sandboxed=$((sandboxed + 1))
      fi
    done
    say "seccomp_user_namespace_descendants=$sandboxed"
    if [ "$isolation" = 'enforced' ] && [ "$sandboxed" -gt 0 ]; then
      pass 'start1 isolation_matches_proc_evidence'
    elif [ "$isolation" = 'enforced' ]; then
      fail 'start1 isolation_matches_proc_evidence (API says enforced, /proc shows no sandboxed child)'
    else
      # A non-enforced verdict is recorded, not promoted to a pass.
      say "start1 isolation_proc_evidence=sandboxed_descendants=$sandboxed (reported isolation=$isolation)"
    fi
  else
    fail 'start1 kernel_process_found_in_proc'
  fi
fi

# ---------------------------------------------------------------------------
# 6. Stop, relaunch, stop again
# ---------------------------------------------------------------------------
say '== runtime stop #1 =='
stop=$(api POST "/api/v1/profiles/$PROFILE_ID/stop" '')
expect_equal 'stop1_status' "$(status_of_response "$stop")" '202'
if wait_for_state stopped; then
  pass 'stop1 runtime_state: stopped'
else
  fail "stop1 runtime_state: $(printf '%s' "$RUNTIME_BODY" | jq -r '.state // "unknown"')"
fi
if kernel_alive "$RUNNING_PID"; then
  fail "stop1 kernel_process_gone pid=$RUNNING_PID"
else
  pass "stop1 kernel_process_gone pid=${RUNNING_PID:-none}"
fi

say '== runtime start #2 =='
if start_once 'start2'; then
  say "start2_isolation_actual=$(printf '%s' "$RUNTIME_BODY" | jq -r '.isolation')"
  if RUNNING_PID=$(kernel_pid); then
    pass "start2 kernel_process pid=$RUNNING_PID"
  else
    fail 'start2 kernel_process_found_in_proc'
  fi
fi

say '== runtime stop #2 (final) =='
stop=$(api POST "/api/v1/profiles/$PROFILE_ID/stop" '')
expect_equal 'stop2_status' "$(status_of_response "$stop")" '202'
if [ -n "$RUNNING_PID" ]; then
  # Give the supervisor a moment to signal the process group before asserting.
  waited=0
  while [ "$waited" -lt 15 ] && kernel_alive "$RUNNING_PID"; do
    sleep 1
    waited=$((waited + 1))
  done
  if kernel_alive "$RUNNING_PID"; then
    fail "stop2 kernel_process_gone pid=$RUNNING_PID"
  else
    pass "stop2 kernel_process_gone pid=$RUNNING_PID"
  fi
fi

# ---------------------------------------------------------------------------
# 7. No root process besides the container init
# ---------------------------------------------------------------------------
say '== privileges =='
root_processes=$(ps -eo uid=,pid=,comm= | awk '$1 == 0 && $2 != 1 { print $2 ":" $3 }')
if [ -z "$root_processes" ]; then
  pass 'no_root_processes_besides_init'
else
  fail "no_root_processes_besides_init: $root_processes"
fi
expect_equal 'running_uid' "$(id -u)" '10001'

# ---------------------------------------------------------------------------
# 8. Data root ownership and permissions
# ---------------------------------------------------------------------------
say '== data root =='
for path in "$DATA_ROOT" "$DATA_ROOT/secrets" "$DATA_ROOT/profiles"; do
  if [ -d "$path" ]; then
    mode=$(stat -c '%a' "$path")
    owner=$(stat -c '%u:%g' "$path")
    if [ "$mode" = '700' ] && [ "$owner" = '10001:10001' ]; then
      pass "data_permissions $path mode=$mode owner=$owner"
    else
      fail "data_permissions $path mode=$mode owner=$owner"
    fi
  else
    fail "data_permissions $path is missing"
  fi
done
master="$DATA_ROOT/secrets/master.key"
if [ -f "$master" ]; then
  expect_equal 'master_key_mode' "$(stat -c '%a' "$master")" '600'
  expect_equal 'master_key_owner' "$(stat -c '%u:%g' "$master")" '10001:10001'
else
  fail 'master_key_present'
fi

# ---------------------------------------------------------------------------
# 9. Schema version reached the latest migration
# ---------------------------------------------------------------------------
say '== migrations =='
version=$(sqlite3 "$DATA_ROOT/mien.db" \
  'SELECT COALESCE(MAX(version), 0) FROM schema_migrations;' 2>/dev/null || echo 'error')
expect_equal 'schema_version' "$version" '8'

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
say '== summary =='
say "profile_id=$PROFILE_ID isolation_last=$(printf '%s' "$RUNTIME_BODY" | jq -r '.isolation // "n/a"' 2>/dev/null)"
if [ "$FAILURES" -eq 0 ]; then
  say 'RESULT PASS (0 failures)'
  exit 0
fi
say "RESULT FAIL ($FAILURES failure(s))"
exit 1
