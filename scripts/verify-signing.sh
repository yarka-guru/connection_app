#!/usr/bin/env bash
# Verifies macOS signing invariants for ConnectionApp.
#
#   ./scripts/verify-signing.sh                      # entitlements + workflow
#   ./scripts/verify-signing.sh --entitlements
#   ./scripts/verify-signing.sh --workflow
#   ./scripts/verify-signing.sh --artifacts App.app Disk.dmg
#   ./scripts/verify-signing.sh --p12 devid.p12 p12-password.txt
#
# Exit 0 = all assertions pass. Exit 1 = at least one failed.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FAILED=0

pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED=1; }

check_entitlements() {
  echo "Entitlements (direct build):"
  local plist="$REPO_ROOT/src-tauri/Entitlements.plist"

  if [ ! -f "$plist" ]; then
    fail "missing $plist"
    return
  fi

  # The direct build must not be sandboxed. Enabling app-sandbox repoints $HOME
  # to ~/Library/Containers/com.connection-app.desktop/Data, hiding every
  # existing user's ~/.connection-app data with no in-sandbox migration path.
  # App Sandbox is required only for the Mac App Store build, which will carry
  # its own Entitlements.mas.plist.
  if /usr/libexec/PlistBuddy -c "Print :com.apple.security.app-sandbox" "$plist" >/dev/null 2>&1; then
    fail "com.apple.security.app-sandbox is present"
    fail "  the direct build must NOT be sandboxed — it would repoint \$HOME to a"
    fail "  container and hide every existing user's ~/.connection-app data"
  else
    pass "app-sandbox absent"
  fi

  # The decisive check: actually run codesign against this file.
  #
  # On 2026-08-06 this plist carried an explanatory comment ending in
  # "verify-signing.sh --entitlements." — and "--" is illegal inside an XML
  # comment, so the file was not well-formed XML. codesign refused it with
  # "AMFIUnserializeXML: syntax error near line 17" and the release rolled back.
  #
  # The trap: `plutil -lint` reports OK on that exact file, and PlistBuddy reads
  # it fine. Apple's own plist tooling does not catch it. Only a real codesign
  # run does, which is why this check signs a throwaway binary rather than
  # linting. Keep this plist comment-free and the problem cannot recur.
  local probe="/tmp/verify-signing-entitlements-$$"
  cp /bin/echo "$probe"
  local out
  out="$(codesign --force --entitlements "$plist" -s - "$probe" 2>&1)"
  if grep -q "Failed to parse entitlements" <<<"$out"; then
    fail "codesign cannot parse the entitlements file:"
    fail "  ${out}"
    fail "  note: plutil -lint may still report OK on this file — trust codesign"
    fail "  a common cause is '--' inside an XML comment, which is illegal XML"
  else
    pass "codesign parses the entitlements file"
  fi
  rm -f "$probe"
}

check_workflow() {
  echo "Release workflow:"
  local wf="$REPO_ROOT/.github/workflows/release.yml"

  if [ ! -f "$wf" ]; then
    fail "missing $wf"
    return
  fi

  local v
  for v in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY \
           APPLE_TEAM_ID APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH; do
    if grep -qE "^[[:space:]]*${v}:" "$wf"; then
      pass "$v wired into the workflow"
    else
      fail "$v is not passed to tauri-action — builds would ship unsigned"
    fi
  done

  if grep -q "APPLE_API_KEY_P8" "$wf"; then
    pass "a step materializes the App Store Connect key on disk"
  else
    fail "nothing writes APPLE_API_KEY_P8 to a file; APPLE_API_KEY_PATH would dangle"
  fi

  # Tauri notarizes the .app but ships it inside a DMG it only signs. Gatekeeper
  # checks the DMG too when a user opens a downloaded one, so the DMG needs its
  # own notarytool submission. v3.7.8 shipped with the .app "accepted /
  # Notarized Developer ID" and the .dmg "rejected / Unnotarized Developer ID".
  if grep -q "notarytool submit" "$wf"; then
    pass "the DMG is submitted to notarytool separately"
  else
    fail "no notarytool submission for the DMG — Tauri only notarizes the .app,"
    fail "  so downloaded disk images would be rejected by Gatekeeper"
  fi
}

