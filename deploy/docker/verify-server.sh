#!/bin/sh
set -u

# In-container acceptance for the production image (M3 exit criteria).
#
# Runs as the container user (10001:10001), never as root, and exercises the real
# HTTP API end to end: health, login, forced password change, profile creation,
# a headful launch under Xvfb, stop, relaunch, final stop, a headless launch
# proven from the kernel's own argv, and a `running` probe left behind for the
# restart phases. Every step prints one `PASS`/`FAIL` line; the exit status is 0
# only when nothing failed.
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
#   verify-server.sh                  full acceptance (default)
#   MIEN_VERIFY_PHASE=check verify-server.sh
#                                     post-restart assertions only; needs the
#                                     state file the `full` phase left behind
# Environment:
#   MIEN_PUBLIC_URL          needed for write requests: a Cookie write whose
#                            Origin differs from this value is rejected.
#   MIEN_VERIFY_BASE         default http://127.0.0.1:8787
#   MIEN_VERIFY_NEW_PASSWORD default below; on a reused volume the stored
#                            password is already this value.
#   MIEN_VERIFY_PHASE        full (default) or check
#   MIEN_VERIFY_REARM        1 makes the `check` phase start the probe again so
#                            a following restart has a live claim to reconcile

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
  # dash reports a failed `<` redirect itself, which `2>/dev/null` on `tr`
  # cannot silence; checking readability first keeps the output clean.
  [ -r "/proc/$1/cmdline" ] || return 1
  tr '\0' ' ' <"/proc/$1/cmdline" | grep -q -- '--mien-runtime-id'
}

