# macOS Signing & Notarization (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship macOS builds that are signed with a Developer ID and notarized by Apple, so Gatekeeper launches them without warnings and the `xattr -cr` workaround can be deleted from the README.

**Architecture:** Three file changes plus one new verification script. `Entitlements.plist` loses `app-sandbox` so signing does not silently relocate `$HOME` to a sandbox container. `release.yml` gains the Apple credentials that `tauri-action` needs to sign, notarize, and staple. A `scripts/verify-signing.sh` script encodes every invariant as an executable assertion and doubles as the release gate.

**Tech Stack:** Tauri 2, `tauri-apps/tauri-action@v0`, GitHub Actions, macOS `codesign` / `spctl` / `stapler`, bash.

**Source spec:** `docs/superpowers/specs/2026-08-06-macos-signing-and-app-store-design.md`

## Global Constraints

- Team ID is `XG4FR287W6`.
- Signing identity is exactly `Developer ID Application: Iaroslav Pyrogov (XG4FR287W6)`.
- Bundle identifier is `com.connection-app.desktop` and must not change.
- **The direct build must never enable `com.apple.security.app-sandbox`.** Enabling it repoints `$HOME` to `~/Library/Containers/com.connection-app.desktop/Data`, making every existing user's `~/.connection-app` data invisible with no in-sandbox migration path.
- These repository secrets already exist and must not be renamed: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`, `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_P8`.
- **This repository has no JavaScript test runner.** There is no `npm test` script. Do not invent one. Rust tests run with `cargo test` from `src-tauri/` (67 tests across 8 modules).
- JS/Svelte linting is `npx @biomejs/biome check .`.
- Baseline version is `3.7.7`; this work releases as `3.7.8`.

---

## File Structure

| File | Responsibility |
|---|---|
| `scripts/verify-signing.sh` | **new** — executable assertions for every signing invariant: entitlements, workflow wiring, and built artifacts |
| `src-tauri/Entitlements.plist` | **modify** — direct-build entitlements; strip App Sandbox |
| `.github/workflows/release.yml` | **modify** — materialize the `.p8`, pass Apple credentials to `tauri-action` |
| `README.md` | **modify** — remove the `xattr -cr` workaround |

---

### Task 1: Verification script and de-sandboxed entitlements

The script is written first because it is the test for the entitlements change. It also becomes the permanent regression guard — this repo has already shipped a silently-broken release (v3.7.6, AppImage missing from bundle targets while CI stayed green), so asserting release invariants in code rather than in memory is the established lesson.

**Files:**
- Create: `scripts/verify-signing.sh`
- Modify: `src-tauri/Entitlements.plist`

**Interfaces:**
- Consumes: nothing
- Produces: `scripts/verify-signing.sh` with four modes — `--entitlements`, `--workflow`, `--artifacts <app> <dmg>`, and no-argument (runs `--entitlements` and `--workflow`). Exits `0` when all assertions pass, `1` otherwise.

- [ ] **Step 1: Write the failing test**

Create `scripts/verify-signing.sh`:

```bash
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
```

- [ ] **Step 2: Make it executable and run it to verify it fails**

Run:
```bash
chmod +x scripts/verify-signing.sh
./scripts/verify-signing.sh --entitlements
```

Expected: FAIL with `✗ com.apple.security.app-sandbox is present`.

- [ ] **Step 3: Remove App Sandbox from the direct-build entitlements**

Replace the entire contents of `src-tauri/Entitlements.plist` with:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!--
  Entitlements for the DIRECT distribution build (GitHub Releases, Homebrew),
  signed with Developer ID and notarized.

  App Sandbox is deliberately absent. It is not required for Developer ID
  distribution, and enabling it would repoint $HOME to
  ~/Library/Containers/com.connection-app.desktop/Data — making every existing
  user's ~/.connection-app projects, saved connections and history invisible,
  with no way to migrate them from inside the sandbox.

  The Mac App Store build uses Entitlements.mas.plist instead, where the
  sandbox IS mandatory. sandbox.rs detects which case applies at runtime via
  sandbox::is_sandboxed(), so one codebase serves both.
-->
<plist version="1.0">
<dict/>
</plist>
```

- [ ] **Step 4: Run the check to verify it passes**

Run:
```bash
./scripts/verify-signing.sh --entitlements
```

Expected: PASS with `✓ app-sandbox absent`.

- [ ] **Step 5: Confirm the Rust side is unaffected**

