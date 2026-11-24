#!/usr/bin/env bash
# Generate the Tauri updater signing key outside the repository.
set -euo pipefail

fail() {
  echo "updater-keygen: $*" >&2
  exit 1
}

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)
KEY_PATH="${TAURI_KEY_PATH:-${HOME}/.config/mien/tauri-updater.key}"

usage() {
  cat >&2 <<'EOF'
Usage: updater-keygen.sh [--key-path PATH]

The key path must be outside this repository. The Tauri CLI prompts for the
private-key password; keep that password in a password manager, not in Git.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --key-path) [ "$#" -ge 2 ] || fail "--key-path needs a value"; KEY_PATH=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) fail "unknown argument: $1" ;;
  esac
done

case "$KEY_PATH" in
  /*) ;;
  *) KEY_PATH="$PWD/$KEY_PATH" ;;
esac
KEY_PATH=$(CDPATH= cd -- "$(dirname -- "$KEY_PATH")" && pwd)/$(basename -- "$KEY_PATH")
case "$KEY_PATH" in
  "$REPO_ROOT"|"$REPO_ROOT"/*) fail "private key path must be outside the repository: $KEY_PATH" ;;
esac
[ ! -e "$KEY_PATH" ] || fail "refusing to overwrite existing key: $KEY_PATH"
[ ! -e "${KEY_PATH}.pub" ] || fail "refusing to overwrite existing public key: ${KEY_PATH}.pub"
command -v tauri >/dev/null 2>&1 || fail "tauri CLI is required (install @tauri-apps/cli before running this script)"

mkdir -p "$(dirname -- "$KEY_PATH")"
old_umask=$(umask)
umask 077
log=$(mktemp "${TMPDIR:-/tmp}/mien-updater-keygen.XXXXXX")
trap 'rm -f "$log"' EXIT
# Suppress the CLI transcript: some versions echo generated key material. The
# password prompt is deliberately non-interactive here; use the Tauri CLI's
# normal local terminal prompt through its configured stdin/TTY.
if ! tauri signer generate --write-keys "$KEY_PATH" >"$log" 2>&1; then
  umask "$old_umask"
  rm -f "$KEY_PATH" "${KEY_PATH}.pub"
  fail "tauri signer generate failed; no key was retained"
fi
umask "$old_umask"
[ -s "$KEY_PATH" ] || fail "Tauri did not create a private key at $KEY_PATH"
[ -s "${KEY_PATH}.pub" ] || fail "Tauri did not create a public key at ${KEY_PATH}.pub"
chmod 600 "$KEY_PATH"
chmod 644 "${KEY_PATH}.pub"

cat <<EOF
Updater key generated locally.
Private key: $KEY_PATH
Public key:  ${KEY_PATH}.pub

Configure these GitHub [1menvironment secrets[0m in the protected release environment:
  TAURI_SIGNING_PRIVATE_KEY
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD
The private key content was not printed and was not written to the repository.
EOF
