#!/usr/bin/env bash
# Install the pinned SBOM and vulnerability scanners into a private directory.
#
# Versions and SHA-256 digests are constants here, not workflow inputs: a tag is
# not a version and `latest` would make every scan a different tool. Each
# download is verified before it is extracted, and a mismatch is a hard failure.
#
# Usage: deploy/docker/install-scanners.sh [--dir DIR] [--platform OS/ARCH]
#   --dir       install root; binaries land in DIR/bin (default: target/scanners)
#   --platform  override detection, e.g. linux/amd64 (auto-detected otherwise)
#
# Exit codes: 0 installed; 2 blocked (unsupported platform, missing curl/tar,
# digest mismatch, download failure). The printer never claims an install that
# did not verify.
set -euo pipefail

SYFT_VERSION=v1.57.2
TRIVY_VERSION=v0.76.2

# sha256 of the official release archives for the platforms this project runs on
# (GitHub Actions linux/amd64, Incus linux/arm64, developer macOS/arm64).
syft_digest() {
  case "$1" in
    linux/amd64) echo ad92bd3ec1b94a3df01d4d0aec199afa9c42b72e86da436ff5f837914d32706b ;;
    linux/arm64) echo 8efe605309c98798e1ee0cffdcf8c05ca26819b4cd44c0b03650f1e963e9bdb6 ;;
    darwin/amd64) echo 73c6100bc43fbcc1c99aed3428d859c6289c4076be7e296dde7e427324c15908 ;;
    darwin/arm64) echo 38afbcc03a9d9afac2d7572432fd2c435e051e03930582ea90f4710ffb515224 ;;
    *) return 1 ;;
  esac
}

trivy_digest() {
  case "$1" in
    linux/amd64) echo bc0fc726e017b8927a62b2d3531d7a84ee01769daf5c99ed866dced54089fdbf ;;
    linux/arm64) echo ab9d51f86e6a5fb936f605faff6b593ac51bc378fff4d3e8b9d58358b9c367c3 ;;
    darwin/amd64) echo e08bb9ea449ecf66c6bfb03cbb8697c503ff33d35fc585c9747e3eaeb7ed4c7b ;;
    darwin/arm64) echo ae7ea358326960406b703c77b2ac490190797d80b306b5b50c2a27221da0a5b8 ;;
    *) return 1 ;;
  esac
}

syft_asset() {
  case "$1" in
    linux/amd64) echo syft_${SYFT_VERSION#v}_linux_amd64.tar.gz ;;
    linux/arm64) echo syft_${SYFT_VERSION#v}_linux_arm64.tar.gz ;;
    darwin/amd64) echo syft_${SYFT_VERSION#v}_darwin_amd64.tar.gz ;;
    darwin/arm64) echo syft_${SYFT_VERSION#v}_darwin_arm64.tar.gz ;;
  esac
}

trivy_asset() {
  case "$1" in
    linux/amd64) echo trivy_${TRIVY_VERSION#v}_Linux-64bit.tar.gz ;;
    linux/arm64) echo trivy_${TRIVY_VERSION#v}_Linux-ARM64.tar.gz ;;
    darwin/amd64) echo trivy_${TRIVY_VERSION#v}_macOS-64bit.tar.gz ;;
    darwin/arm64) echo trivy_${TRIVY_VERSION#v}_macOS-ARM64.tar.gz ;;
  esac
}

SCRIPT_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd)
DEST=$ROOT/target/scanners
PLATFORM=

usage() { sed -n '2,/^set -euo pipefail$/p' "$0" | sed -e '/^set -euo pipefail$/d' -e 's/^# \{0,1\}//'; }
blocked() { echo "install-scanners.sh: BLOCKED $*" >&2; exit 2; }

while [ $# -gt 0 ]; do
  case "$1" in
    --dir) [ $# -ge 2 ] || blocked "--dir requires a value"; DEST=$2; shift 2 ;;
    --platform) [ $# -ge 2 ] || blocked "--platform requires a value"; PLATFORM=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) blocked "unknown argument $1 (see --help)" ;;
  esac
done

if [ -z "$PLATFORM" ]; then
  case "$(uname -s)" in
    Linux) OS=linux ;;
    Darwin) OS=darwin ;;
    *) blocked "unsupported_os: $(uname -s); only Linux and macOS are provisioned" ;;
  esac
  case "$(uname -m)" in
    x86_64|amd64) ARCH=amd64 ;;
    arm64|aarch64) ARCH=arm64 ;;
    *) blocked "unsupported_arch: $(uname -m)" ;;
  esac
  PLATFORM=$OS/$ARCH
fi

command -v curl >/dev/null 2>&1 || blocked "missing_curl"
command -v tar >/dev/null 2>&1 || blocked "missing_tar"
SYFT_SHA=$(syft_digest "$PLATFORM") || blocked "unsupported_platform: $PLATFORM has no pinned syft archive"
TRIVY_SHA=$(trivy_digest "$PLATFORM") || blocked "unsupported_platform: $PLATFORM has no pinned trivy archive"

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

mkdir -p "$DEST/bin"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/mien-scanners.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

install_one() {
  tool=$1 version=$2 expected=$3 archive=$4 url=$5
  target=$DEST/bin/$tool
  if [ -x "$target" ] && [ "$("$target" --version 2>&1 | head -1)" != "" ]; then
    installed=$("$target" --version 2>&1 | sed -n 's/^Version:[[:space:]]*//p' | head -1)
    [ -n "$installed" ] || installed=$("$target" version 2>/dev/null | sed -n 's/.*"version":[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
    if [ "$installed" = "${version#v}" ]; then
      echo "scanners: $tool $version already present at $target"
      return 0
    fi
  fi
  echo "scanners: downloading $tool $version ($archive) for $PLATFORM"
  curl -fsSL --retry 5 --retry-all-errors --retry-delay 2 -o "$WORK/$archive" "$url" \
    || blocked "download_failed: $url"
  actual=$(hash_file "$WORK/$archive")
  [ "$actual" = "$expected" ] \
    || blocked "digest_mismatch: $archive expected $expected, got $actual"
  tar -xzf "$WORK/$archive" -C "$DEST/bin" "$tool" || blocked "extract_failed: $archive"
  chmod 0755 "$DEST/bin/$tool"
  echo "scanners: $tool $version verified sha256=$expected"
}

install_one syft "$SYFT_VERSION" "$SYFT_SHA" "$(syft_asset "$PLATFORM")" \
  "https://github.com/anchore/syft/releases/download/$SYFT_VERSION/$(syft_asset "$PLATFORM")"
install_one trivy "$TRIVY_VERSION" "$TRIVY_SHA" "$(trivy_asset "$PLATFORM")" \
  "https://github.com/aquasecurity/trivy/releases/download/$TRIVY_VERSION/$(trivy_asset "$PLATFORM")"

echo "scanners: dir=$DEST/bin syft=$SYFT_VERSION trivy=$TRIVY_VERSION platform=$PLATFORM"