Run:
```bash
cd src-tauri && cargo test && cd ..
```

Expected: all 67 tests pass. `sandbox.rs` is runtime-gated, so removing a build-time entitlement must not change any test outcome. If a test fails here, stop — it means something depends on the entitlement in a way the design did not account for.

- [ ] **Step 6: Commit**

```bash
git add scripts/verify-signing.sh src-tauri/Entitlements.plist
git commit -m "fix(macos): drop app-sandbox from direct-build entitlements

Entitlements are inert on an adhoc-signed binary, so the declared
app-sandbox has never taken effect. Signing with a real Developer ID
would activate it for the first time and repoint \$HOME to a container,
hiding every existing user's ~/.connection-app data with no in-sandbox
migration path.

App Sandbox is not required for Developer ID distribution — only for the
App Store, which will carry its own Entitlements.mas.plist.

Adds scripts/verify-signing.sh to assert this and the other release
invariants executably rather than by memory."
```

---

### Task 2: Wire signing and notarization into the release workflow

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `scripts/verify-signing.sh --workflow` from Task 1
- Produces: macOS release artifacts that are signed, notarized and stapled; a `APPLE_API_KEY_PATH` environment value exported for the macOS matrix legs

- [ ] **Step 1: Run the workflow check to verify it fails**

Run:
```bash
./scripts/verify-signing.sh --workflow
```

Expected: FAIL — all seven `APPLE_*` variables reported missing, plus the missing key-materialization step.

- [ ] **Step 2: Add the API key materialization step**

In `.github/workflows/release.yml`, insert this step immediately **before** the existing `- name: Build Tauri app` step:

```yaml
      # The App Store Connect key is a file on disk as far as notarization is
      # concerned, but a secret as far as GitHub is concerned. Bridge the two.
      # macOS legs only — the Linux and Windows legs never notarize.
      - name: Prepare Apple API key
        if: startsWith(matrix.platform, 'macos')
        env:
          APPLE_API_KEY_P8: ${{ secrets.APPLE_API_KEY_P8 }}
        run: |
          if [ -z "$APPLE_API_KEY_P8" ]; then
            echo "ERROR: APPLE_API_KEY_P8 secret is empty — notarization would"
            echo "silently produce signed-but-unnotarized builds."
            exit 1
          fi
          mkdir -p "$HOME/private_keys"
          printf '%s' "$APPLE_API_KEY_P8" > "$HOME/private_keys/AuthKey.p8"
          chmod 600 "$HOME/private_keys/AuthKey.p8"
          echo "APPLE_API_KEY_PATH=$HOME/private_keys/AuthKey.p8" >> "$GITHUB_ENV"
```

- [ ] **Step 3: Pass the Apple credentials to tauri-action**

In the same file, extend the `env:` block of the `- name: Build Tauri app` step. It currently reads:

```yaml
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
```

Change it to:

```yaml
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
          # Apple code signing. Ignored by the Linux and Windows legs.
          APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
          # Notarization via App Store Connect API.
          APPLE_API_ISSUER: ${{ secrets.APPLE_API_ISSUER }}
          APPLE_API_KEY: ${{ secrets.APPLE_API_KEY }}
          APPLE_API_KEY_PATH: ${{ env.APPLE_API_KEY_PATH }}
```

Note: `TAURI_SIGNING_PRIVATE_KEY` is the updater's minisign key and is unrelated to Apple signing. Both are required; do not remove either.

- [ ] **Step 4: Run the workflow check to verify it passes**

Run:
```bash
./scripts/verify-signing.sh --workflow
```

Expected: PASS on all eight assertions.

- [ ] **Step 5: Confirm the workflow is still valid YAML**

Run:
```bash
python3 -c "import sys,yaml;yaml.safe_load(open('.github/workflows/release.yml'));print('release.yml parses')" \
  2>/dev/null || echo "pyyaml unavailable — check indentation manually against the diff"
```

Expected: `release.yml parses`. If PyYAML is not installed, review `git diff .github/workflows/release.yml` and confirm both new blocks sit at the same indentation as their neighbouring steps and keys.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): sign and notarize macOS builds

Passes the Developer ID certificate and App Store Connect API credentials
to tauri-action, which signs with hardened runtime, notarizes, and staples.

