#!/usr/bin/env bash
# Produce the image SBOM (SPDX + CycloneDX) and the vulnerability report.
#
# Two modes, both real: nothing here fabricates an image or a report.
#
#   deploy/docker/scan-image.sh --image mien-server:local
#       Scan an image that already exists in the local Docker daemon. Used on a
#       host that can build the full image (needs the 141 MiB kernel asset).
#
#   deploy/docker/scan-image.sh --docker-archive /path/image.tar [--label NAME]
#                                  [--image-ref REF] [--image-id ID]
#       Scan a docker-archive tarball. `build.sh --target runtime-base` plus
#       `docker save` produces one without the kernel/Xray assets, so CI can scan
#       the real OS layer (base image + every apt package) on every run. `--label`
#       only names the artifact files; `--image-ref`/`--image-id` record what the
#       archive was built from, because the tarball itself carries the tag.
#
# In both modes syft writes `artifacts/sbom/<name>.spdx.json` and
# `artifacts/sbom/<name>.cdx.json`, trivy writes `artifacts/scan/<name>.trivy.json`,
# and `scripts/release/scan-report.mjs` validates both SBOMs, applies
# `deploy/docker/security-exceptions.yaml` and writes `artifacts/scan/<name>.summary.json`.
# Exit codes: 0 clean; 1 findings/SBOM/exception violation; 2 blocked.
set -euo pipefail

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd)

IMAGE=
DOCKER_ARCHIVE=
LABEL=
IMAGE_REF=
IMAGE_ID=
ARTIFACT_ROOT=$ROOT/artifacts
SYFT=${MIEN_SYFT:-syft}
TRIVY=${MIEN_TRIVY:-trivy}
SYFT_VERSION=${MIEN_SYFT_VERSION:-v1.57.2}
TRIVY_VERSION=${MIEN_TRIVY_VERSION:-v0.76.2}
PLATFORM=linux/amd64

