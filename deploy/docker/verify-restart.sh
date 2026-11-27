#!/bin/sh
set -u

# Host-side restart acceptance for the production image (M3 exit criteria).
#
# `verify-server.sh` runs inside the container and cannot restart the container
# it lives in, so this driver issues the two restarts and asks the in-container
# script (phase `check`) to assert what the new control plane made true:
#
#   1. graceful `docker restart`  -> ready, profile kept, runtime row terminal,
#                                    no kernel process left behind;
#   2. `docker kill` (SIGKILL, no entrypoint cleanup) + `docker start`
#                                 -> the leftover row is reconciled, orphan
#                                    kernels are 0, health and readiness are ok.
#
# Every assertion prints one `PASS`/`FAIL` line and the exit status is 0 only
# when nothing failed. A value that cannot be established prints `BLOCKED` and
# exits 3; no result is ever faked.
#
# The container is started with the run command documented in
# `README.server.md`: read-only root, 1 GiB `/tmp` tmpfs and `/dev/shm`,
# `--cap-drop ALL`, `no-new-privileges`, the spike seccomp profile,
# `apparmor=unconfined`, uid `10001:10001`, and a loopback port binding.
# `MIEN_ALLOW_DIRECT_EGRESS=1` is required because both probe profiles are direct.
#
# Usage (on the Docker host, from the repository root):
#   deploy/docker/verify-restart.sh <image>
# Environment:
#   MIEN_VERIFY_CONTAINER  default mien-restart-acceptance
#   MIEN_VERIFY_VOLUME     default mien-restart-acceptance-data
#   MIEN_VERIFY_PORT       default 8787 (bound on 127.0.0.1 only)
#   MIEN_VERIFY_TIMEOUT    default 180 seconds of readiness waiting

IMAGE="${1:-}"
CONTAINER="${MIEN_VERIFY_CONTAINER:-mien-restart-acceptance}"
VOLUME="${MIEN_VERIFY_VOLUME:-mien-restart-acceptance-data}"
PORT="${MIEN_VERIFY_PORT:-8787}"
TIMEOUT="${MIEN_VERIFY_TIMEOUT:-180}"
HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
BASE="http://127.0.0.1:$PORT"

# The post-restart phases log in with the rotated password; forwarding a caller
# override keeps that working instead of silently using the script default.
VERIFY_ENV=
if [ -n "${MIEN_VERIFY_NEW_PASSWORD:-}" ]; then
  VERIFY_ENV="MIEN_VERIFY_NEW_PASSWORD=$MIEN_VERIFY_NEW_PASSWORD"
fi

FAILURES=0
say() { printf '%s\n' "$*"; }
pass() { say "PASS $1"; }
fail() {
  say "FAIL $1"
  FAILURES=$((FAILURES + 1))
}
blocked() {
  say "BLOCKED $1"
  shift
  for line in "$@"; do say "BLOCKED $line"; done
  say 'RESULT BLOCKED (no assertion was skipped)'
  exit 3
}

[ -n "$IMAGE" ] || blocked 'image_missing' 'detail=usage: deploy/docker/verify-restart.sh <image>'
command -v docker >/dev/null 2>&1 || blocked 'docker_missing'
docker info >/dev/null 2>&1 || blocked 'docker_unavailable' 'detail=the current user cannot reach the Docker daemon'
[ -f "$HERE/seccomp/chromium.json" ] || blocked "seccomp_profile_missing: $HERE/seccomp/chromium.json"
docker image inspect "$IMAGE" >/dev/null 2>&1 || blocked "image_not_found: $IMAGE"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  docker volume rm "$VOLUME" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

cleanup

status_of() {
  curl -sS -o /dev/null -w '%{http_code}' --max-time 5 "$1" 2>/dev/null || printf '000'
}

