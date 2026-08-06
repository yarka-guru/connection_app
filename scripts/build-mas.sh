#!/usr/bin/env bash
# Builds the Mac App Store variant: no updater, sandboxed, signed with Apple
# Distribution, packaged as a .pkg for App Store Connect.
#
#   ./scripts/build-mas.sh [--upload]
#
# Requires:
#   - apple-distribution.p12 + mac-installer.p12 + mas-p12-password.txt in
#     ~/Documents/09 - Security & Recovery/Apple Developer ID
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
# Universal by default. App Store Connect rejects an arm64-only build unless
# the deployment target is macOS 12+ (error 90869), and the direct distribution
# already supports Intel — shipping an Apple-silicon-only Store build would be a
# narrowing no user asked for.
TARGET="${MAS_TARGET:-universal-apple-darwin}"
API_KEY_ID="GBQH68KN3W"
API_ISSUER="799a6169-6e5d-4fc9-bac6-38993ccf145e"

cd "$REPO_ROOT"

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }
die()  { printf '\033[31mERROR: %s\033[0m\n' "$1" >&2; exit 1; }

step "Checking prerequisites"
[ -f "$PROFILE" ] || die "missing provisioning profile: $PROFILE"
[ -f "$CREDS/apple-distribution.p12" ] || die "missing apple-distribution.p12"
[ -f "$CREDS/mac-installer.p12" ]      || die "missing mac-installer.p12"
[ -f "$CREDS/mas-p12-password.txt" ]   || die "missing mas-p12-password.txt"
echo "  profile and both .p12 bundles present"

# Sign from a dedicated keychain rather than the login keychain.
#
# codesign needs the private key's partition list to include it, and setting
# that on the login keychain requires the account password — which this script
# must not handle. Signing from the login keychain without it fails with
# "errSecInternalComponent", which is what happened on the first run here.
#
# A throwaway keychain whose password this script chooses avoids the problem
# entirely, keeps the login keychain untouched, and mirrors what CI does.
KEYCHAIN="$(mktemp -u "${TMPDIR:-/tmp}/connectionapp-mas-XXXXXX").keychain"
# openssl rand rather than `tr -dc < /dev/urandom | head -c`: head closes the
# pipe, tr takes SIGPIPE, and under `set -o pipefail` that aborts the script
# with 141 before it does anything.
KEYCHAIN_PW="$(openssl rand -hex 24)"
ORIG_KEYCHAINS="$(security list-keychains -d user | tr -d '"' | xargs)"

cleanup_keychain() {
  # shellcheck disable=SC2086
  security list-keychains -d user -s $ORIG_KEYCHAINS >/dev/null 2>&1 || true
  security delete-keychain "$KEYCHAIN" >/dev/null 2>&1 || true
}

# Tauri scans capabilities/ at build time regardless of cargo features, so a
# permission naming the unregistered updater plugin aborts the build. Move it
# aside for the duration and restore it however this script exits.
CAP="src-tauri/capabilities/updater.json"
CAP_BAK="$(mktemp -t updater-capability)"
RESTORE=0
cleanup() {
  if [ "$RESTORE" = "1" ] && [ ! -f "$CAP" ]; then
    mv "$CAP_BAK" "$CAP"
    echo "  restored $CAP"
  fi
  rm -f "$CAP_BAK"
  cleanup_keychain
}
trap cleanup EXIT

if [ -f "$CAP" ]; then
  step "Removing the updater capability for this build"
  mv "$CAP" "$CAP_BAK"
  RESTORE=1
fi

step "Building without the updater feature"
# Compile and bundle are separate steps here for one reason: Tauri's universal
# build lipos the main binary but not the second one. The bundler then dies with
# `connection-app-cli does not exist`, because the app bundle ships both. So
# compile first, lipo the CLI by hand, then bundle.
#
# --no-sign because Tauri cannot embed a provisioning profile; this script signs
# afterwards. Everything after `--` goes to cargo.
npx tauri build \
  --target "$TARGET" \
  --config src-tauri/tauri.mas.conf.json \
  --no-bundle \
  --no-sign \
  -- --no-default-features --features gui

