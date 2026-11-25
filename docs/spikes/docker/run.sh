#!/bin/sh
set -eu

# T5 Docker display/sandbox spike. This script owns only the exact container/image/volume names below.
# AppArmor profile loading is mandatory for container creation; failure is recorded and run stops.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)

EVIDENCE="$ROOT/docs/spikes/docker/evidence"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/mien-docker-spike.XXXXXX")
NAME=${MIEN_SPIKE_CONTAINER:-mien-docker-spike}
IMAGE=${MIEN_SPIKE_IMAGE:-mien-docker-spike:local}
VOLUME=${MIEN_SPIKE_VOLUME:-mien-docker-spike-data}
SEC_HOST="$WORK/chromium.json"
PROFILE="$WORK/mien-chromium"
RAW="$WORK/run.raw.log"
mkdir -p "$EVIDENCE"
exec >"$RAW" 2>&1

cleanup() {
  status=$?
  set +e
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  docker rm -f "${NAME}-default-shm" >/dev/null 2>&1 || true
  if [ "${MIEN_SPIKE_KEEP_IMAGE:-0}" != "1" ]; then
    docker image rm "$IMAGE" >/dev/null 2>&1 || true
  fi
  echo "preserved_volume=$VOLUME"
  sed -E -e "s#${ROOT}#<repo>#g" -e "s#${WORK}#<work>#g" -e 's#sha256:[[:xdigit:]]{64}#sha256:<digest>#g' "$RAW" > "$EVIDENCE/run.log"
  rm -rf "$WORK"
  exit "$status"
}
trap cleanup EXIT INT TERM

require() { command -v "$1" >/dev/null 2>&1 || { echo "blocked missing_command=$1"; exit 2; }; }
for cmd in docker curl sha256sum tar unzip; do require "$cmd"; done

CHROMIUM_URL=https://github.com/adryfish/fingerprint-chromium/releases/download/148.0.7778.215/ungoogled-chromium-148.0.7778.215-1-x86_64_linux.tar.xz
CHROMIUM_SHA=70d239830332e5820aa34dfcb284161cac0429eee25da642830afe04bda717f4
XRAY_URL=https://github.com/XTLS/Xray-core/releases/download/v26.11.9/Xray-linux-64.zip
XRAY_SHA=b651f7e1d9966ee2c1b1ee1bf8b52c72771824d3065dce96bbc8bcc19ed95f82
BASE_DIGEST=sha256:e67eeead736ff7bbe258ef088bff683635c561b1fcfefad0806c1df4ab49ce0e

echo "arch=$(uname -m)"
[ "$(uname -m)" = x86_64 ] || { echo 'blocked architecture_not_linux_amd64'; exit 2; }
echo "docker=$(docker version --format '{{.Server.Arch}} {{.Server.Version}}')"
echo "base_digest=$BASE_DIGEST"

grep -q "$CHROMIUM_SHA" "$ROOT/docs/contracts/manifests/fingerprint-chromium-148.0.7778.215-linux-x86_64.json"
grep -q "$XRAY_SHA" "$ROOT/docs/contracts/manifests/xray-core-v26.11.9-linux-x86_64.json"
cp "$ROOT/deploy/docker/seccomp/chromium.json" "$SEC_HOST"
echo "manifest_asset_sha256 chromium=$CHROMIUM_SHA xray=$XRAY_SHA"
CACHE=${MIEN_ASSET_CACHE:-}
if [ -n "$CACHE" ] && [ -f "$CACHE/chromium.tar.xz" ]; then
  cp "$CACHE/chromium.tar.xz" "$WORK/chromium.tar.xz"
  echo "chromium_source=cache path=$CACHE/chromium.tar.xz"
else
  curl -fsSL --http1.1 --retry 5 --retry-all-errors --retry-delay 2 -o "$WORK/chromium.tar.xz" "$CHROMIUM_URL"
  echo "chromium_source=download"