# Wait until `/healthz` answers 200 and `/readyz` answers 200.
wait_ready() {
  label=$1
  waited=0
  while [ "$waited" -lt "$TIMEOUT" ]; do
    if [ "$(status_of "$BASE/healthz")" = '200' ] && [ "$(status_of "$BASE/readyz")" = '200' ]; then
      pass "$label ready: healthz 200, readyz 200"
      return 0
    fi
    # A container that exited will never become ready; report why instead of
    # polling until the deadline.
    running=$(docker inspect -f '{{.State.Running}}' "$CONTAINER" 2>/dev/null || printf 'false')
    if [ "$running" != 'true' ]; then
      fail "$label container_exited_before_ready state=$(docker inspect -f '{{.State.Status}}' "$CONTAINER" 2>/dev/null || echo missing)"
      docker logs --tail 40 "$CONTAINER" 2>&1 | sed 's/^/  log: /'
      return 1
    fi
    sleep 2
    waited=$((waited + 2))
  done
  fail "$label ready: timed out after ${TIMEOUT}s healthz=$(status_of "$BASE/healthz") readyz=$(status_of "$BASE/readyz")"
  return 1
}

# Run one phase of the in-container verifier and stream its PASS/FAIL lines.
# Extra arguments are `docker exec` options, so the phase can receive its env.
verify_phase() {
  phase=$1
  shift
  if [ -n "$VERIFY_ENV" ]; then
    docker exec -e "MIEN_VERIFY_PHASE=$phase" -e "$VERIFY_ENV" \
      "$@" "$CONTAINER" /usr/local/bin/verify-server.sh
  else
    docker exec -e "MIEN_VERIFY_PHASE=$phase" \
      "$@" "$CONTAINER" /usr/local/bin/verify-server.sh
  fi
}