if [ "$TARGET" = "universal-apple-darwin" ]; then
  step "Producing a universal connection-app-cli"
  CLI_UNIVERSAL="src-tauri/target/universal-apple-darwin/release/connection-app-cli"
  if [ ! -f "$CLI_UNIVERSAL" ]; then
    for arch in aarch64-apple-darwin x86_64-apple-darwin; do
      [ -f "src-tauri/target/$arch/release/connection-app-cli" ] \
        || die "missing CLI slice for $arch"
    done
    lipo -create \
      "src-tauri/target/aarch64-apple-darwin/release/connection-app-cli" \
      "src-tauri/target/x86_64-apple-darwin/release/connection-app-cli" \
      -output "$CLI_UNIVERSAL"
  fi
  lipo -archs "$CLI_UNIVERSAL" | sed 's/^/  CLI architectures: /'
fi

step "Bundling"
npx tauri bundle \
  --target "$TARGET" \
  --bundles app \
  --config src-tauri/tauri.mas.conf.json \
  --no-sign

APP="src-tauri/target/$TARGET/release/bundle/macos/ConnectionApp.app"
[ -d "$APP" ] || die "no app bundle at $APP"

step "Embedding the provisioning profile"
# `cat >` rather than `cp`: the archived profile was downloaded through a
# browser and carries com.apple.quarantine, and cp brings extended attributes
# along. Apple rejects any package containing a quarantined file —
# ITMS-91109, which is exactly how the first upload died. Redirecting through a
# new file inherits nothing.
cat "$PROFILE" > "$APP/Contents/embedded.provisionprofile"
# The archived profile is mode 600, and cp carries that across. Inside a package
# installed to /Applications as root, a 600 file is unreadable by the user
# running the app, which breaks signature verification. altool rejects it:
# "The installer package includes files that are only readable by the root
# user" (90255).
chmod 644 "$APP/Contents/embedded.provisionprofile"
echo "  embedded.provisionprofile written (0644)"

# Nothing in a shipped bundle may be unreadable by other users.
UNREADABLE="$(find "$APP" -type f ! -perm -o+r 2>/dev/null || true)"
if [ -n "$UNREADABLE" ]; then
  echo "$UNREADABLE" | sed 's/^/  fixing perms: /'
  find "$APP" -type f ! -perm -o+r -exec chmod o+r {} +
fi

step "Stripping extended attributes"
# Must happen before signing — clearing xattrs afterwards invalidates the
# signature. Apple rejects a package if any file carries com.apple.quarantine
# (ITMS-91109).
# `xattr -cr` is not portable — the xattr on this macOS has no -r and exits 64.
# Clear each entry explicitly instead, directories included.
find "$APP" -exec xattr -c {} + 2>/dev/null || true
QUARANTINED="$(find "$APP" -exec sh -c 'xattr "$1" 2>/dev/null | grep -q quarantine && echo "$1"' _ {} \; 2>/dev/null || true)"
if [ -n "$QUARANTINED" ]; then
  echo "$QUARANTINED" | sed 's/^/  still quarantined: /'
  die "quarantine attributes survived; Apple will reject the package"
fi
echo "  no quarantine attributes remain"

step "Setting the build number"
# App Store Connect consumes a build number even when the delivery later fails
# processing, so a retry with the same CFBundleVersion is refused as a
# duplicate — and the replacement must compare HIGHER than the consumed one.
#
# CFBundleVersion must be at most three period-separated integers (error 90257),
# so it cannot simply have a suffix appended. Set MAS_BUILD to the full value,
# e.g. MAS_BUILD=3.7.9 after 3.7.8 was consumed. CFBundleShortVersionString —
# the version users see — is left alone.
INFO="$APP/Contents/Info.plist"
SHORT_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$INFO")"
if [ -n "${MAS_BUILD:-}" ]; then
  case "$MAS_BUILD" in
    [0-9]*.[0-9]*.[0-9]*|[0-9]*.[0-9]*|[0-9]*) ;;
    *) die "MAS_BUILD must be up to three dot-separated integers, got '$MAS_BUILD'" ;;
  esac
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $MAS_BUILD" "$INFO"
fi
echo "  version $SHORT_VERSION, build $(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$INFO")"