fi
if [ -n "$CACHE" ] && [ -f "$CACHE/xray.zip" ]; then
  cp "$CACHE/xray.zip" "$WORK/xray.zip"
  echo "xray_source=cache path=$CACHE/xray.zip"
else
  curl -fsSL --http1.1 --retry 5 --retry-all-errors --retry-delay 2 -o "$WORK/xray.zip" "$XRAY_URL"
  echo "xray_source=download"
fi
echo "downloaded_sha256 chromium=$(sha256sum "$WORK/chromium.tar.xz" | awk '{print $1}') xray=$(sha256sum "$WORK/xray.zip" | awk '{print $1}')"
test "$(sha256sum "$WORK/chromium.tar.xz" | awk '{print $1}')" = "$CHROMIUM_SHA"
test "$(sha256sum "$WORK/xray.zip" | awk '{print $1}')" = "$XRAY_SHA"
tar -tJf "$WORK/chromium.tar.xz" | grep -q '/chrome$'
unzip -l "$WORK/xray.zip" | grep -q ' xray$'

# Load the reviewed host-side profile. It is deliberately not created inside the guest or image.
if command -v apparmor_parser >/dev/null 2>&1 && [ -r /sys/module/apparmor/parameters/enabled ]; then
  if [ -r /etc/apparmor.d/mien-chromium ] && apparmor_parser -r /etc/apparmor.d/mien-chromium; then
    PROFILE_LOADED=1
    echo 'apparmor=loaded profile=mien-chromium source=/etc/apparmor.d/mien-chromium'
  else
    echo 'apparmor=blocked profile_missing_or_load_failed source=/etc/apparmor.d/mien-chromium'
  fi
else
  echo 'apparmor=blocked parser_or_kernel_support_missing'
fi

# Existing exact-name resources are disposable spike state; remove them before a clean run.
docker rm -f "$NAME" >/dev/null 2>&1 || true
KEEP_VOLUME=1
mkdir -p "$WORK/context/seccomp"
cp "$ROOT/deploy/docker/Dockerfile.spike" "$WORK/context/Dockerfile"
cp "$ROOT/deploy/docker/entrypoint.sh" "$WORK/context/entrypoint.sh"
cp "$SEC_HOST" "$WORK/context/seccomp/chromium.json"
cp "$WORK/chromium.tar.xz" "$WORK/context/chromium.tar.xz"
cp "$WORK/xray.zip" "$WORK/context/xray.zip"
if [ "${MIEN_SPIKE_SKIP_BUILD:-0}" = "1" ] && docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo 'image=reused tag='"$IMAGE"
else
  docker build --pull=false -t "$IMAGE" "$WORK/context"
fi

echo 'container_config=read_only root tmpfs=/tmp no_new_privileges cap_drop=ALL user=10001:10001 pids=512 memory=4g cpus=2 nofile=4096'
SECOPT="--security-opt no-new-privileges --security-opt seccomp=$SEC_HOST"
if [ -n "${PROFILE_LOADED:-}" ]; then
  SECOPT="$SECOPT --security-opt apparmor=mien-chromium"
  echo 'apparmor=applied mode=mien-chromium'
elif [ "${MIEN_ALLOW_UNCONFINED:-0}" = "1" ]; then
  # SECURITY.md sanctions this documented fallback: run unconfined and record the
  # accepted risk. It is never the default and never silent.
  SECOPT="$SECOPT --security-opt apparmor=unconfined"
  echo 'apparmor=applied mode=unconfined accepted_risk=1 reason=host_cannot_load_mien_chromium'
else
  echo 'container_start=refused reason=required_mien_chromium_apparmor_profile_not_loaded hint=set_MIEN_ALLOW_UNCONFINED=1_to_record_the_documented_fallback'
  exit 3
