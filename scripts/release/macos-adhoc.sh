#!/usr/bin/env bash
# Build and verify an explicitly classified macOS AdHoc application bundle.
#
# Manifest format (tab-separated):
#   role<TAB>path relative to .app<TAB>jit|none
#
# The manifest must contain exactly one `app<TAB>.<TAB>jit` row. Every regular
# executable file in the bundle must have one manifest row. The supported JIT
# roles are app, app-main, chromium, renderer, gpu, and utility (plus their
# `*-helper` spellings); helper, xray, and framework are non-JIT roles.
set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../.." && pwd)

APP=""
MANIFEST=""
OUT_DIR=""
ENTITLEMENTS="$REPO_ROOT/mien-desktop/entitlements.plist"
VERSION="${MIEN_VERSION:-0.0.0-dev}"
DMG_NAME=""
UPDATE_URL=""
UPDATE_SIGNATURE="${MIEN_UPDATE_SIGNATURE:-}"
UPDATE_SIGNATURE_FILE="${MIEN_UPDATE_SIGNATURE_FILE:-}"

usage() {
  cat >&2 <<'EOF'
Usage: macos-adhoc.sh --app PATH --manifest PATH --output-dir PATH [options]

Required:
  --app PATH                 .app bundle to sign
  --manifest PATH            tab-separated explicit signing manifest
  --output-dir PATH          directory for DMG and release metadata

Options:
  --entitlements PATH        Chromium JIT entitlements (default: mien-desktop/entitlements.plist)
  --version VERSION          version placed in the Tauri update manifest
  --dmg-name NAME            DMG filename (default: Mien-VERSION-macos-arm64.dmg)
  --update-url URL            URL placed in the Tauri update manifest
  --update-signature-file P  read the updater signature from a file
  --help
EOF
}

fail() {
  echo "macos-adhoc: $*" >&2
  exit 1
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --app) [ "$#" -ge 2 ] || fail "--app needs a value"; APP=$2; shift 2 ;;
    --manifest) [ "$#" -ge 2 ] || fail "--manifest needs a value"; MANIFEST=$2; shift 2 ;;
    --output-dir) [ "$#" -ge 2 ] || fail "--output-dir needs a value"; OUT_DIR=$2; shift 2 ;;
    --entitlements) [ "$#" -ge 2 ] || fail "--entitlements needs a value"; ENTITLEMENTS=$2; shift 2 ;;
    --version) [ "$#" -ge 2 ] || fail "--version needs a value"; VERSION=$2; shift 2 ;;
    --dmg-name) [ "$#" -ge 2 ] || fail "--dmg-name needs a value"; DMG_NAME=$2; shift 2 ;;
    --update-url) [ "$#" -ge 2 ] || fail "--update-url needs a value"; UPDATE_URL=$2; shift 2 ;;
    --update-signature-file) [ "$#" -ge 2 ] || fail "--update-signature-file needs a value"; UPDATE_SIGNATURE_FILE=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) fail "unknown argument: $1" ;;
  esac
done

[ -n "$APP" ] || fail "--app is required"
[ -n "$MANIFEST" ] || fail "--manifest is required"
[ -n "$OUT_DIR" ] || fail "--output-dir is required"
[ -d "$APP" ] || fail "app bundle does not exist: $APP"
case "$APP" in *.app) ;; *) fail "--app must point to a .app bundle: $APP" ;; esac
[ -f "$MANIFEST" ] || fail "signing manifest does not exist: $MANIFEST"
[ -f "$ENTITLEMENTS" ] || fail "entitlements file does not exist: $ENTITLEMENTS"
[ -r "$ENTITLEMENTS" ] || fail "entitlements file is not readable: $ENTITLEMENTS"
command -v codesign >/dev/null 2>&1 || fail "codesign is required (run this script on macOS or provide a test double)"
command -v hdiutil >/dev/null 2>&1 || fail "hdiutil is required (run this script on macOS or provide a test double)"

case "$VERSION" in *$'\n'*|*$'\r'*|*'"'*|*'\\'*) fail "version contains characters unsafe for JSON" ;; esac
[ -n "$DMG_NAME" ] || DMG_NAME="Mien-${VERSION}-macos-arm64.dmg"
case "$DMG_NAME" in */*|*$'\n'*|*$'\r'*) fail "DMG name must be a filename" ;; esac
[ -n "$UPDATE_URL" ] || UPDATE_URL="$DMG_NAME"
case "$UPDATE_URL" in *$'\n'*|*$'\r'*) fail "update URL contains a newline" ;; esac
mkdir -p "$OUT_DIR"

# Extract plist key names without depending on Python or jq, both of which are
# absent on some clean macOS runners. The source file is intentionally tiny and
# is checked as an exact two-key allowlist before it is ever passed to codesign.
plist_keys() {
  grep -Eo '<key>[^<]+</key>' "$1" 2>/dev/null | sed -E 's#</?key>##g' || true
}

assert_jit_entitlements() {
  local file=$1 keys expected key
  keys=$(plist_keys "$file" | LC_ALL=C sort)
  expected=$(printf '%s\n%s\n' \
    'com.apple.security.cs.allow-jit' \
    'com.apple.security.cs.allow-unsigned-executable-memory' | LC_ALL=C sort)
  [ "$keys" = "$expected" ] || fail "entitlements must contain exactly the two Chromium JIT keys: $file"
  for key in com.apple.security.cs.allow-jit com.apple.security.cs.allow-unsigned-executable-memory; do
    grep -A1 -F "<key>$key</key>" "$file" | grep -q '<true[[:space:]]*/>' || \
      fail "entitlement must be true: $key"
  done
}

