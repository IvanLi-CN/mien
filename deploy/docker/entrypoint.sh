#!/bin/sh
set -eu

# Spike entrypoint supervises the virtual display stack; it does not run as root.
export DISPLAY="${DISPLAY:-:99}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/runtime-mien}"
export XAUTHORITY="${XAUTHORITY:-/tmp/mien-Xauthority}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 0700 "$XDG_RUNTIME_DIR"
: > "$XAUTHORITY"
chmod 0600 "$XAUTHORITY"
COOKIE=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
xauth -f "$XAUTHORITY" add "$DISPLAY" . "$COOKIE"
unset COOKIE

cleanup() {
  status=$?
  trap - EXIT INT TERM
  for pid in "${MAIN_PID:-}" "${NOVNC_PID:-}" "${X11VNC_PID:-}" "${OPENBOX_PID:-}" "${XVFB_PID:-}"; do
    if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi
  done
  wait 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT INT TERM

Xvfb "$DISPLAY" -auth "$XAUTHORITY" -screen 0 "${MIEN_SCREEN_SIZE:-1920x1080x24}" -nolisten tcp &
XVFB_PID=$!

ready=0
for _ in $(seq 1 50); do
  if xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then ready=1; break; fi
  sleep 0.1
done
[ "$ready" = 1 ] || { echo 'display_not_ready' >&2; exit 1; }
echo "display=ready name=$DISPLAY geometry=${MIEN_SCREEN_SIZE:-1920x1080x24}"

openbox --sm-disable >/tmp/openbox.log 2>&1 &
OPENBOX_PID=$!

if [ -n "${MIEN_VIEWER_TOKEN:-}" ]; then
  echo 'viewer=blocked one_time_token_validation_not_implemented' >&2
  exit 1
else
  echo 'viewer=disabled'
fi

if [ "$#" -eq 0 ]; then set -- sleep infinity; fi
"$@" &
MAIN_PID=$!
wait "$MAIN_PID"
