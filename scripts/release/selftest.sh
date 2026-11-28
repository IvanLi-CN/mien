#!/usr/bin/env bash
# Exercise the signing manifest and entitlement guardrails without macOS tools.
set -euo pipefail

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SIGNER="$SCRIPT_DIR/macos-adhoc.sh"
[ -x "$SIGNER" ] || { echo "selftest: signer is not executable" >&2; exit 1; }

root=$(mktemp -d "${TMPDIR:-/tmp}/mien-adhoc-selftest.XXXXXX")
cleanup() { rm -rf "$root"; }
trap cleanup EXIT
stub_bin="$root/bin"
mkdir -p "$stub_bin"

cat > "$stub_bin/codesign" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
jit=0
for arg in "$@"; do [ "$arg" = --entitlements ] && jit=1; done
last=${!#}
marker="$last.mien-signature"
[ -d "$last" ] && marker="$last/.mien-signature"
case "${1:-}" in
  -d)
    if [ -f "$marker" ] && grep -q '^jit$' "$marker"; then
      cat <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.cs.allow-jit</key><true/>
<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
</dict></plist>
PLIST
    else
      printf '%s\n' '<?xml version="1.0"?><plist version="1.0"><dict/></plist>'
    fi
    ;;
  -dv)
    # Mirrors real `codesign -dv --verbose=4` for an ad-hoc, hardened-runtime
    # signature: the runtime bit lives in the CodeDirectory flags, not in a
    # `Flags=` line. A stub that printed `Flags=...` let a broken check pass.
    printf 'Executable=%s\nCodeDirectory v=20500 size=35889 flags=0x10002(adhoc,runtime) hashes=1+1 location=embedded\nSignature=adhoc\nTeamIdentifier=not set\n' "$last"
    ;;
  --verify)
    printf '%s: valid on mock host\n' "$last"
    ;;
  *)
    if [ "$jit" -eq 1 ]; then printf 'jit\n' > "$marker"; else printf 'none\n' > "$marker"; fi
    ;;
esac
STUB
chmod +x "$stub_bin/codesign"

cat > "$stub_bin/hdiutil" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' 'mock DMG' > "${!#}"
STUB
chmod +x "$stub_bin/hdiutil"

make_entitlements() {
  cat > "$1" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>com.apple.security.cs.allow-jit</key><true/>
<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
</dict></plist>
PLIST
}

make_app() {
  local app=$1
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks"
  printf '#!/bin/sh\n' > "$app/Contents/MacOS/Mien"
  printf '#!/bin/sh\n' > "$app/Contents/Frameworks/Chromium"
  printf '#!/bin/sh\n' > "$app/Contents/Frameworks/RendererHelper"
  printf '#!/bin/sh\n' > "$app/Contents/Frameworks/OtherHelper"
  chmod +x "$app/Contents/MacOS/Mien" "$app/Contents/Frameworks/Chromium" \
    "$app/Contents/Frameworks/RendererHelper" "$app/Contents/Frameworks/OtherHelper"
}

manifest="$root/signing.tsv"
cat > "$manifest" <<'EOF'
# role<TAB>relative path<TAB>entitlements class
app	.	jit
app-main	Contents/MacOS/Mien	jit
chromium	Contents/Frameworks/Chromium	jit
renderer-helper	Contents/Frameworks/RendererHelper	jit
helper	Contents/Frameworks/OtherHelper	none
EOF
entitlements="$root/entitlements.plist"
make_entitlements "$entitlements"
app="$root/Mien.app"
make_app "$app"
PATH="$stub_bin:$PATH" "$SIGNER" --app "$app" --manifest "$manifest" \
  --entitlements "$entitlements" --output-dir "$root/out" --version 0.1.0 \
  --update-url https://example.invalid/mien/latest.json
[ -s "$root/out/Mien-0.1.0-macos-arm64.dmg" ]
[ -s "$root/out/SHA256SUMS" ]
[ -s "$root/out/entitlements.plist" ]
[ -s "$root/out/latest.json" ]
grep -q 'Signature=adhoc' "$root/out/latest.json" && {
  echo 'selftest: update manifest unexpectedly contains codesign output' >&2
  exit 1
}

echo 'selftest: positive classified bundle passed'
bad_entitlements="$root/bad-entitlements.plist"
make_entitlements "$bad_entitlements"
printf '<key>com.apple.security.get-task-allow</key><true/>\n' >> "$bad_entitlements"
if PATH="$stub_bin:$PATH" "$SIGNER" --app "$app" --manifest "$manifest" \
  --entitlements "$bad_entitlements" --output-dir "$root/out-bad-entitlements" >/dev/null 2>&1; then
  echo 'selftest: extra entitlement was accepted' >&2
  exit 1
fi
echo 'selftest: extra entitlement failed closed'
printf '#!/bin/sh\n' > "$app/Contents/Frameworks/Unclassified"
chmod +x "$app/Contents/Frameworks/Unclassified"
if PATH="$stub_bin:$PATH" "$SIGNER" --app "$app" --manifest "$manifest" \
  --entitlements "$entitlements" --output-dir "$root/out-unclassified" >/dev/null 2>&1; then
  echo 'selftest: unclassified executable was accepted' >&2
  exit 1
fi
echo 'selftest: unclassified executable failed closed'
echo 'selftest: all checks passed'
