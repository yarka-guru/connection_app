#!/usr/bin/env bash
# Verifies macOS signing invariants for ConnectionApp.
#
#   ./scripts/verify-signing.sh                      # entitlements + workflow
#   ./scripts/verify-signing.sh --entitlements
#   ./scripts/verify-signing.sh --workflow
#   ./scripts/verify-signing.sh --artifacts App.app Disk.dmg
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

  if /usr/libexec/PlistBuddy -c "Print :com.apple.security.app-sandbox" "$plist" >/dev/null 2>&1; then
    fail "com.apple.security.app-sandbox is present"
    fail "  the direct build must NOT be sandboxed — it would repoint \$HOME to a"
    fail "  container and hide every existing user's ~/.connection-app data"
  else
    pass "app-sandbox absent"
  fi
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