# Kernel processes inside the container, read from /proc instead of `ps`. The
# `sh` that performs the scan is excluded, and nothing else can carry this
# application's runtime-id argument.
orphan_count() {
  docker exec "$CONTAINER" sh -c '
    count=0
    for cmdline in /proc/[0-9]*/cmdline; do
      pid=${cmdline#/proc/}
      pid=${pid%/cmdline}
      [ "$pid" = "$$" ] && continue
      [ "$pid" = "$PPID" ] && continue
      if tr "\0" " " <"$cmdline" 2>/dev/null | grep -q -- "--mien-runtime-id="; then
        count=$((count + 1))
        printf "orphan pid=%s cmdline=%s\n" "$pid" "$(tr "\0" " " <"$cmdline" 2>/dev/null)"
      fi
    done
    echo "orphan_count=$count"
  ' 2>/dev/null
}

say "== start container =="
say "image=$IMAGE"
# `docker run` is spelled out once, and the same text is printed for the report.
RUN_CMD="docker run -d --name $CONTAINER --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --shm-size=1g --cap-drop ALL --security-opt no-new-privileges --security-opt seccomp=$HERE/seccomp/chromium.json --security-opt apparmor=unconfined --user 10001:10001 --pids-limit 512 --memory 4g --cpus 2 --ulimit nofile=4096:4096 -p 127.0.0.1:$PORT:8787 -v $VOLUME:/data -e MIEN_BIND=0.0.0.0:8787 -e MIEN_PUBLIC_URL=http://127.0.0.1:8787 -e MIEN_ALLOW_DIRECT_EGRESS=1 $IMAGE"
say "run_command=$RUN_CMD"
if ! docker run -d --name "$CONTAINER" \
  --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --shm-size=1g \
  --cap-drop ALL --security-opt no-new-privileges \
  --security-opt "seccomp=$HERE/seccomp/chromium.json" \
  --security-opt apparmor=unconfined \
  --user 10001:10001 --pids-limit 512 --memory 4g --cpus 2 \
  --ulimit nofile=4096:4096 \
  -p "127.0.0.1:$PORT:8787" \
  -v "$VOLUME:/data" \
  -e MIEN_BIND=0.0.0.0:8787 \
  -e MIEN_PUBLIC_URL=http://127.0.0.1:8787 \
  -e MIEN_ALLOW_DIRECT_EGRESS=1 \
  "$IMAGE" >/tmp/mien-verify-run.log 2>&1; then
  sed 's/^/  run: /' /tmp/mien-verify-run.log
  blocked "container_start_failed: $CONTAINER" 'detail=see the docker run error above'
fi
pass "container_started name=$CONTAINER"

wait_ready 'initial' || true
say '== in-container acceptance (phase full) =='
verify_phase full
full_status=$?
if [ "$full_status" -eq 0 ]; then
  pass 'verify_phase_full: exit 0'
elif [ "$full_status" -eq 3 ]; then
  blocked 'in_container_verifier_blocked' 'detail=see the BLOCKED lines above'
else
  fail "verify_phase_full: exit $full_status"
fi

say '== graceful restart (docker restart) =='
if docker restart "$CONTAINER" >/dev/null 2>&1; then
  pass 'graceful_restart_issued'
else
  fail 'graceful_restart_issued'
fi
wait_ready 'graceful_restart' || true
# The phase leaves a fresh `running` row behind, so the hard kill below
# reconciles a live claim instead of an already-terminal one.
verify_phase check -e MIEN_VERIFY_REARM=1 -e MIEN_VERIFY_EXPECT_STOPPED=1
graceful_status=$?
if [ "$graceful_status" -eq 0 ]; then
  pass 'graceful_restart_verify: exit 0'
else
  fail "graceful_restart_verify: exit $graceful_status"
fi

say '== hard kill (docker kill, SIGKILL) then start =='
# SIGKILL never runs the entrypoint's EXIT trap, so the old container performs
# no cleanup. The next container process is what has to reconcile the leftover
# claim; the old kernel cannot outlive its container namespace.
if docker kill "$CONTAINER" >/dev/null 2>&1; then
  pass 'hard_kill_issued signal=SIGKILL'
else
  fail 'hard_kill_issued signal=SIGKILL'
fi
# Only the lines the *new* container writes matter; `--since` is relative to
# the start below, so the previous process's log is out of scope by definition.
STARTED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)
if docker start "$CONTAINER" >/dev/null 2>&1; then
  pass 'hard_kill_restart_issued'
else
  fail 'hard_kill_restart_issued'
fi
wait_ready 'hard_restart' || true
orphans=$(orphan_count)
printf '%s\n' "$orphans"
case "$orphans" in
  *'orphan_count=0'*)
    pass 'hard_restart_orphan_kernel_processes: 0'
    ;;
  *)
    fail 'hard_restart_orphan_kernel_processes: 0 (see orphan lines above)'
    ;;
esac
# Reconciliation evidence from the new control plane's own log, not inferred
# from the row: the row state below is the effect, this is the recorded action.
reconcile_log=$(docker logs --since "$STARTED_AT" "$CONTAINER" 2>&1 \
  | grep -E 'runtime reconciliation (finished|failed)' | head -n 2)
if printf '%s' "$reconcile_log" | grep -q 'runtime reconciliation finished'; then
  pass 'hard_restart_reconcile_log: runtime reconciliation finished'
  say "hard_restart_reconcile_log_line=$reconcile_log"
else
  fail 'hard_restart_reconcile_log: runtime reconciliation finished'
  say "hard_restart_reconcile_log_actual=${reconcile_log:-none}"
fi
verify_phase check
hard_status=$?
if [ "$hard_status" -eq 0 ]; then
  pass 'hard_restart_verify: exit 0'
else
  fail "hard_restart_verify: exit $hard_status"
fi

say '== summary =='
say "image=$IMAGE"
if [ "$FAILURES" -eq 0 ]; then
  say 'RESULT PASS (0 failures)'
  exit 0
fi
say "RESULT FAIL ($FAILURES failure(s))"
exit 1