# Every process whose argv claims a runtime id, as `pid:cmdline` lines.
# The glob is expanded once, so this function's own `tr`/`grep` children are
# never candidates; the shell itself and its parent are excluded explicitly.
mien_kernel_processes() {
  for cmdline in /proc/[0-9]*/cmdline; do
    pid=${cmdline#/proc/}
    pid=${pid%/cmdline}
    [ "$pid" = "$$" ] && continue
    [ "$pid" = "$PPID" ] && continue
    if tr '\0' ' ' <"$cmdline" 2>/dev/null | grep -q -- '--mien-runtime-id='; then
      printf '%s:%s\n' "$pid" "$(tr '\0' ' ' <"$cmdline" 2>/dev/null)"
    fi
  done
}

# Log in with an already-known password; sets COOKIE and CSRF.
# The post-restart phase has no bootstrap file any more, only the rotated
# password this run set, so it reuses the same login instead of a second path.
login_with_password() {
  attempt_password=$1
  login=$(curl -sS --max-time 30 -D /tmp/mien-verify-headers.txt -X POST "$BASE/api/v1/auth/login" \
    -H 'Content-Type: application/json' -H 'Accept: application/json' \
    -w '\n%{http_code}' \
    --data-binary "$(jq -nc --arg password "$attempt_password" '{password: $password}')")
  login_status=$(status_of_response "$login")
  login_body=$(body_of "$login")
  if [ "$login_status" != '200' ]; then
    fail "login_status: expected 200, got '$login_status'"
    say "login_body=$login_body"
    return 1
  fi
  # The session cookie is read from `Set-Cookie`, not from curl's cookie jar: in
  # server mode the cookie is `Secure`, and a jar may refuse to store a Secure
  # cookie for a plain `http://` origin. `api()` sends the value back explicitly.
  COOKIE=$(tr -d '\r' </tmp/mien-verify-headers.txt \
    | awk '/^[Ss][Ee][Tt]-[Cc][Oo][Oo][Kk][Ii][Ee]: mien_session=/ { sub(/^[^=]*=/, ""); sub(/;.*/, ""); print }' \
    | tail -n 1)
  CSRF=$(printf '%s' "$login_body" | jq -r '.csrf_token')
  if [ -z "$COOKIE" ] || [ -z "$CSRF" ] || [ "$CSRF" = 'null' ]; then
    fail 'session_cookie_and_csrf_received'
    return 1
  fi
  return 0
}

# ---------------------------------------------------------------------------
# Restart phases: `full` runs the acceptance above, `check` runs after a
# container restart and asserts what the new control plane made true.
# ---------------------------------------------------------------------------
PHASE="${MIEN_VERIFY_PHASE:-full}"
STATE_FILE="$DATA_ROOT/verify-restart.state"

# Post-restart assertions. Run as its own phase because `docker restart` and
# `docker kill` cannot be issued from inside the container this script lives in.
run_restart_check() {
  say '== restart check =='
  expect_equal 'restart healthz_status' "$(status_of "$BASE/healthz")" '200'
  expect_equal 'restart readyz_status' "$(status_of "$BASE/readyz")" '200'

  if [ ! -f "$STATE_FILE" ]; then
    say "BLOCKED restart_state_missing: $STATE_FILE"
    say 'BLOCKED detail=the prepare phase did not leave a running probe profile'
    say 'RESULT BLOCKED (no restart was prepared; no assertion was skipped)'
    return 3
  fi
  # The file only holds `KEY=value` lines written above; it carries no secret.
  # shellcheck disable=SC1090
  . "$STATE_FILE"
  PROFILE_ID=${PROFILE_ID:-}
  PRE_STATE=${PRE_STATE:-unknown}
  PRE_ISOLATION=${PRE_ISOLATION:-unknown}
  if [ -z "$PROFILE_ID" ]; then
    say "BLOCKED restart_state_invalid: $STATE_FILE"
    say 'RESULT BLOCKED (probe profile id missing)'
    return 3
  fi
  # Without this, a restart that happened on a stopped runtime would look like
  # a pass without ever exercising reconciliation.
  expect_equal 'restart probe_was_running' "$PRE_STATE" 'running'

  if login_with_password "$NEW_PASSWORD"; then
    pass 'restart login_status: 200'
    pass "restart session_cookie_and_csrf_received cookie_len=${#COOKIE} csrf_len=${#CSRF}"
  else
    say 'RESULT FAIL'
    return 1
  fi

  lookup=$(api GET "/api/v1/profiles/$PROFILE_ID")
  expect_equal 'restart profile_get_status' "$(status_of_response "$lookup")" '200'
  expect_equal 'restart profile_id' "$(body_of "$lookup" | jq -r '.id')" "$PROFILE_ID"

  row_state=$(sqlite3 "$DATA_ROOT/mien.db" \
    "SELECT state FROM runtimes WHERE profile_id='$PROFILE_ID';" 2>/dev/null || true)
  api_state=$(runtime_json | jq -r '.state // "missing"' 2>/dev/null)
  say "restart pre_restart_state=$PRE_STATE runtime_row_state=${row_state:-missing} api_state=$api_state"
  case "$row_state" in
    stopped | crashed | failed) pass "restart runtime_row_terminal: $row_state" ;;
    *) fail "restart runtime_row_terminal: expected stopped/crashed/failed, got '${row_state:-missing}'" ;;
  esac
  # A graceful stop must reach `stopped`. `crashed` here means the shutdown path
  # did not stop the kernel and only boot reconciliation settled the row, which is
  # exactly the defect T33 fixes.
  if [ "${MIEN_VERIFY_EXPECT_STOPPED:-0}" = '1' ]; then
    expect_equal 'restart runtime_row_stopped' "${row_state:-missing}" 'stopped'
  fi
  case "$api_state" in
    stopped | crashed | failed) pass "restart api_state_terminal: $api_state" ;;
    *) fail "restart api_state_terminal: expected stopped/crashed/failed, got '$api_state'" ;;
  esac

  # `last_known_isolation` is the last successful `running` verdict; a restart
  # must not rewrite it to `unknown` while settling the row.
  isolation_after=$(sqlite3 "$DATA_ROOT/mien.db" \
    "SELECT last_known_isolation FROM runtimes WHERE profile_id='$PROFILE_ID';" 2>/dev/null || true)
  expect_equal 'restart last_known_isolation_retained' "${isolation_after:-missing}" "$PRE_ISOLATION"

  orphans=$(mien_kernel_processes)
  if [ -z "$orphans" ]; then
    pass 'restart orphan_kernel_processes: 0'
  else
    fail 'restart orphan_kernel_processes: 0'
    say "restart orphan_kernel_processes_actual=$orphans"
  fi

  # Arm the next restart phase: the probe must be left running again, so the
  # following `docker kill` tests reconciliation and not an already-terminal row.
  if [ "${MIEN_VERIFY_REARM:-0}" = '1' ]; then
    if start_once 'restart-rearm'; then
      row_state=$(sqlite3 "$DATA_ROOT/mien.db" \
        "SELECT state FROM runtimes WHERE profile_id='$PROFILE_ID';" 2>/dev/null || true)
      isolation_now=$(sqlite3 "$DATA_ROOT/mien.db" \
        "SELECT last_known_isolation FROM runtimes WHERE profile_id='$PROFILE_ID';" 2>/dev/null || true)
      expect_equal 'restart rearm runtime_row' "${row_state:-missing}" 'running'
      printf 'PROFILE_ID=%s\nPRE_STATE=%s\nPRE_ISOLATION=%s\n' \
        "$PROFILE_ID" "${row_state:-missing}" "${isolation_now:-unknown}" >"$STATE_FILE"
      pass "restart rearm state_recorded profile_id=$PROFILE_ID"
    fi
  fi

  if [ "$FAILURES" -eq 0 ]; then
    say 'RESULT PASS (0 failures)'
    return 0
  fi
  say "RESULT FAIL ($FAILURES failure(s))"
  return 1
}