assert_jit_entitlements "$ENTITLEMENTS"

roles=()
paths=()
kinds=()
app_rows=0
line_no=0
while IFS=$'\t' read -r role path kind extra || [ -n "${role:-}" ]; do
  line_no=$((line_no + 1))
  role=${role%$'\r'}
  path=${path%$'\r'}
  kind=${kind%$'\r'}
  case "$role" in ''|'#'*) continue ;; esac
  [ -z "${extra:-}" ] || fail "manifest line $line_no has more than three columns"
  [ -n "$path" ] || fail "manifest line $line_no has an empty path"
  case "$path" in /*) fail "manifest line $line_no uses an absolute path" ;; esac
  case "/$path/" in */../*|*/.. ) fail "manifest line $line_no escapes the app bundle" ;; esac
  case "$role" in
    app|app-main|chromium|renderer|gpu|utility|renderer-helper|gpu-helper|utility-helper|chromium-helper) expected=jit ;;
    helper|other-helper|xray|framework) expected=none ;;
    *) fail "manifest line $line_no has unsupported role: $role" ;;
  esac
  [ "$kind" = "$expected" ] || fail "manifest line $line_no role $role must use entitlements class $expected"
  [ "$role" != app ] || {
    app_rows=$((app_rows + 1))
    [ "$path" = . ] || fail "the app row must use path ."
  }
  for previous_path in "${paths[@]:-}"; do
    [ "$previous_path" != "$path" ] || fail "manifest path is listed more than once: $path"
  done
  target="$APP/$path"
  if [ "$role" = app ]; then
    [ -d "$target" ] || fail "app manifest target is not a directory: $path"
  else
    [ -e "$target" ] || fail "manifest target does not exist: $path"
    [ -x "$target" ] || fail "manifest target is not executable: $path"
  fi
  roles+=("$role")
  paths+=("$path")
  kinds+=("$kind")
done < "$MANIFEST"
[ "$app_rows" -eq 1 ] || fail "manifest must contain exactly one app row"
[ "${#paths[@]}" -gt 1 ] || fail "manifest must enumerate nested executables"

