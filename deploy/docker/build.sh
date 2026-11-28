#!/usr/bin/env bash
# Build the production server image, or just its OS layer, from a staged context.
#
# Why this file exists: `Dockerfile.server` is not buildable from the repository
# root. It expects `mien-server`, `dist/`, the two verified archives and the
# seccomp file to sit beside it, so the old `docker build -f Dockerfile .` shape
# could never work and CI silently skipped it. This script performs the whole
# staging step and then the build, so CI and a local host run the same path.
#
# Usage:
#   deploy/docker/build.sh [options]
#
#   --tag NAME            image tag (default: mien-server:local)
#   --platform PLATFORM   target platform (default: linux/amd64)
#   --target STAGE        `runtime-base` builds only the base image plus apt
#                         packages; it needs no kernel/Xray archive, no server
#                         binary and no dist/, which is what makes scanning the
#                         OS layer possible without the 141 MiB kernel asset.
#                         `server` (default) builds the complete image.
#   --context-dir DIR     where to stage the build context
#                         (default: target/docker-context/mien-server)
#   --assets-dir DIR      directory holding chromium.tar.xz and xray.zip
#                         (default: $MIEN_ASSET_CACHE, else unset)
#   --server-binary PATH  prebuilt Linux x86-64 binary
#                         (default: target/x86_64-unknown-linux-gnu/release/mien-server)
#   --dist DIR            built web assets (default: dist)
#   --image-id-file PATH  write the built image id there
#   --prepare-only        stage the context and print the docker command; no
#                         BuildKit is required
#   --help
#
# Exit codes: 0 built or staged; 1 contract violation or build failure;
# 2 blocked (missing tool, missing asset, unusable artifact). No step is skipped
# silently: a missing input is an error, never a "SKIP" line.
set -euo pipefail

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd)

TAG=${MIEN_IMAGE_TAG:-mien-server:local}
PLATFORM=${MIEN_IMAGE_PLATFORM:-linux/amd64}
TARGET=${MIEN_IMAGE_TARGET:-server}
CONTEXT_DIR=${MIEN_BUILD_CONTEXT:-$ROOT/target/docker-context/mien-server}
ASSETS_DIR=${MIEN_ASSET_CACHE:-}
SERVER_BINARY=$ROOT/target/x86_64-unknown-linux-gnu/release/mien-server
DIST_DIR=$ROOT/dist
IMAGE_ID_FILE=
PREPARE_ONLY=0

usage() { sed -n '2,/^set -euo pipefail$/p' "$0" | sed -e '/^set -euo pipefail$/d' -e 's/^# \{0,1\}//'; }

die() { echo "build.sh: $*" >&2; exit 1; }
blocked() { echo "build.sh: BLOCKED $*" >&2; exit 2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --tag) [ $# -ge 2 ] || die "--tag requires a value"; TAG=$2; shift 2 ;;
    --platform) [ $# -ge 2 ] || die "--platform requires a value"; PLATFORM=$2; shift 2 ;;
    --target) [ $# -ge 2 ] || die "--target requires a value"; TARGET=$2; shift 2 ;;
    --context-dir) [ $# -ge 2 ] || die "--context-dir requires a value"; CONTEXT_DIR=$2; shift 2 ;;
    --assets-dir) [ $# -ge 2 ] || die "--assets-dir requires a value"; ASSETS_DIR=$2; shift 2 ;;
    --server-binary) [ $# -ge 2 ] || die "--server-binary requires a value"; SERVER_BINARY=$2; shift 2 ;;
    --dist) [ $# -ge 2 ] || die "--dist requires a value"; DIST_DIR=$2; shift 2 ;;
    --image-id-file) [ $# -ge 2 ] || die "--image-id-file requires a value"; IMAGE_ID_FILE=$2; shift 2 ;;
    --prepare-only) PREPARE_ONLY=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) die "unknown argument $1 (see --help)" ;;
  esac
done

case "$TARGET" in
  server|runtime-base) ;;
  *) die "--target must be 'server' or 'runtime-base', got $TARGET" ;;
esac

# `sha256sum` exists on Linux runners but not on macOS; `shasum -a 256` is the
# portable fallback, so a developer can run the same script before pushing.
hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

# The contract module is the single source of truth for the base digest, the apt
# list, the COPY sources and the asset digests. Prefer `node` (available on every
# GitHub runner and in the release host image); bun is an accepted fallback.
if command -v node >/dev/null 2>&1; then RUNNER=(node)
elif command -v bun >/dev/null 2>&1; then RUNNER=(bun)
else blocked "missing_node: install Node.js to run scripts/release/dockerfile.mjs"; fi
CONTRACT=("${RUNNER[@]}" "$ROOT/scripts/release/dockerfile.mjs")

