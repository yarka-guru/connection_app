#!/usr/bin/env bash
# Builds the Mac App Store variant: no updater, sandboxed, signed with Apple
# Distribution, packaged as a .pkg for App Store Connect.
#
#   ./scripts/build-mas.sh [--upload]
#
# Requires:
#   - Apple Distribution + 3rd Party Mac Developer Installer identities in the
#     login keychain (see ~/Documents/09 - Security & Recovery/Apple Developer ID)
#   - ConnectionApp_MAS.provisionprofile alongside them
#
# Runs locally by design. MAS packaging fails with opaque ITMS errors, and
# debugging those through GitHub Actions logs costs far more than running it by
# hand until a build has been accepted once.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CREDS="$HOME/Documents/09 - Security & Recovery/Apple Developer ID"
PROFILE="$CREDS/ConnectionApp_MAS.provisionprofile"
APP_IDENTITY="Apple Distribution: Iaroslav Pyrogov (XG4FR287W6)"
PKG_IDENTITY="3rd Party Mac Developer Installer: Iaroslav Pyrogov (XG4FR287W6)"
TARGET="${MAS_TARGET:-aarch64-apple-darwin}"
API_KEY_ID="GBQH68KN3W"
API_ISSUER="799a6169-6e5d-4fc9-bac6-38993ccf145e"

cd "$REPO_ROOT"

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
die()  { printf '\033[31mERROR: %s\033[0m\n' "$1" >&2; exit 1; }

step "Checking prerequisites"
[ -f "$PROFILE" ] || die "missing provisioning profile: $PROFILE"
security find-identity -v -p codesigning | grep -q "$APP_IDENTITY" \
  || die "no '$APP_IDENTITY' in the keychain"
security find-identity -v | grep -q "$PKG_IDENTITY" \
  || die "no '$PKG_IDENTITY' in the keychain"
echo "  identities present, profile present"

# Tauri scans capabilities/ at build time regardless of cargo features, so a
# permission naming the unregistered updater plugin aborts the build. Move it
# aside for the duration and restore it however this script exits.
CAP="src-tauri/capabilities/updater.json"
CAP_BAK="$(mktemp -t updater-capability)"
RESTORE=0
restore_cap() {
  if [ "$RESTORE" = "1" ] && [ ! -f "$CAP" ]; then
    mv "$CAP_BAK" "$CAP"
    echo "  restored $CAP"
  fi
  rm -f "$CAP_BAK"
}
trap restore_cap EXIT

if [ -f "$CAP" ]; then
  step "Removing the updater capability for this build"
  mv "$CAP" "$CAP_BAK"
  RESTORE=1
fi

step "Building without the updater feature"
# --no-sign because Tauri cannot embed a provisioning profile; this script signs
# afterwards. Everything after `--` goes to cargo.
npx tauri build \
  --target "$TARGET" \
  --bundles app \
  --config src-tauri/tauri.mas.conf.json \
  --no-sign \
  -- --no-default-features --features gui

APP="src-tauri/target/$TARGET/release/bundle/macos/ConnectionApp.app"
[ -d "$APP" ] || die "no app bundle at $APP"

step "Embedding the provisioning profile"
cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"
echo "  embedded.provisionprofile written"

step "Signing"
# Sign nested executables first, then the bundle. --deep is deprecated by Apple
# and silently mis-signs nested content.
while IFS= read -r -d '' f; do
  echo "  nested: $(basename "$f")"
  codesign --force --timestamp --options runtime \
    --entitlements src-tauri/Entitlements.mas.plist \
    --sign "$APP_IDENTITY" "$f"
done < <(find "$APP/Contents/MacOS" -type f -perm +111 -print0)

codesign --force --timestamp \
  --entitlements src-tauri/Entitlements.mas.plist \
  --sign "$APP_IDENTITY" "$APP"
echo "  bundle signed"

step "Verifying the app bundle"
"$REPO_ROOT/scripts/verify-signing.sh" --mas-binary "$APP" \
  || die "the app bundle failed verification"

step "Packaging"
PKG="$REPO_ROOT/ConnectionApp.pkg"
productbuild --component "$APP" /Applications --sign "$PKG_IDENTITY" "$PKG"
pkgutil --check-signature "$PKG" | sed 's/^/  /'

step "Done"
echo "  $PKG"
if [ "${1:-}" = "--upload" ]; then
  step "Uploading to App Store Connect"
  xcrun altool --upload-app -f "$PKG" -t macos \
    --apiKey "$API_KEY_ID" --apiIssuer "$API_ISSUER"
else
  echo
  echo "  Re-run with --upload to send it to App Store Connect."
fi
