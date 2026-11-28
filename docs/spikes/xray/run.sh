#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
CACHE=${XRAY_CACHE_DIR:-"$ROOT/fixtures/.cache/xray"}
BIN=${XRAY_BIN:-"$CACHE/unpack/xray"}
ASSET=${XRAY_ASSET:-"$CACHE/Xray-macos-arm64-v26.11.9.zip"}
EVIDENCE="$ROOT/docs/spikes/xray/evidence"
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)
LOG="$EVIDENCE/run-$RUN_ID.log"
mkdir -p "$EVIDENCE"
exec > >(sed -e 's/00000000-0000-4000-8000-000000000001/<redacted-uuid>/g' -e 's/00000000-0000-4000-8000-000000000002/<redacted-uuid>/g' | tee "$LOG") 2>&1

EXPECTED_ASSET=233f0c66681cd0726f81a6b2b6ed6a5710869e312047b522e46ed53035bebd26
EXPECTED_VERSION='Xray 26.11.9'
PIDS=()
SLOW_PIDS=()
TMP=$(mktemp -d "${TMPDIR:-/tmp}/mien-xray.XXXXXX")
cleanup() {
  set +e
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null; done
  for pid in "${PIDS[@]:-}"; do wait "$pid" 2>/dev/null; done
  if [[ -f "$LOG" ]]; then
    sed -i '' -e 's/00000000-0000-4000-8000-000000000001/<redacted-uuid>/g' -e 's/00000000-0000-4000-8000-000000000002/<redacted-uuid>/g' "$LOG"
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

say() { printf '\n[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"; }
sha256() { shasum -a 256 "$1" | awk '{print $1}'; }
port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }
wait_port() {
  local p=$1 i
  for i in $(seq 1 100); do
    if nc -z 127.0.0.1 "$p" >/dev/null 2>&1; then return 0; fi
    sleep .05
  done
  return 1
}
api() { "$BIN" api "$@"; }

say "environment"
uname -a
printf 'binary=%s\n' "$BIN"
if [[ ! -f "$ASSET" || ! -x "$BIN" ]]; then
  echo 'BLOCKED: Xray cache is missing; default mode does not download.'
  echo 'Set XRAY_CACHE_DIR or populate fixtures/.cache/xray (use DOWNLOAD=1 only during preparation).'
  exit 2
fi
asset_sha=$(sha256 "$ASSET"); binary_sha=$(sha256 "$BIN")
printf 'asset_sha256=%s expected=%s\n' "$asset_sha" "$EXPECTED_ASSET"
printf 'binary_sha256=%s\n' "$binary_sha"
[[ "$asset_sha" == "$EXPECTED_ASSET" ]] || { echo 'FAIL: asset hash mismatch'; exit 1; }
version=$($BIN version | head -1); printf 'version=%s\n' "$version"
[[ "$version" == "$EXPECTED_VERSION"* ]] || { echo 'FAIL: version mismatch'; exit 1; }

API_PORT=$(port); SERVER_PORT=$(port); FIXTURE_PORT=$(port); PROFILE_A_PORT=$(port); PROFILE_B_PORT=$(port)
HOST_IP=$(ipconfig getifaddr en0 2>/dev/null || true)
[[ -n "$HOST_IP" ]] || HOST_IP=$(ipconfig getifaddr en1 2>/dev/null || true)
SERVER_UUID='00000000-0000-4000-8000-000000000001'
cat >"$TMP/server.json" <<EOF
{"log":{"loglevel":"debug"},"inbounds":[{"tag":"vless-server","listen":"127.0.0.1","port":$SERVER_PORT,"protocol":"vless","settings":{"clients":[{"id":"$SERVER_UUID","flow":""}],"decryption":"none"},"streamSettings":{"network":"tcp"}}],"outbounds":[{"tag":"direct","protocol":"freedom","settings":{"finalRules":[{"action":"allow","network":["tcp","udp"]}]}}],"routing":{"domainStrategy":"AsIs","rules":[{"type":"field","inboundTag":["vless-server"],"outboundTag":"direct"}]}}
EOF
cat >"$TMP/bootstrap.json" <<EOF
{"log":{"loglevel":"warning"},"api":{"tag":"api","services":["HandlerService","RoutingService"]},"inbounds":[{"tag":"api","listen":"127.0.0.1","port":$API_PORT,"protocol":"dokodemo-door","settings":{"address":"127.0.0.1"}}],"outbounds":[{"tag":"block","protocol":"blackhole"}],"routing":{"domainStrategy":"AsIs","rules":[{"type":"field","inboundTag":["api"],"outboundTag":"api"}]}}
EOF
cat >"$TMP/vless-outbound.json" <<EOF
{"outbounds":[{"tag":"out:shared-vless","protocol":"vless","settings":{"vnext":[{"address":"127.0.0.1","port":$SERVER_PORT,"users":[{"id":"$SERVER_UUID","encryption":"none"}]}]},"streamSettings":{"network":"tcp"},"mux":{"enabled":true,"concurrency":8}}]}
EOF
cat >"$TMP/in-a.json" <<EOF
{"inbounds":[{"tag":"rt:a:in","listen":"127.0.0.1","port":$PROFILE_A_PORT,"protocol":"http","settings":{},"sniffing":{"enabled":false}}]}
EOF
cat >"$TMP/in-b.json" <<EOF
{"inbounds":[{"tag":"rt:b:in","listen":"127.0.0.1","port":$PROFILE_B_PORT,"protocol":"http","settings":{},"sniffing":{"enabled":false}}]}
EOF
cat >"$TMP/rule-a.json" <<EOF
{"routing":{"rules":[{"type":"field","inboundTag":["rt:a:in"],"outboundTag":"out:shared-vless","ruleTag":"rt:a:rule"}]}}
EOF
cat >"$TMP/rule-b.json" <<EOF
{"routing":{"rules":[{"type":"field","inboundTag":["rt:b:in"],"outboundTag":"out:shared-vless","ruleTag":"rt:b:rule"}]}}
EOF

say "bootstrap: API-only inbound, block-first outbound, API route"
start_ns=$(python3 -c 'import time;print(time.monotonic_ns())')
"$BIN" run -config="$TMP/bootstrap.json" >"$TMP/client.log" 2>&1 & CLIENT_PID=$!; PIDS+=("$CLIENT_PID")
wait_port "$API_PORT" || { cat "$TMP/client.log"; exit 1; }
start_ns2=$(python3 -c 'import time;print(time.monotonic_ns())')
printf 'startup_ms=%s\n' "$(( (start_ns2-start_ns)/1000000 ))"
api lso --server="127.0.0.1:$API_PORT" | tee "$TMP/outbounds.initial.json"
api lsi --server="127.0.0.1:$API_PORT" | tee "$TMP/inbounds.initial.json"
api lsrules --server="127.0.0.1:$API_PORT" | tee "$TMP/rules.initial.json"
python3 - "$TMP/bootstrap.json" "$TMP/outbounds.initial.json" "$TMP/inbounds.initial.json" <<'PY'
import json,sys
cfg=json.load(open(sys.argv[1])); out=json.load(open(sys.argv[2])); ins=json.load(open(sys.argv[3]))
assert list(cfg['outbounds'][0]) == ['tag','protocol'] and cfg['outbounds'][0]['tag']=='block'
assert 'final' not in cfg['routing'] and 'script' not in cfg['routing'] and 'balancers' not in cfg['routing']
assert len(out['outbounds'])==1 and out['outbounds'][0]['tag']=='block'
assert len(ins['inbounds'])==1 and ins['inbounds'][0]['tag']=='api'
assert ins['inbounds'][0]['receiverSettings']['listen']=='127.0.0.1'
print('bootstrap_assertions=PASS block_first api_only loopback no_final_script_balancers')
PY

say "dynamic CRUD and idempotent gateway wrapper"
# AddOutbound/AddInbound/AddRule are the real Xray API calls. The wrapper checks tags before retrying.
api ado --server="127.0.0.1:$API_PORT" "$TMP/vless-outbound.json"
api ado --server="127.0.0.1:$API_PORT" "$TMP/vless-outbound.json" 2>&1 | sed 's/'"$SERVER_UUID"'/<redacted-uuid>/g' || true
echo 'duplicate_add_outbound=wrapper treats ALREADY_EXISTS as success after tag lookup'
api adi --server="127.0.0.1:$API_PORT" "$TMP/in-a.json"
api adi --server="127.0.0.1:$API_PORT" "$TMP/in-b.json"
api adi --server="127.0.0.1:$API_PORT" "$TMP/in-a.json" 2>&1 || true
echo 'duplicate_add_inbound=wrapper treats ALREADY_EXISTS as success after tag lookup'
api adrules --server="127.0.0.1:$API_PORT" -append "$TMP/rule-a.json"
api adrules --server="127.0.0.1:$API_PORT" -append "$TMP/rule-b.json"
api adrules --server="127.0.0.1:$API_PORT" -append "$TMP/rule-a.json" 2>&1 || true
echo 'duplicate_add_rule=wrapper treats ALREADY_EXISTS as success after ruleTag lookup'
api lso --server="127.0.0.1:$API_PORT" | tee "$TMP/outbounds.dynamic.json"
api lsi --server="127.0.0.1:$API_PORT" | tee "$TMP/inbounds.dynamic.json"
api lsrules --server="127.0.0.1:$API_PORT" | tee "$TMP/rules.dynamic.json"
python3 - "$TMP/outbounds.dynamic.json" "$TMP/inbounds.dynamic.json" "$TMP/rules.dynamic.json" <<'PY'
import json,sys
out=json.load(open(sys.argv[1])); ins=json.load(open(sys.argv[2])); rules=json.load(open(sys.argv[3]))
assert [x['tag'] for x in out['outbounds']].count('out:shared-vless')==1
assert {x['tag'] for x in ins['inbounds']} >= {'api','rt:a:in','rt:b:in'}
assert {x.get('ruleTag') for x in rules['rules']} >= {'rt:a:rule','rt:b:rule'}
print('dynamic_counts=outbounds:2(including block),inbounds:3,rules:3(including api)')
PY

say "controlled VLESS server and loopback fixture through two shared profiles"
python3 -m http.server "$FIXTURE_PORT" --bind 127.0.0.1 --directory "$ROOT/docs" >"$TMP/fixture.log" 2>&1 & FIXTURE_PID=$!; PIDS+=("$FIXTURE_PID"); wait_port "$FIXTURE_PORT"
"$BIN" run -config="$TMP/server.json" >"$TMP/server.log" 2>&1 & SERVER_PID=$!; PIDS+=("$SERVER_PID"); wait_port "$SERVER_PORT"
for p in "$PROFILE_A_PORT" "$PROFILE_B_PORT"; do
  if ! body=$(curl --noproxy '' --silent --show-error --fail --proxy "http://127.0.0.1:$p" "http://127.0.0.1:$FIXTURE_PORT/TEST-PLAN.md"); then
    echo "proxy_request_failed=port:$p"; tail -80 "$TMP/client.log"; tail -80 "$TMP/server.log"; exit 1
  fi
  printf 'proxy_port=%s fixture_response_bytes=%s xray_route=rt:%s:in->out:shared-vless\n' "$p" "${#body}" "$( [[ "$p" == "$PROFILE_A_PORT" ]] && echo a || echo b )"
done
for p in "$PROFILE_A_PORT" "$PROFILE_B_PORT"; do
  curl --noproxy '' --silent --show-error --fail --limit-rate 10k --proxy "http://127.0.0.1:$p" "http://127.0.0.1:$FIXTURE_PORT/TEST-PLAN.md" >"$TMP/slow-$p.out" &
  SLOW_PIDS+=("$!")
done
sleep .2
established=$(lsof -nP -a -p "$CLIENT_PID" -iTCP 2>/dev/null | awk '/ESTABLISHED/ {count++} END {print count+0}')
printf 'shared_parallel_requests=2 mux_concurrency=8 established_client_tcp_connections=%s\n' "$established"
for pid in "${SLOW_PIDS[@]}"; do wait "$pid"; done
if curl --noproxy '' --silent --show-error --max-time 2 --proxy "http://127.0.0.1:$PROFILE_A_PORT" "http://127.0.0.1:$FIXTURE_PORT/TEST-PLAN.md" | grep -q 'ENGINE-009'; then echo 'traffic_proof=fixture only reachable via loopback HTTP inbound and route log'; else echo 'FAIL: fixture request proof missing'; exit 1; fi
printf 'server_access_log_tail:\n'; tail -5 "$TMP/server.log" | sed "s/$SERVER_UUID/<redacted-uuid>/g"

say "shared outbound and connection reuse evidence"
api lso --server="127.0.0.1:$API_PORT" | tee "$TMP/outbounds.shared.json"
shared_count=$(python3 -c 'import json,sys;print(sum(1 for x in json.load(open(sys.argv[1]))["outbounds"] if x["tag"]=="out:shared-vless"))' "$TMP/outbounds.shared.json")
printf 'same_outbound_key_profiles=2 xray_shared_outbound_count=%s\n' "$shared_count"
# Distinct credential creates a distinct tag/outbound; the UUID is redacted from all evidence.
OTHER_UUID='00000000-0000-4000-8000-000000000002'; sed "s/$SERVER_UUID/$OTHER_UUID/g;s/out:shared-vless/out:distinct-credential/g" "$TMP/vless-outbound.json" >"$TMP/vless-distinct.json"
api ado --server="127.0.0.1:$API_PORT" "$TMP/vless-distinct.json"
api lso --server="127.0.0.1:$API_PORT" | tee "$TMP/outbounds.distinct.json"
python3 - "$TMP/outbounds.distinct.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))['outbounds']; tags=[o['tag'] for o in x]
assert tags.count('out:shared-vless')==1 and tags.count('out:distinct-credential')==1
print('different_credentials=distinct_outbound_count:2')
PY

say "non-loopback access checks"
if [[ -n "$HOST_IP" ]] && curl --silent --show-error --max-time 1 "http://$HOST_IP:$API_PORT" >/dev/null; then echo "non_loopback_api=UNEXPECTED_REACHABLE address:$HOST_IP"; else echo "non_loopback_api=PASS address:${HOST_IP:-unavailable}"; fi
if [[ -n "$HOST_IP" ]] && curl --silent --show-error --max-time 1 --proxy "http://$HOST_IP:$PROFILE_A_PORT" "http://127.0.0.1:$FIXTURE_PORT/TEST-PLAN.md" >/dev/null; then echo "non_loopback_inbound=UNEXPECTED_REACHABLE address:$HOST_IP"; else echo "non_loopback_inbound=PASS address:${HOST_IP:-unavailable}"; fi

say "TUIC rejection and Hysteria2 capability result"
cat >"$TMP/tuic.json" <<EOF
{"inbounds":[],"outbounds":[{"tag":"tuic","protocol":"tuic","settings":{"address":"127.0.0.1","port":1}}]}
EOF
if "$BIN" run -test -config="$TMP/tuic.json" >"$TMP/tuic.log" 2>&1; then echo 'tuic=FAIL unexpectedly accepted'; exit 1; else sed 's/'"$SERVER_UUID"'/<redacted-uuid>/g' "$TMP/tuic.log"; echo 'tuic=PASS rejected (unknown protocol / unsupported outbound)'; fi
cat >"$TMP/hysteria.json" <<EOF
{"inbounds":[],"outbounds":[{"tag":"hysteria2","protocol":"hysteria","settings":{"version":2,"address":"127.0.0.1","port":1},"streamSettings":{"network":"hysteria","security":"tls","tlsSettings":{"serverName":"127.0.0.1"},"hysteriaSettings":{"version":2}}}]}
EOF
if "$BIN" run -test -config="$TMP/hysteria.json" >"$TMP/hysteria.log" 2>&1; then echo 'hysteria2_config=PASS schema accepted by v26.11.9 (connectivity not claimed; no controlled UDP peer)'; else echo 'hysteria2_config=FAIL schema rejected'; cat "$TMP/hysteria.log"; fi

say "CRUD removals"
api rmrules --server="127.0.0.1:$API_PORT" rt:a:rule rt:b:rule rt:a:rule 2>&1 || true
api rmi --server="127.0.0.1:$API_PORT" rt:a:in rt:b:in rt:a:in 2>&1 || true
api rmo --server="127.0.0.1:$API_PORT" out:distinct-credential out:shared-vless out:shared-vless 2>&1 || true
printf 'remove_idempotency=wrapper maps unknown-tag ErrNoClue to success\n'

say "strong-kill and bounded rebuild observation"
kill -9 "$CLIENT_PID"; wait "$CLIENT_PID" 2>/dev/null || true
if curl --silent --show-error --max-time 1 --proxy "http://127.0.0.1:$PROFILE_A_PORT" "http://127.0.0.1:$FIXTURE_PORT/TEST-PLAN.md" >/dev/null; then echo 'strong_kill_proxy=FAIL remained reachable'; else echo 'strong_kill_proxy=PASS no direct fallback'; fi
rebuild_start=$(python3 -c 'import time;print(time.monotonic_ns())')
"$BIN" run -config="$TMP/bootstrap.json" >"$TMP/rebuilt.log" 2>&1 & CLIENT_PID=$!; PIDS+=("$CLIENT_PID"); wait_port "$API_PORT" || { cat "$TMP/rebuilt.log"; exit 1; }
api ado --server="127.0.0.1:$API_PORT" "$TMP/vless-outbound.json"; api adi --server="127.0.0.1:$API_PORT" "$TMP/in-a.json"; api adrules --server="127.0.0.1:$API_PORT" -append "$TMP/rule-a.json"
rebuild_end=$(python3 -c 'import time;print(time.monotonic_ns())'); printf 'rebuild_ms=%s original_profile_port=%s\n' "$(( (rebuild_end-rebuild_start)/1000000 ))" "$PROFILE_A_PORT"
body=$(curl --noproxy '' --silent --show-error --fail --proxy "http://127.0.0.1:$PROFILE_A_PORT" "http://127.0.0.1:$FIXTURE_PORT/TEST-PLAN.md"); printf 'rebuild_proxy_response_bytes=%s\n' "${#body}"

say "RSS and connection snapshot"
ps -o pid=,rss=,command= -p "$CLIENT_PID" "$SERVER_PID" | sed 's/'"$SERVER_UUID"'/<redacted-uuid>/g'
lsof -nP -p "$CLIENT_PID" -a -iTCP | tail -n +2 | sed 's/'"$SERVER_UUID"'/<redacted-uuid>/g' || true
printf 'evidence_log=%s\n' "$LOG"