check_artifacts() {
  local app="$1" dmg="$2"
  echo "Built artifacts:"

  if [ ! -d "$app" ]; then fail "no such app bundle: $app"; return; fi
  if [ ! -f "$dmg" ]; then fail "no such disk image: $dmg"; return; fi

  local info
  info="$(codesign -dv --verbose=4 "$app" 2>&1 || true)"

  if grep -q "Authority=Developer ID Application: Iaroslav Pyrogov (XG4FR287W6)" <<<"$info"; then
    pass "signed with the expected Developer ID"
  else
    fail "wrong or missing signing authority"
  fi

  if grep -qE "flags=.*runtime" <<<"$info"; then
    pass "hardened runtime enabled"
  else
    fail "hardened runtime not enabled — notarization will be refused"
  fi

  if codesign -d --entitlements - "$app" 2>/dev/null | grep -q "app-sandbox"; then
    fail "the shipped app is sandboxed — existing users would lose sight of their data"
  else
    pass "shipped app is not sandboxed"
  fi

  if spctl -a -vvv -t exec "$app" 2>&1 | grep -q "source=Notarized Developer ID"; then
    pass "app accepted by Gatekeeper as notarized"
  else
    fail "app is not notarized"
  fi

  if spctl -a -vvv -t install "$dmg" 2>&1 | grep -q "source=Notarized Developer ID"; then
    pass "dmg accepted by Gatekeeper as notarized"
  else
    fail "dmg is not notarized"
  fi

  if xcrun stapler validate "$app" >/dev/null 2>&1; then
    pass "app has a stapled ticket"
  else
    fail "app is not stapled — it would fail Gatekeeper offline"
  fi

  if xcrun stapler validate "$dmg" >/dev/null 2>&1; then
    pass "dmg has a stapled ticket"
  else
    fail "dmg is not stapled"
  fi
}

# Apple's SecKeychainItemImport only verifies SHA-1 PKCS#12 MACs. OpenSSL 3.x
# defaults to a SHA-256 MAC with AES-256-CBC, which it rejects as
# "MAC verification failed during PKCS12 import (wrong password?)" — a
# misleading message, since the password is fine. This cost a rolled-back
# v3.7.8 release on 2026-08-06. Run this before putting a .p12 into
# APPLE_CERTIFICATE; it fails locally in seconds instead of after a tag.
check_p12() {
  local p12="$1" pwfile="$2"
  echo "Signing certificate bundle:"

  if [ ! -f "$p12" ];    then fail "no such .p12: $p12"; return; fi
  if [ ! -f "$pwfile" ]; then fail "no such password file: $pwfile"; return; fi

  local pw
  pw="$(cat "$pwfile")"

  local mac
  mac="$(openssl pkcs12 -in "$p12" -passin "pass:$pw" -info -nokeys -noout 2>&1 \
         | sed -n 's/^MAC: *\([a-z0-9]*\).*/\1/p')"

  if [ "$mac" = "sha1" ]; then
    pass "PKCS#12 MAC is sha1"
  else
    fail "PKCS#12 MAC is '${mac:-unreadable}', but Apple only accepts sha1"
    fail "  rebuild with: -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1"
  fi

  # The decisive check: does macOS itself accept it? Uses a throwaway keychain
  # added to the user search list, mirroring what tauri-action does in CI.
  local kc="/tmp/verify-signing-$$.keychain" orig
  orig="$(security list-keychains -d user | tr -d '"' | xargs)"
  security create-keychain -p verifypw "$kc" >/dev/null 2>&1
  # shellcheck disable=SC2086
  security list-keychains -d user -s $orig "$kc" >/dev/null 2>&1
  security unlock-keychain -p verifypw "$kc" >/dev/null 2>&1

  if security import "$p12" -k "$kc" -P "$pw" -T /usr/bin/codesign >/dev/null 2>&1; then
    pass "macOS security import accepts it"
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k verifypw "$kc" >/dev/null 2>&1
    if security find-identity -v -p codesigning "$kc" 2>/dev/null \
         | grep -q "Developer ID Application: Iaroslav Pyrogov (XG4FR287W6)"; then
      pass "yields a valid Developer ID codesigning identity"
    else
      fail "imported, but no valid Developer ID identity — is the G2 intermediate bundled?"
    fi
  else
    fail "macOS security import REJECTS it — CI would fail at the bundling step"
  fi

  # shellcheck disable=SC2086
  security list-keychains -d user -s $orig >/dev/null 2>&1
  security delete-keychain "$kc" >/dev/null 2>&1
}

case "${1:-}" in
  --entitlements) check_entitlements ;;
  --workflow)     check_workflow ;;
  --artifacts)
    if [ $# -ne 3 ]; then
      echo "usage: $0 --artifacts <path/to/App.app> <path/to/Disk.dmg>" >&2
      exit 2
    fi
    check_artifacts "$2" "$3"
    ;;
  --p12)
    if [ $# -ne 3 ]; then
      echo "usage: $0 --p12 <path/to.p12> <path/to/password-file>" >&2
      exit 2
    fi
    check_p12 "$2" "$3"
    ;;
  "")             check_entitlements; echo; check_workflow ;;
  *)              echo "unknown mode: $1" >&2; exit 2 ;;
esac

echo
if [ "$FAILED" -eq 0 ]; then
  echo "All checks passed."
else
  echo "Some checks FAILED."
fi
exit "$FAILED"