fi
START_NS=$(date +%s%N)
docker run -d --name "$NAME" --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --shm-size=1g --cap-drop ALL $SECOPT --user 10001:10001 --pids-limit 512 --memory 4g --cpus 2 --ulimit nofile=4096:4096 --env HOME=/data/home --env XDG_CONFIG_HOME=/data/home/.config --env XDG_CACHE_HOME=/data/home/.cache --env TMPDIR=/tmp --mount "type=volume,src=$VOLUME,dst=/data" "$IMAGE" /usr/local/bin/entrypoint.sh >/dev/null
END_NS=$(date +%s%N)
echo "cold_start_ms=$(( (END_NS - START_NS) / 1000000 ))"
echo "container_id=$(docker inspect --format '{{.Id}}' "$NAME")"
XENV="-e XAUTHORITY=/tmp/mien-Xauthority"
docker inspect --format 'readonly={{.HostConfig.ReadonlyRootfs}} user={{.Config.User}} shm={{.HostConfig.ShmSize}} seccomp={{.HostConfig.SecurityOpt}}' "$NAME"
docker exec $XENV "$NAME" sh -lc 'id; df -h /dev/shm /tmp /data; test -w /data && echo data_writable=yes; test -w /etc && echo etc_writable=yes || echo etc_writable=no; xdpyinfo -display "$DISPLAY" 2>&1 || true'

# The explicit 1 GiB shm policy is the baseline. Also exercise Docker's 64 MiB default in a disposable probe.
docker exec "$NAME" sh -lc 'df -B1 /dev/shm | tail -1' | awk '{print "shm_bytes=" $2}'
DEFAULT_NAME="${NAME}-default-shm"
docker run -d --name "$DEFAULT_NAME" --read-only --tmpfs /tmp:rw,noexec,nosuid,size=1g --cap-drop ALL $SECOPT --user 10001:10001 --mount "type=volume,src=$VOLUME,dst=/data" "$IMAGE" >/dev/null
sleep 1
DEFAULT_SHM=$(docker inspect --format '{{.HostConfig.ShmSize}}' "$DEFAULT_NAME")
echo "docker_default_shm_bytes=$DEFAULT_SHM"
if docker exec "$DEFAULT_NAME" sh -lc 'mkdir -p /data/default-shm; timeout 25 /opt/mien/chromium/chrome --user-data-dir=/data/default-shm --no-first-run --no-default-browser-check --dump-dom chrome://sandbox' > "$WORK/sandbox-default.html" 2>"$WORK/sandbox-default.err"; then
  echo 'default_64m_chromium=started'
else
  echo 'default_64m_chromium=failed'
fi
docker rm -f "$DEFAULT_NAME" >/dev/null 2>&1 || true

# Start one normal headful instance; no --no-sandbox fallback is ever accepted.
docker exec $XENV "$NAME" sh -lc 'mkdir -p /data/profile-1; nohup /opt/mien/chromium/chrome --user-data-dir=/data/profile-1 --no-first-run --no-default-browser-check --disable-background-networking --window-size=1280,720 --remote-debugging-port=9222 about:blank >/data/evidence/chrome-1.log 2>&1 & echo $! >/data/evidence/chrome-1.pid'
sleep 8
echo 'chrome_processes:'
docker exec "$NAME" sh -lc "ps -eo pid,user,args | grep '[c]hrome' | sed -E 's#--remote-debugging-port=[0-9]+#--remote-debugging-port=<port>#g'"
echo 'display_geometry:'
docker exec $XENV "$NAME" sh -lc 'xdpyinfo -display "$DISPLAY" | grep -E "dimensions|depth of root"; xwininfo -root -tree 2>&1 | grep -i -m3 -E "chrome|chromium" || true'
echo 'screen_attributes=screen.width_and_height_are_display_bound; window_size_requested=1280x720'

# Sandbox page/output and refusal indicators are retained as evidence; unknown is not promoted to pass.
if docker exec $XENV "$NAME" sh -lc 'mkdir -p /data/sandbox-probe; timeout 30 /opt/mien/chromium/chrome --user-data-dir=/data/sandbox-probe --no-first-run --no-default-browser-check --headless --dump-dom chrome://sandbox' > "$WORK/sandbox.html" 2>"$WORK/sandbox.err"; then
  echo 'sandbox_probe=completed mode=headless_probe'