BASE_IMAGE=$("${CONTRACT[@]}" base-image)
[ -n "$BASE_IMAGE" ] || die "cannot read BASE_IMAGE from deploy/docker/Dockerfile.server"

REQUIRE_ARTIFACTS=1
[ "$TARGET" = runtime-base ] && REQUIRE_ARTIFACTS=0

echo "build: root=$ROOT"
echo "build: target=$TARGET platform=$PLATFORM tag=$TAG"
echo "build: base_image=$BASE_IMAGE"

if [ "$REQUIRE_ARTIFACTS" = 1 ]; then
  "${CONTRACT[@]}" check --root "$ROOT" --require-artifacts \
    --server-binary "$SERVER_BINARY" --dist "$DIST_DIR"
else
  # The OS layer still copies nothing from the context: only the Dockerfile is
  # read. Validate the Dockerfile itself and leave the artefacts to the full build.
  "${CONTRACT[@]}" check --root "$ROOT"
fi

echo "build: context=$CONTEXT_DIR"
rm -rf "$CONTEXT_DIR"
mkdir -p "$CONTEXT_DIR/seccomp"

install -m 0644 "$SCRIPT_DIR/Dockerfile.server" "$CONTEXT_DIR/Dockerfile.server"
install -m 0755 "$SCRIPT_DIR/entrypoint.sh" "$CONTEXT_DIR/entrypoint.sh"
install -m 0755 "$SCRIPT_DIR/healthcheck.sh" "$CONTEXT_DIR/healthcheck.sh"
install -m 0755 "$SCRIPT_DIR/verify-server.sh" "$CONTEXT_DIR/verify-server.sh"
install -m 0644 "$SCRIPT_DIR/seccomp/chromium.json" "$CONTEXT_DIR/seccomp/chromium.json"

if [ "$REQUIRE_ARTIFACTS" = 1 ]; then
  install -m 0755 "$SERVER_BINARY" "$CONTEXT_DIR/mien-server"
  mkdir -p "$CONTEXT_DIR/dist"
  cp -R "$DIST_DIR/." "$CONTEXT_DIR/dist/"
  [ -s "$CONTEXT_DIR/dist/index.html" ] || die "staged dist has no index.html"

  [ -n "$ASSETS_DIR" ] || blocked \
    "missing_assets_dir: pass --assets-dir or MIEN_ASSET_CACHE with chromium.tar.xz and xray.zip (see deploy/docker/README.server.md)"
  for asset in chromium.tar.xz xray.zip; do
    [ -f "$ASSETS_DIR/$asset" ] || blocked "missing_asset: $ASSETS_DIR/$asset"
  done
  # Re-verify against the frozen manifests before the build even starts; the
  # Dockerfile re-checks inside the build, and both must agree.
  while read -r name digest; do
    actual=$(hash_file "$ASSETS_DIR/$name")
    [ "$actual" = "$digest" ] || die "asset $name sha256 mismatch: expected $digest, got $actual"
    install -m 0644 "$ASSETS_DIR/$name" "$CONTEXT_DIR/$name"
    echo "build: asset=$name sha256=$digest"
  done < <("${CONTRACT[@]}" assets)
fi

BUILD_ARGS=(--platform "$PLATFORM" --target "$TARGET" -f "$CONTEXT_DIR/Dockerfile.server" -t "$TAG")
DOCKER_CMD=(docker build "${BUILD_ARGS[@]}" "$CONTEXT_DIR")

if [ "$PREPARE_ONLY" = 1 ]; then
  echo "build: staged=${CONTEXT_DIR}"
  printf 'build: docker'
  printf ' %q' "${DOCKER_CMD[@]:1}"
  printf '\n'
  # Staging is the whole job in these modes: a host without Docker still proves
  exit 0
fi

command -v docker >/dev/null 2>&1 || blocked "missing_docker: no Docker CLI on PATH; nothing was built"
docker version >/dev/null 2>&1 || blocked "docker_daemon_unreachable: the Docker daemon did not answer"

echo "build: docker ${DOCKER_CMD[*]:1}"
"${DOCKER_CMD[@]}"

IMAGE_ID=$(docker image inspect --format '{{.Id}}' "$TAG")
[ -n "$IMAGE_ID" ] || die "docker build did not register $TAG"
echo "build: image=$TAG image_id=$IMAGE_ID"
if [ -n "$IMAGE_ID_FILE" ]; then
  mkdir -p "$(dirname -- "$IMAGE_ID_FILE")"
  printf '%s\n' "$IMAGE_ID" >"$IMAGE_ID_FILE"
  echo "build: wrote $IMAGE_ID_FILE"
fi