step "Preparing a signing keychain"
P12_PW="$(cat "$CREDS/mas-p12-password.txt")"
security create-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"
# shellcheck disable=SC2086
security list-keychains -d user -s $ORIG_KEYCHAINS "$KEYCHAIN"
security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"
security set-keychain-settings -t 3600 -u "$KEYCHAIN"
# -A rather than -T: allow any tool to use these keys without a prompt.
# -T authorises named binaries, but productbuild still asked for confirmation
# through a GUI dialog that nothing could answer — the keychain password is
# random and generated above, and the run is non-interactive. It failed with
# CSSMERR_CSP_USER_CANCELED. -A is safe here precisely because this keychain
# holds nothing else and is destroyed a few seconds later.
security import "$CREDS/apple-distribution.p12" -k "$KEYCHAIN" -P "$P12_PW" -A >/dev/null
security import "$CREDS/mac-installer.p12" -k "$KEYCHAIN" -P "$P12_PW" -A >/dev/null
# Without this, codesign cannot reach the private key non-interactively and
# fails with errSecInternalComponent. The partition list is `apple-tool:,apple:`
# — `apple:` covers the Apple-signed tools, productbuild included. Do not add a
# `productbuild:` partition; there is no such identifier, and an earlier version
# of this script both used one and hid the command's output, so the failure was
# invisible until productbuild sat waiting on a GUI password prompt.
security set-key-partition-list -S apple-tool:,apple: \
  -s -k "$KEYCHAIN_PW" "$KEYCHAIN" >/dev/null \
  || die "set-key-partition-list failed; signing would hang on a GUI prompt"
security find-identity -v -p codesigning "$KEYCHAIN" | grep -q "$APP_IDENTITY" \
  || die "'$APP_IDENTITY' not usable from the signing keychain"
echo "  identities imported and usable"

step "Signing"
# Sign nested executables first, then the bundle. --deep is deprecated by Apple
# and silently mis-signs nested content.
#
# Nested binaries get the inherit entitlements, NOT the app's. Signing them with
# com.apple.application-identifier makes altool warn that a nested executable
# carries an application identifier without a matching provisioning profile
# (90885), which disqualifies the build from TestFlight — the one way to
# exercise the sandbox path before release. app-sandbox + inherit is what a
# helper executable is supposed to carry.
MAIN_BIN="$APP/Contents/MacOS/connection-app"
while IFS= read -r -d '' f; do
  [ "$f" = "$MAIN_BIN" ] && continue
  echo "  nested: $(basename "$f")"
  codesign --force --timestamp --options runtime \
    --entitlements src-tauri/Entitlements.mas.inherit.plist \
    --keychain "$KEYCHAIN" --sign "$APP_IDENTITY" "$f"
done < <(find "$APP/Contents/MacOS" -type f -perm +111 -print0)

echo "  main: $(basename "$MAIN_BIN")"
codesign --force --timestamp --options runtime \
  --entitlements src-tauri/Entitlements.mas.plist \
  --keychain "$KEYCHAIN" --sign "$APP_IDENTITY" "$MAIN_BIN"

codesign --force --timestamp \
  --entitlements src-tauri/Entitlements.mas.plist \
  --keychain "$KEYCHAIN" --sign "$APP_IDENTITY" "$APP"
echo "  bundle signed"

step "Verifying the app bundle"
"$REPO_ROOT/scripts/verify-signing.sh" --mas-binary "$APP" \
  || die "the app bundle failed verification"

step "Packaging"
PKG="$REPO_ROOT/ConnectionApp.pkg"
rm -f "$PKG"
# If a copy of the installer identity also sits in the login keychain without an
# authorized partition list, productbuild finds that one instead and blocks on a
# GUI password prompt with no way to answer it. Fail fast rather than hang.
if security find-identity -v ~/Library/Keychains/login.keychain-db 2>/dev/null \
     | grep -q "$PKG_IDENTITY"; then
  die "'$PKG_IDENTITY' is also in the login keychain; productbuild will prompt.
  Remove it:  security delete-identity -c '$PKG_IDENTITY' ~/Library/Keychains/login.keychain-db"
fi
productbuild --component "$APP" /Applications --keychain "$KEYCHAIN" --sign "$PKG_IDENTITY" "$PKG"
pkgutil --check-signature "$PKG" | sed 's/^/  /'

step "Validating with App Store Connect"
# Always validate before offering to upload. The first attempt here was rejected
# for three things no local check catches: an arm64-only slice (90869), a
# root-only-readable file (90255), and a signature with no application
# identifier (90886). Validation is free; a rejected upload is not.
xcrun altool --validate-app -f "$PKG" -t macos \
  --apiKey "$API_KEY_ID" --apiIssuer "$API_ISSUER" 2>&1 | tail -5 | sed 's/^/  /'

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
