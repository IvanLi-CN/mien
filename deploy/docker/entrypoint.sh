#!/bin/sh
set -eu

# Mien container entrypoint: one shared Xvfb display, openbox, then the command.
#
# Shared by `Dockerfile.spike` (CMD `sleep infinity`) and `Dockerfile.server`
# (CMD `mien-server`). The display contract, the ordered shutdown and the
# exit-code propagation are identical for both, so a second entrypoint would only
# duplicate the display stack and drift.
#
# Contract:
# 1. The display stack starts before the main command, and startup fails closed
#    if Xvfb never answers `xdpyinfo`.
# 2. The container exits with the main command's exit status. `mien-server`
#    exits non-zero on unusable storage or an invalid configuration, and that
#    code must not be replaced with 0.
# 3. SIGTERM/SIGINT are forwarded to the main command, which shuts down
#    gracefully; then openbox, then Xvfb are stopped in that order.
# 4. The entrypoint never elevates privileges; every step runs as the container
#    user (10001:10001 in both images).
#
# Data-directory preparation is also shared: a mounted volume may arrive with
# different ownership than the image layer, and `MasterKey::load` and
# `MasterKey::from_file` reject a key that is not a 0600 regular file.

export DISPLAY="${DISPLAY:-:99}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/runtime-mien}"
export XAUTHORITY="${XAUTHORITY:-/tmp/mien-Xauthority}"
SCREEN="${MIEN_SCREEN_SIZE:-${MIEN_SCREEN:-1920x1080x24}}"
DATA_ROOT="${MIEN_DATA_ROOT:-/data}"

mkdir -p "$XDG_RUNTIME_DIR"
chmod 0700 "$XDG_RUNTIME_DIR"
: >"$XAUTHORITY"
chmod 0600 "$XAUTHORITY"
COOKIE=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
xauth -f "$XAUTHORITY" add "$DISPLAY" . "$COOKIE"
unset COOKIE

MAIN_PID=
XVFB_PID=
OPENBOX_PID=
STOP_TIMEOUT="${MIEN_STOP_TIMEOUT_SECS:-10}"

# Stop one supervised process: SIGTERM, a bounded wait, then SIGKILL.
# The deadline is polled with `kill -0` instead of backgrounding a `sleep`
# watchdog: a subshell cannot reap this shell's child, and `wait` on the
# watchdog would block for the full timeout even after a fast exit.
kill_and_wait() {
  pid=$1
  [ -n "$pid" ] || return 0
  kill -0 "$pid" 2>/dev/null || return 0
  kill -TERM "$pid" 2>/dev/null || true
  elapsed=0
  while [ "$elapsed" -lt "$STOP_TIMEOUT" ]; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
    elapsed=$((elapsed + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    echo "stop=forced pid=$pid" >&2
    kill -KILL "$pid" 2>/dev/null || true
  fi
  wait "$pid" 2>/dev/null || true
}

# Fixed order: main command, then the window manager, then the X server.
stop_all() {
  kill_and_wait "${MAIN_PID:-}"
  MAIN_PID=
  kill_and_wait "${OPENBOX_PID:-}"
  OPENBOX_PID=
  kill_and_wait "${XVFB_PID:-}"
  XVFB_PID=
}

cleanup() {
  status=$?
  trap - EXIT INT TERM
  stop_all
  exit "$status"
}
trap cleanup EXIT

# Forward the stop request to the main command only. The display outlives it
# until the command has actually exited, so in-flight browser windows keep a
# working X server during their own shutdown.
forward_stop() {
  trap - INT TERM
  echo 'shutdown=requested'
  if [ -n "${MAIN_PID:-}" ]; then
    kill -TERM "$MAIN_PID" 2>/dev/null || true
  else
    # Signal arrived during display startup; stop the stack and let the EXIT
    # trap report the signal-derived status.
    exit 143
  fi
}
trap forward_stop INT TERM

# Prepare the data root as the container user. Existing volumes keep whatever
# ownership the host gave them; when that is not ours, fail with a precise
# message instead of starting a server that cannot read its own secrets.
prepare_data_root() {
  mkdir -p "$DATA_ROOT/secrets" "$DATA_ROOT/profiles"
  chmod 0700 "$DATA_ROOT" "$DATA_ROOT/secrets" "$DATA_ROOT/profiles" 2>/dev/null || {
    echo "data_root_unusable=permissions path=$DATA_ROOT hint=chown_10001:10001_the_volume" >&2
    exit 1
  }
  if [ -f "$DATA_ROOT/secrets/master.key" ]; then
    # A key that arrived with a volume restore must be tightened, not trusted.
    chmod 0600 "$DATA_ROOT/secrets/master.key" 2>/dev/null || {
      echo "master_key_unusable=permissions path=$DATA_ROOT/secrets/master.key" >&2
      exit 1
    }
  elif [ ! -f "$DATA_ROOT/mien.db" ]; then
    # Fresh data root: provision the master key once, 0600, from the CSPRNG.
    # An existing database without a key is *not* repaired here: replacing it
    # would silently invalidate every stored proxy credential and API token, so
    # that case is left to the operator (see README.server.md).
    (
      umask 077
      od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$DATA_ROOT/secrets/master.key"
    )
    chmod 0600 "$DATA_ROOT/secrets/master.key"
    echo 'master_key=provisioned path=secrets/master.key mode=0600'
  else
    # Refuse here rather than letting the server fail later with a generic
    # storage error: a restored volume with a missing key is an operator
    # mistake, and the recovery hint must be the last thing in the log.
    echo 'master_key=missing database_present=1 action=refusing_to_replace' >&2
    echo 'master_key_recovery=restore_the_file_or_set_MIEN_MASTER_KEY' >&2
    exit 1
  fi
}

prepare_data_root

Xvfb "$DISPLAY" -auth "$XAUTHORITY" -screen 0 "$SCREEN" -nolisten tcp &
XVFB_PID=$!

ready=0
for _ in $(seq 1 50); do
  if xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.1
done
[ "$ready" = 1 ] || {
  echo 'display_not_ready' >&2
  exit 1
}
echo "display=ready name=$DISPLAY geometry=$SCREEN"

openbox --sm-disable >/tmp/openbox.log 2>&1 &
OPENBOX_PID=$!

# noVNC/x11vnc stay out of both images: the viewer needs one-time tokens
# (`REQ-API-019`), which are not implemented. A request to enable it is refused
# rather than served without replay-safe authentication.
if [ -n "${MIEN_VIEWER_TOKEN:-}" ]; then
  echo 'viewer=blocked one_time_token_validation_not_implemented' >&2
  exit 1
fi
echo 'viewer=disabled'

if [ "$#" -eq 0 ]; then
  set -- sleep infinity
fi
"$@" &
MAIN_PID=$!

# Wait for the main command, surviving forwarded signals. `wait` returns
# 128+signal when a trap interrupts it while the child is still alive, so the
# loop retries; the real exit status is returned once the child is reaped.
status=0
while :; do
  set +e
  wait "$MAIN_PID"
  code=$?
  set -e
  if [ "$code" -gt 128 ] && kill -0 "$MAIN_PID" 2>/dev/null; then
    continue
  fi
  status=$code
  break
done
MAIN_PID=
echo "main=exited status=$status"
exit "$status"