# Compare the explicit manifest with every executable file under the bundle.
# This catches dropped helpers and arbitrary executable payloads before signing.
actual_list=$(mktemp "${TMPDIR:-/tmp}/mien-adhoc-actual.XXXXXX")
cleanup() { rm -f "$actual_list" "$TMP_DETAILS" "$TMP_ENTITLEMENTS"; }
TMP_DETAILS=$(mktemp "${TMPDIR:-/tmp}/mien-adhoc-details.XXXXXX")
TMP_ENTITLEMENTS=$(mktemp "${TMPDIR:-/tmp}/mien-adhoc-entitlements.XXXXXX")
trap cleanup EXIT
find "$APP" -type f -perm -111 -print | while IFS= read -r actual; do
  case "$actual" in
    "$APP"/*) printf '%s\n' "${actual#"$APP"/}" >> "$actual_list" ;;
    *) fail "executable escaped app bundle: $actual" ;;
  esac
done
while IFS= read -r actual; do
  found=0
  for path in "${paths[@]}"; do
    [ "$path" = "$actual" ] && found=1
  done
  [ "$found" -eq 1 ] || fail "unclassified executable in app bundle: $actual"
done < <(LC_ALL=C sort -u "$actual_list")

# Sign deepest nested targets first. The app row is always last, regardless of
# its position in the manifest; --deep is used only for verification below.
order=$(mktemp "${TMPDIR:-/tmp}/mien-adhoc-order.XXXXXX")
trap 'rm -f "$actual_list" "$TMP_DETAILS" "$TMP_ENTITLEMENTS" "$order"' EXIT
for i in "${!paths[@]}"; do
  [ "${roles[$i]}" = app ] && continue
  path=${paths[$i]}
  slashes=${path//[^\/]/}
  printf '%08d\t%08d\n' "${#slashes}" "$i" >> "$order"
done
while IFS=$'\t' read -r _ index; do
  [ -n "${index:-}" ] || continue
  target="$APP/${paths[$index]}"
  args=(--force --sign - --timestamp=none --options runtime)
  [ "${kinds[$index]}" = jit ] && args+=(--entitlements "$ENTITLEMENTS")
  codesign "${args[@]}" "$target"
done < <(LC_ALL=C sort -rn "$order")

# Sign the application bundle itself last, with the JIT entitlements.
for i in "${!roles[@]}"; do
  if [ "${roles[$i]}" = app ]; then
    codesign --force --sign - --timestamp=none --options runtime --entitlements "$ENTITLEMENTS" "$APP"
  fi
done

verify_target() {
  local target=$1 kind=$2 details entitlements actual_keys jit_key
  codesign --verify --strict --verbose=2 "$target"
  details=$(mktemp "${TMPDIR:-/tmp}/mien-adhoc-detail.XXXXXX")
  if ! codesign -dv --verbose=4 "$target" >"$details" 2>&1; then
    rm -f "$details"
    fail "codesign display failed: $target"
  fi
  grep -q 'Signature=adhoc' "$details" || { rm -f "$details"; fail "not an AdHoc signature: $target"; }
  # Real `codesign -dv` reports the hardened-runtime bit as
  # `CodeDirectory ... flags=0x10002(adhoc,runtime)`; it never prints a bare
  # `Flags=` line for an ad-hoc signature. Match the flag token instead of a
  # made-up key name, or every genuine signature is rejected.
  grep -Eq 'flags=0x[0-9a-fA-F]+\([^)]*runtime[^)]*\)' "$details" \
    || { rm -f "$details"; fail "hardened runtime flag missing: $target"; }
  rm -f "$details"
  if [ "$kind" = jit ]; then
    if ! codesign -d --entitlements :- "$target" >"$TMP_ENTITLEMENTS" 2>&1; then
      fail "cannot read JIT entitlements: $target"
    fi
    actual_keys=$(plist_keys "$TMP_ENTITLEMENTS" | LC_ALL=C sort)
    jit_key=$(printf '%s\n%s\n' \
      'com.apple.security.cs.allow-jit' \
      'com.apple.security.cs.allow-unsigned-executable-memory' | LC_ALL=C sort)
    [ "$actual_keys" = "$jit_key" ] || fail "JIT target does not have exactly the two allowed entitlements: $target"
  else
    if ! codesign -d --entitlements :- "$target" >"$TMP_ENTITLEMENTS" 2>&1; then
      fail "cannot read entitlements: $target"
    fi
    if plist_keys "$TMP_ENTITLEMENTS" | grep -Eq 'com\.apple\.security\.cs\.(allow-jit|allow-unsigned-executable-memory)'; then
      fail "non-JIT target carries Chromium JIT entitlements: $target"
    fi
  fi
}

# Required structural verification uses --deep exactly once, on the app; each
# manifest target is then checked separately so an unlisted executable cannot
# be hidden by a recursive verification shortcut.
codesign --verify --deep --strict --verbose=2 "$APP"
for i in "${!paths[@]}"; do
  verify_target "$APP/${paths[$i]}" "${kinds[$i]}"
done

DMG_PATH="$OUT_DIR/$DMG_NAME"
hdiutil create -volname "Mien $VERSION" -srcfolder "$APP" -ov -format UDZO "$DMG_PATH" >/dev/null
[ -f "$DMG_PATH" ] || fail "hdiutil did not create DMG: $DMG_PATH"
cp "$ENTITLEMENTS" "$OUT_DIR/entitlements.plist"

if [ -n "$UPDATE_SIGNATURE_FILE" ]; then
  [ -f "$UPDATE_SIGNATURE_FILE" ] || fail "update signature file does not exist: $UPDATE_SIGNATURE_FILE"
  UPDATE_SIGNATURE=$(cat "$UPDATE_SIGNATURE_FILE")
fi
json_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}
version_json=$(json_escape "$VERSION")
url_json=$(json_escape "$UPDATE_URL")
signature_json=$(json_escape "$UPDATE_SIGNATURE")
cat > "$OUT_DIR/latest.json" <<EOF
{
  "version": "$version_json",
  "notes": "Mien macOS AdHoc update",
  "platforms": {
    "darwin-aarch64": {
      "signature": "$signature_json",
      "url": "$url_json"
    }
  }
}
EOF

sha256_file() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else fail "shasum or sha256sum is required to produce SHA256SUMS"; fi
}
(
  cd "$OUT_DIR"
  for artifact in "$DMG_NAME" entitlements.plist latest.json; do
    [ -f "$artifact" ] || fail "missing release artifact: $artifact"
    printf '%s  %s\n' "$(sha256_file "$artifact")" "$artifact"
  done
) > "$OUT_DIR/SHA256SUMS"

printf 'AdHoc package ready: %s\n' "$OUT_DIR"
printf 'Update manifest: %s/latest.json\n' "$OUT_DIR"
printf 'Updater signature is %s; CI must sign latest.json before publication.\n' \
  "$([ -n "$UPDATE_SIGNATURE" ] && printf 'present' || printf 'not present')"