The .p8 key arrives as a secret but notarization wants a file path, so a
macOS-only step materializes it and exports APPLE_API_KEY_PATH. That step
fails loudly on an empty secret rather than producing signed-but-
unnotarized builds that look fine until a user downloads one."
```

---

### Task 3: Remove the `xattr` workaround from the README

**Files:**
- Modify: `README.md:43-55`

**Interfaces:**
- Consumes: nothing
- Produces: nothing consumed by later tasks

- [ ] **Step 1: Confirm the current state**

Run:
```bash
grep -n "xattr" README.md
```

Expected: exactly one match, `README.md:51`, inside the unsigned-build note. If more appear, the note has been edited since this plan was written — remove every occurrence in step 2, not just line 51.

- [ ] **Step 2: Replace the note**

In `README.md`, delete lines 43-55 — the block that begins `> **Note — unsigned build (no Apple Developer ID yet)**` and ends `> signed and notarised build.` — and put this in its place:

```markdown
> **Signed and notarised**
>
> macOS builds are signed with an Apple Developer ID and notarised by Apple,
> so they open without Gatekeeper warnings. No `xattr` workaround is needed.
```

- [ ] **Step 3: Verify no workaround text survives anywhere**

Run:
Run from the repository root:
```bash
grep -rn "xattr" README.md && echo "^^^ still referenced" || echo "no xattr references remain"
```

Expected: `no xattr references remain`. `CHANGELOG.md` currently contains no `xattr` reference; if a future entry describes the removal as history, that is fine and should be left alone.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: drop the xattr -cr workaround

macOS builds are signed and notarised from v3.7.8 onward, so Gatekeeper
no longer quarantines them."
```

---

### Task 4: Release v3.7.8 and verify against real artifacts

> **This task performs outward-facing actions — it publishes a public GitHub release. Do not run it without the repository owner's explicit go-ahead.**
>
> Everything before this point is verifiable locally. Nothing in Tasks 1-3 proves that notarization actually works, because notarization requires Apple's servers and a tagged build. This task is where the design is either confirmed or refuted.

**Files:**
- Modify: `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`

**Interfaces:**
- Consumes: `scripts/verify-signing.sh --artifacts` from Task 1
- Produces: a notarized v3.7.8 release

- [ ] **Step 1: Bump the version in all three places**

The version appears in three files and they must agree, or the updater manifest will advertise a version that does not match the bundle.

```bash
grep -n '"version"' package.json src-tauri/tauri.conf.json
grep -n '^version' src-tauri/Cargo.toml
```

Change every `3.7.7` found by those commands to `3.7.8`.

- [ ] **Step 2: Verify the three agree**

Run:
```bash
grep -h '"version"' package.json src-tauri/tauri.conf.json | grep -o '3\.7\.[0-9]*'
grep '^version' src-tauri/Cargo.toml | grep -o '3\.7\.[0-9]*'
```

Expected: `3.7.8` printed three times and nothing else.

- [ ] **Step 3: Run the full local gate**

```bash
./scripts/verify-signing.sh
npx @biomejs/biome check .
cd src-tauri && cargo test && cd ..
```

Expected: all checks pass, Biome reports no errors, 67 Rust tests pass.

- [ ] **Step 4: Commit, push, and open the pull request**

```bash
git add package.json src-tauri/Cargo.toml src-tauri/tauri.conf.json
git commit -m "chore(release): v3.7.8 — first signed and notarised build"
git push -u origin feat/macos-signing-app-store
gh pr create --title "feat(macos): sign and notarise builds (v3.7.8)" --body "$(cat <<'PRBODY'
First signed and notarised macOS release.

- Drops `app-sandbox` from the direct-build entitlements. It has never
  actually been in effect (the binary was adhoc-signed), and switching it on
  would have hidden every existing user's `~/.connection-app` data behind a
  sandbox container.
- Wires the Developer ID certificate and App Store Connect API credentials
  into the release workflow.
- Adds `scripts/verify-signing.sh`, which asserts the invariants and is the
  gate for this and future signed releases.
- Removes the `xattr -cr` workaround from the README.

Design: `docs/superpowers/specs/2026-08-06-macos-signing-and-app-store-design.md`

🤖 Generated with [Claude Code](https://claude.com/claude-code)
PRBODY
)"
```

- [ ] **Step 5: Merge, tag, and watch the release build**

After the PR is approved and merged to `main`:

```bash
git checkout main && git pull
git tag v3.7.8
git push origin v3.7.8
gh run watch
```

Expected: the four matrix legs succeed. The macOS legs take noticeably longer than before — notarization adds roughly 2-10 minutes each while Apple's service processes the submission.

