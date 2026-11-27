#!/bin/sh
set -eu

# Mien readiness probe, used by the image `HEALTHCHECK`.
#
# `/readyz` answers 503 by contract (`docs/contracts/openapi.yaml#/paths/~1readyz`):
# the server is alive but a dependency (database, kernel manifest) is not ready
# yet. `curl -f` would collapse that documented answer into the same failure as
# an unreachable process, so this script separates the three outcomes itself and
# maps them to the exit codes documented on `HEALTHCHECK` in `Dockerfile.server`:
#
#   0  ready                `/healthz` 200 and `/readyz` 200
#   1  not ready            `/healthz` 200 and `/readyz` 503
#   2  probe failure        `/healthz` not 200, unexpected `/readyz` status,
#                           or the request could not be completed
#
# Liveness and readiness are both probed because a container that answers
# `/readyz` 503 forever is a real, reportable state, while a container whose
# process is gone must not be described as merely "not ready".

BASE="${MIEN_HEALTHCHECK_URL:-http://127.0.0.1:${MIEN_HEALTHCHECK_PORT:-8787}}"
TIMEOUT="${MIEN_HEALTHCHECK_TIMEOUT:-4}"

# Print the HTTP status code, or `000` when no response was received.
status_of() {
  curl -sS -o /dev/null -w '%{http_code}' \
    --max-time "$TIMEOUT" --connect-timeout "$TIMEOUT" \
    "$BASE$1" 2>/dev/null || printf '000'
}

liveness=$(status_of /healthz)
if [ "$liveness" != "200" ]; then
  printf 'healthcheck=failed probe=liveness path=/healthz status=%s base=%s\n' "$liveness" "$BASE" >&2
  exit 2
fi

readiness=$(status_of /readyz)
case "$readiness" in
  200)
    printf 'healthcheck=ready path=/readyz status=200\n'
    exit 0
    ;;
  503)
    printf 'healthcheck=not_ready path=/readyz status=503 (documented readiness answer)\n' >&2
    exit 1
    ;;
  *)
    printf 'healthcheck=failed probe=readiness path=/readyz status=%s\n' "$readiness" >&2
    exit 2
    ;;
esac