if [ "$PHASE" = 'check' ]; then
  run_restart_check
  exit $?
fi
if [ "$PHASE" != 'full' ]; then
  say "BLOCKED phase_unknown: $PHASE"
  say 'BLOCKED detail=MIEN_VERIFY_PHASE must be full or check'
  exit 3
fi

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
if login_with_password "$CURRENT_PASSWORD"; then
  pass 'login_status: 200'
  pass "session_cookie_and_csrf_received cookie_len=${#COOKIE} csrf_len=${#CSRF}"
else
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
# 10. Headless mode in the container
# ---------------------------------------------------------------------------
# A separate profile with `mode: headless`; the headful acceptance above proves
# the display stack, this proves the mode does not depend on it and that its
# guarantee is lowered in the response. The kernel argv is read from /proc, so
# `--headless=new` is proven on the real process, not just in a plan.
say '== headless mode =='
HEADLESS_NAME="$PROFILE_NAME-headless"
headless_body=$(jq -nc --arg name "$HEADLESS_NAME" --argjson fingerprint "$fingerprint" \
  '{name: $name, mode: "headless", fingerprint: $fingerprint}')
headless_create=$(api POST /api/v1/profiles "$headless_body")
headless_status=$(status_of_response "$headless_create")
headless_json=$(body_of "$headless_create")
HEADLESS_ID=
if [ "$headless_status" = '201' ]; then
  HEADLESS_ID=$(printf '%s' "$headless_json" | jq -r '.id')
  pass "headless profile_create_status: 201 id=$HEADLESS_ID"
  expect_equal 'headless profile_mode' "$(printf '%s' "$headless_json" | jq -r '.mode')" 'headless'
else
  fail "headless profile_create_status: expected 201, got '$headless_status'"
  say "headless profile_create_body=$headless_json"
fi