- [ ] **Step 6: Verify the published artifacts**

```bash
mkdir -p /tmp/v378 && cd /tmp/v378
gh release download v3.7.8 --repo yarka-guru/connection_app --pattern "*aarch64*.dmg"
hdiutil attach ./*.dmg -mountpoint /tmp/v378/mnt
cd - >/dev/null
./scripts/verify-signing.sh --artifacts /tmp/v378/mnt/ConnectionApp.app /tmp/v378/*.dmg
hdiutil detach /tmp/v378/mnt
```

Expected: all eight artifact assertions pass. If `source=Notarized Developer ID` is missing while the signature checks pass, the build was signed but not notarized — inspect the `Prepare Apple API key` step's log and confirm `APPLE_API_KEY_PATH` reached `tauri-action`.

- [ ] **Step 7: Test on a machine that has never seen the app**

Install the downloaded `.dmg` on a different Mac — or a fresh VM — and launch it.

Expected: the app opens with no Gatekeeper dialog whatsoever.

This step cannot be skipped or substituted with a local run. The build machine's Gatekeeper caches a verdict for apps it has already seen and will happily launch a bundle that a clean machine would reject.

- [ ] **Step 8: Verify existing-user data survived**

On a Mac that already had v3.7.7 installed with real projects configured, upgrade to v3.7.8 and open the app.

Expected: all projects, saved connections and history are present, and **no folder picker appears** asking for `~/.aws`.

Then confirm no container was created:

```bash
ls -d ~/Library/Containers/com.connection-app.desktop 2>/dev/null \
  && echo "REGRESSION: a sandbox container exists" \
  || echo "correct: no sandbox container"
```

Expected: `correct: no sandbox container`.

- [ ] **Step 9: Verify auto-update still works end to end**

From a machine running v3.7.7, trigger the in-app updater and let it install v3.7.8.

Expected: the update applies and the updated app launches without a Gatekeeper prompt.

This is the check most likely to be skipped and most expensive to get wrong. The updater replaces the whole `.app` bundle, and the notarization ticket is stapled *inside* that bundle — so if the updater's `.app.tar.gz` were built before stapling, auto-updated users would end up with a quarantined app while fresh DMG installs looked perfect.

- [ ] **Step 10: Confirm the Homebrew cask serves the signed build**

```bash
brew update && brew upgrade --cask connection-app
open -a ConnectionApp
```

Expected: upgrade succeeds and the app launches cleanly. If `update-homebrew.yml` carried any `xattr` postinstall stanza, remove it in a follow-up commit — verify with:

```bash
gh api repos/yarka-guru/homebrew-tap/contents/Casks/connection-app.rb \
  --jq '.content' | base64 -d | grep -n "xattr" \
  && echo "^^^ cask still strips quarantine; remove it" \
  || echo "cask is clean"
```

---

## Out of scope for this plan

Phase 2 — the Mac App Store build — gets its own plan once this one has shipped and been verified. It is blocked on material that does not yet exist:

- Apple Distribution certificate
- Mac Installer Distribution certificate
- Mac App Store provisioning profile for `com.connection-app.desktop`
- Agreements, Tax and Banking completed in App Store Connect (owner's task; blocks any paid sale)
- A hosted privacy policy URL

Phase 2 work items, recorded so they are not lost: split `tauri-plugin-updater` out of the `gui` cargo feature and move `updater:default` into its own capability file; add `Entitlements.mas.plist`; add `tauri.mas.conf.json` carrying `LSApplicationCategoryType` and `ITSAppUsesNonExemptEncryption`; write `scripts/build-mas.sh`; build demo mode behind `src/lib/ipc.js`; prepare listing assets.

## Self-review notes

- **Spec coverage.** This plan covers the spec's "Phase 1" and "Verification" sections in full. The spec's "Phase 2", "Demo mode" and "Listing and pricing" sections are deliberately deferred, and the deferral is recorded above with its blockers.
- **Task 4 ordering.** Steps 7, 8 and 9 each require a machine state the build host does not have (never-seen-the-app, previously-on-3.7.7, running-3.7.7). They can be performed in any order relative to each other, but all three must pass before the release is considered good.
- **Rollback.** If Step 6 or 7 fails, delete the release and tag (`gh release delete v3.7.8 --yes; git push --delete origin v3.7.8`) before investigating. A published-but-broken signed build is worse than an unsigned one, because Homebrew users will have already upgraded.