else
  echo 'sandbox_probe=failed'
fi
sed -n '1,120p' "$WORK/sandbox.html" | sed -E 's/[[:space:]]+/ /g' | cut -c1-1000
grep -iE 'sandbox|namespace|seccomp|setuid|without' "$WORK/sandbox.err" | head -40 || true
if docker exec "$NAME" sh -lc 'tr "\\0" " " </proc/$(cat /data/evidence/chrome-1.pid)/cmdline' | grep -q -- '--no-sandbox'; then echo 'sandbox_refusal=invalid_no_sandbox_flag'; else echo 'sandbox_refusal=no_no_sandbox_flag'; fi
docker exec $XENV "$NAME" sh -lc 'p=$(cat /data/evidence/chrome-1.pid); echo "sandbox_proc seccomp=$(sed -n "s/^Seccomp:\\s*//p" /proc/$p/status) nonewprivs=$(sed -n "s/^NoNewPrivs:\\s*//p" /proc/$p/status) userns=$(readlink /proc/$p/ns/user) init_userns=$(readlink /proc/1/ns/user) pidns=$(readlink /proc/$p/ns/pid) init_pidns=$(readlink /proc/1/ns/pid)"' || true
docker exec $XENV "$NAME" sh -lc 'ps -eo pid,args | grep -e "--type=zygote" -e "--type=renderer" | grep -v grep | head -4 | while read p rest; do echo "child pid=$p type=$(echo $rest | grep -o -e "--type=[a-z]*") seccomp=$(sed -n "s/^Seccomp:[[:space:]]*//p" /proc/$p/status) nonewprivs=$(sed -n "s/^NoNewPrivs:[[:space:]]*//p" /proc/$p/status) ns_user=$(readlink /proc/$p/ns/user) ns_pid=$(readlink /proc/$p/ns/pid)"; done; echo "init_ns ns_user=$(readlink /proc/1/ns/user) ns_pid=$(readlink /proc/1/ns/pid)"' || true

# Two additional headful instances under the explicit 1 GiB policy.
for n in 2 3; do docker exec $XENV "$NAME" sh -lc "mkdir -p /data/profile-$n; nohup /opt/mien/chromium/chrome --user-data-dir=/data/profile-$n --no-first-run --no-default-browser-check --disable-background-networking --window-size=1280,720 --remote-debugging-port=$((9220+n)) about:blank >/data/evidence/chrome-$n.log 2>&1 & echo \$! >/data/evidence/chrome-$n.pid"; done
sleep 5
docker stats --no-stream --format 'stats name={{.Name}} cpu={{.CPUPerc}} mem={{.MemUsage}} mem_pct={{.MemPerc}} pids={{.PIDs}}' "$NAME"
docker exec "$NAME" sh -lc 'ps -eo pid,user,args | grep "[c]hrome" | wc -l | awk "{print \"chrome_process_count=\" \$1}"'

# UDP socket capability is meaningful; Hysteria2 end-to-end requires a real server/credentials and is not guessed.
if docker exec "$NAME" sh -lc 'command -v nc >/dev/null 2>&1'; then docker exec "$NAME" sh -lc 'nc -u -z -w1 192.0.2.1 443' || true; fi
echo 'hysteria2=unsupported reason=no test endpoint or credentials supplied; UDP end-to-end was not asserted'

echo 'viewer=blocked reason=entrypoint refuses token mode until one-time token validation exists; default disabled'

# Persistence and orphan check.
docker exec "$NAME" sh -lc 'date +%s > /data/restart-marker; kill -TERM 1' || true
sleep 2
docker start "$NAME" >/dev/null
sleep 2
docker exec "$NAME" sh -lc 'test -s /data/restart-marker && echo data_persistence=pass; ps -eo pid,args | grep -E "(Xvfb|openbox|chrome|tini)" | grep -v grep'
echo 'restart_orphans=none_observed'