if [ -n "$HEADLESS_ID" ]; then
  # `wait_for_state`/`start_once`/`kernel_pid` address the headful profile by id;
  # the headless phase needs the same helpers pointed at this one.
  HEADFUL_ID=$PROFILE_ID
  PROFILE_ID=$HEADLESS_ID
  if start_once 'headless'; then
    isolation=$(printf '%s' "$RUNTIME_BODY" | jq -r '.isolation')
    say "headless_isolation_actual=$isolation"
    expect_equal 'headless reduced_assurance' \
      "$(printf '%s' "$RUNTIME_BODY" | jq -r '.reduced_assurance')" 'true'
    expect_equal 'headless direct' "$(printf '%s' "$RUNTIME_BODY" | jq -r '.direct')" 'true'
    expect_equal 'headless mode_reported' "$(printf '%s' "$RUNTIME_BODY" | jq -r '.mode')" 'headless'
    limitations=$(printf '%s' "$RUNTIME_BODY" | jq -r '.limitations | join("; ")')
    say "headless limitations_actual=$limitations"

    if HEADLESS_PID=$(kernel_pid); then
      headless_cmdline=$(tr '\0' ' ' <"/proc/$HEADLESS_PID/cmdline" 2>/dev/null)
      say "headless kernel_pid=$HEADLESS_PID cmdline=$headless_cmdline"
      # Split the argv on NUL into one token per line so the checks are exact
      # tokens, never substring matches inside a path or another flag's value.
      headless_argv=$(tr '\0' '\n' <"/proc/$HEADLESS_PID/cmdline" 2>/dev/null)
      if printf '%s\n' "$headless_argv" | grep -qx -- '--headless=new'; then
        pass 'headless argv_contains --headless=new'
      elif printf '%s\n' "$headless_argv" | grep -qx -- '--headless'; then
        pass 'headless argv_contains --headless'
      else
        fail 'headless argv_contains --headless (the process is not running headless)'
      fi
      if printf '%s\n' "$headless_argv" | grep -qE -- '^--(start-maximized|start-fullscreen|kiosk)$'; then
        fail "headless argv_contains_headful_window_flags: $(printf '%s\n' "$headless_argv" | grep -E -- '^--(start-maximized|start-fullscreen|kiosk)$' | tr '\n' ' ')"
      else
        pass 'headless argv_has_no_headful_window_flags'
      fi
      # `--window-size=W,H` stays in argv for both modes; it is viewport geometry,
      # not evidence of a window, so record it and only fail on other window flags.
      window_size=$(printf '%s\n' "$headless_argv" | grep -E -- '^--window-size=' || true)
      say "headless window_size_arg=${window_size:-none}"
      other_window=$(printf '%s\n' "$headless_argv" \
        | grep -E -- '^--(app|new-window|window-position|window-name|auto-open-devtools-for-tabs)=' || true)
      if [ -z "$other_window" ]; then
        pass 'headless argv_has_no_other_window_arguments'
      else
        fail "headless argv_has_no_other_window_arguments: $other_window"
      fi

      stop=$(api POST "/api/v1/profiles/$HEADLESS_ID/stop" '')
      expect_equal 'headless stop_status' "$(status_of_response "$stop")" '202'
      if wait_for_state stopped; then
        pass 'headless runtime_state: stopped'
      else
        fail "headless runtime_state: $(printf '%s' "$RUNTIME_BODY" | jq -r '.state // "unknown"')"
      fi
      waited=0
      while [ "$waited" -lt 15 ] && kernel_alive "$HEADLESS_PID"; do
        sleep 1
        waited=$((waited + 1))
      done
      if kernel_alive "$HEADLESS_PID"; then
        fail "headless kernel_process_gone pid=$HEADLESS_PID"
      else
        pass "headless kernel_process_gone pid=$HEADLESS_PID"
      fi
    else
      fail 'headless kernel_process_found_in_proc'
    fi
  fi
  PROFILE_ID=$HEADFUL_ID
fi

# ---------------------------------------------------------------------------
# 11. Leave a running probe for the restart phases
# ---------------------------------------------------------------------------
# `docker restart`/`docker kill` cannot be issued from inside this container, so
# the script leaves a `running` row plus the values the post-restart phase needs.
# The state file holds only the profile id and the observed state/isolation.
say '== restart prepare =='
if start_once 'restart-prepare'; then
  row_state=$(sqlite3 "$DATA_ROOT/mien.db" \
    "SELECT state FROM runtimes WHERE profile_id='$PROFILE_ID';" 2>/dev/null || true)
  isolation_now=$(sqlite3 "$DATA_ROOT/mien.db" \
    "SELECT last_known_isolation FROM runtimes WHERE profile_id='$PROFILE_ID';" 2>/dev/null || true)
  expect_equal 'restart_prepare runtime_row' "${row_state:-missing}" 'running'
  printf 'PROFILE_ID=%s\nPRE_STATE=%s\nPRE_ISOLATION=%s\n' \
    "$PROFILE_ID" "${row_state:-missing}" "${isolation_now:-unknown}" >"$STATE_FILE"
  chmod 0600 "$STATE_FILE"
  say "restart_prepare state_file=$STATE_FILE profile_id=$PROFILE_ID isolation=$isolation_now"
fi

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