usage() { sed -n '2,/^set -euo pipefail$/p' "$0" | sed -e '/^set -euo pipefail$/d' -e 's/^# \{0,1\}//'; }
die() { echo "scan-image.sh: $*" >&2; exit 1; }
blocked() { echo "scan-image.sh: BLOCKED $*" >&2; exit 2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --image) [ $# -ge 2 ] || die "--image requires a value"; IMAGE=$2; shift 2 ;;
    --docker-archive) [ $# -ge 2 ] || die "--docker-archive requires a value"; DOCKER_ARCHIVE=$2; shift 2 ;;
    --label) [ $# -ge 2 ] || die "--label requires a value"; LABEL=$2; shift 2 ;;
    --image-ref) [ $# -ge 2 ] || die "--image-ref requires a value"; IMAGE_REF=$2; shift 2 ;;
    --image-id) [ $# -ge 2 ] || die "--image-id requires a value"; IMAGE_ID=$2; shift 2 ;;
    --artifacts-dir) [ $# -ge 2 ] || die "--artifacts-dir requires a value"; ARTIFACT_ROOT=$2; shift 2 ;;
    --syft) [ $# -ge 2 ] || die "--syft requires a value"; SYFT=$2; shift 2 ;;
    --trivy) [ $# -ge 2 ] || die "--trivy requires a value"; TRIVY=$2; shift 2 ;;
    --platform) [ $# -ge 2 ] || die "--platform requires a value"; PLATFORM=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) die "unknown argument $1 (see --help)" ;;
  esac
done

if [ -n "$IMAGE" ] && [ -n "$DOCKER_ARCHIVE" ]; then
  die "--image and --docker-archive are mutually exclusive"
fi
if [ -z "$IMAGE" ] && [ -z "$DOCKER_ARCHIVE" ]; then
  die "no scan target: pass --image or --docker-archive"
fi

if [ -n "$DOCKER_ARCHIVE" ]; then
  [ -f "$DOCKER_ARCHIVE" ] || blocked "missing_archive: $DOCKER_ARCHIVE"
  [ -s "$DOCKER_ARCHIVE" ] || blocked "empty_archive: $DOCKER_ARCHIVE"
fi

command -v "$SYFT" >/dev/null 2>&1 || blocked "missing_syft: install syft $SYFT_VERSION (anchore/syft) or pass --syft"
command -v "$TRIVY" >/dev/null 2>&1 || blocked "missing_trivy: install trivy $TRIVY_VERSION (aquasecurity/trivy) or pass --trivy"

if [ -n "$IMAGE" ]; then
  command -v docker >/dev/null 2>&1 || blocked "missing_docker: --image needs the Docker daemon; use --docker-archive"
  docker image inspect "$IMAGE" >/dev/null 2>&1 || blocked "image_not_found: $IMAGE is not in the local daemon"
  NAME=$(printf '%s' "$IMAGE" | tr '/:@' '---')
  SYFT_SOURCE="docker:$IMAGE"
  TRIVY_SOURCE=(image "$IMAGE")
else
  NAME=${LABEL:-$(basename -- "$DOCKER_ARCHIVE" .tar)}
  NAME=$(printf '%s' "$NAME" | tr '/:@' '---')
  SYFT_SOURCE="docker-archive:$DOCKER_ARCHIVE"
  # trivy reads an archive through `--input`; the archive must be a `docker save`
  # output of the exact image that was built.
  TRIVY_SOURCE=(image --input "$DOCKER_ARCHIVE")
fi

SBOM_DIR=$ARTIFACT_ROOT/sbom
SCAN_DIR=$ARTIFACT_ROOT/scan
mkdir -p "$SBOM_DIR" "$SCAN_DIR"
SPDX=$SBOM_DIR/$NAME.spdx.json
CDX=$SBOM_DIR/$NAME.cdx.json
TRIVY_JSON=$SCAN_DIR/$NAME.trivy.json
SUMMARY=$SCAN_DIR/$NAME.summary.json

echo "scan: target=${IMAGE:-$DOCKER_ARCHIVE} name=$NAME"
echo "scan: syft=$SYFT sbom_dir=$SBOM_DIR"
echo "scan: trivy=$TRIVY scan_dir=$SCAN_DIR"

SYFT_CHECK_FOR_APP_UPDATE=false "$SYFT" scan "$SYFT_SOURCE" \
  --source-name "$NAME" \
  -o "spdx-json=$SPDX" -o "cyclonedx-json=$CDX"

# Fail before trivy if syft produced an unusable SBOM: a scan of an empty file is
# not evidence, and trivy would happily scan nothing.
for file in "$SPDX" "$CDX"; do
  [ -s "$file" ] || die "syft produced an empty file: $file"
done

SYFT_REPORTED=$("$SYFT" version -o json 2>/dev/null | sed -n 's/.*"version":[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
TRIVY_REPORTED=$("$TRIVY" --version 2>/dev/null | sed -n 's/^Version:[[:space:]]*//p' | head -1)
TRIVY_DB_UPDATED=$("$TRIVY" --version 2>/dev/null | sed -n 's/^  UpdatedAt:[[:space:]]*//p' | head -1)
echo "scan: syft_version=${SYFT_REPORTED:-unknown} trivy_version=${TRIVY_REPORTED:-unknown} trivy_db=${TRIVY_DB_UPDATED:-unknown}"

# Severity and exit code are fixed here, not by a workflow input: this script is
# the only scanning path for the image, and HIGH/CRITICAL always fails.
set +e
TRIVY_CACHE_DIR=${TRIVY_CACHE_DIR:-$ROOT/target/trivy-cache} "$TRIVY" "${TRIVY_SOURCE[@]}" \
  --scanners vuln \
  --severity HIGH,CRITICAL \
  --exit-code 1 \
  --format json \
  --output "$TRIVY_JSON" \
  --quiet
TRIVY_STATUS=$?
set -e
[ -s "$TRIVY_JSON" ] || die "trivy wrote no report: $TRIVY_JSON"
# 1 means findings were found, which the report below decides about; anything else
# nonzero is a broken scan (no DB, unreadable archive) and must not look like one.
[ "$TRIVY_STATUS" -le 1 ] || die "trivy failed with exit $TRIVY_STATUS; see the log above"
echo "scan: trivy_exit=$TRIVY_STATUS (1 means HIGH/CRITICAL were found; that is not skipped)"

if [ -n "$IMAGE" ]; then
  IMAGE_REF=${IMAGE_REF:-$IMAGE}
  IMAGE_ID=$(docker image inspect --format '{{.Id}}' "$IMAGE")
else
  IMAGE_REF=${IMAGE_REF:-${LABEL:-docker-archive:$DOCKER_ARCHIVE}}
fi

if command -v node >/dev/null 2>&1; then JS=(node)
elif command -v bun >/dev/null 2>&1; then JS=(bun)
else die "missing_node: install Node.js (or bun) to run scripts/release/scan-report.mjs"; fi

set +e
MIEN_SYFT_VERSION=$SYFT_REPORTED MIEN_TRIVY_VERSION=$TRIVY_REPORTED \
  MIEN_TRIVY_DB_UPDATED_AT=$TRIVY_DB_UPDATED \
  "${JS[@]}" "$ROOT/scripts/release/scan-report.mjs" \
    --spdx "$SPDX" \
    --cyclonedx "$CDX" \
    --trivy "$TRIVY_JSON" \
    --exceptions "$SCRIPT_DIR/security-exceptions.yaml" \
    --summary "$SUMMARY" \
    --image "$IMAGE_REF" \
    ${IMAGE_ID:+--image-id "$IMAGE_ID"} \
    --platform "$PLATFORM" \
    --root "$ROOT" \
    --dockerfile deploy/docker/Dockerfile.server
REPORT_STATUS=$?
set -e
echo "scan: summary=$SUMMARY"
exit "$REPORT_STATUS"
