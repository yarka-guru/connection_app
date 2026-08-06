# macOS Code Signing & Mac App Store Distribution

**Date:** 2026-08-06
**Status:** Approved design
**Baseline version:** 3.7.7

## Problem

ConnectionApp ships unsigned. `README.md` instructs users to run `xattr -cr
/Applications/ConnectionApp.app` because Gatekeeper refuses to launch the bundle.
An Apple Developer Program membership is now active (Team ID `XG4FR287W6`), so
the app can be signed, notarized, and additionally sold on the Mac App Store.

## Goals

1. Notarized builds on GitHub Releases and Homebrew, free, with the `xattr`
   workaround removed.
2. A paid Mac App Store build at $9.99.

Both, with phase 1 shippable independently of phase 2.

## Critical finding: the app has never been sandboxed

The shipped binary is `Signature=adhoc, linker-signed` with **zero entitlements**
and no container. `Entitlements.plist` declares `com.apple.security.app-sandbox`,
but entitlements are inert without a real signature. Consequently
`sandbox::is_sandboxed()` returns false, the security-scoped bookmark path in
`sandbox.rs` never runs, and the app reads `~/.aws` directly.

User data currently lives at real `$HOME`:

```
~/.connection-app/  projects.json, preferences.json, history.jsonl
```

Signing with the existing `Entitlements.plist` unchanged would activate the
sandbox for the first time. macOS would repoint `$HOME` to
`~/Library/Containers/com.connection-app.desktop/Data`, and every existing
Homebrew user would open the first "properly signed" release to find no
projects, no saved connections, no history, and a folder picker demanding
`~/.aws`. The data is not deleted, only invisible — which reads as data loss.

Migrating it from inside the sandbox is impossible: reading `~/.connection-app`
is precisely what the sandbox forbids without a user grant.

## Decision: two entitlement profiles

App Sandbox is **not required for Developer ID distribution**. It is mandatory
only for the App Store. The two builds therefore diverge only in entitlements
and signing material:

| | Direct build | App Store build |
|---|---|---|
| Entitlements | `Entitlements.plist`, `app-sandbox` **removed** | `Entitlements.mas.plist`, sandbox **on** |
| Certificate | Developer ID Application | Apple Distribution + provisioning profile |
| Runtime | Hardened runtime, notarized | MAS (no hardened runtime) |
| `$HOME` | real, unchanged | container |
| Updater | active | compiled out |
| Existing users | unaffected | n/a, fresh install |

`lib.rs:39` already gates on **runtime** `sandbox::is_sandboxed()`, not a
compile-time flag. One codebase serves both: the direct build takes the direct
path, the MAS build takes the bookmark path. `sandbox.rs` needs no changes.

## Phase 1 — Developer ID + notarization

### Credentials (provisioned 2026-08-06)

| Item | Value |
|---|---|
| Team ID | `XG4FR287W6` |
| Signing identity | `Developer ID Application: Iaroslav Pyrogov (XG4FR287W6)` |
| Certificate expiry | 2031-08-07, Developer ID G2 branch |
| API key | `GBQH68KN3W`, role App Manager |
| API issuer | `799a6169-6e5d-4fc9-bac6-38993ccf145e` |

Seven repository secrets are set: `APPLE_CERTIFICATE`,
`APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_TEAM_ID`,
`APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_P8`.

Private material is archived at
`~/Documents/09 - Security & Recovery/Apple Developer ID/`. The `.p8`, the
RSA private key, and the `.p12` password cannot be re-obtained from Apple.

The App Store Connect API key route was chosen over an Apple ID app-specific
password because phase 2 needs an API key for uploads regardless — one
credential serves both, it is revocable, and it does not break when the Apple ID
password or 2FA changes.

### Changes

1. **`src-tauri/Entitlements.plist`** — remove `com.apple.security.app-sandbox`
   and the four sandbox-only keys beneath it. For a hardened-runtime Developer ID
   build they are inert, and keeping `app-sandbox` is exactly what would strand
   existing users in a container. The result is a near-empty dict, correct for a
   static Rust binary with no JIT and no plugin loading.

2. **`.github/workflows/release.yml`** — add the Apple variables to the `env:`
   block of the existing "Build Tauri app" step. `tauri-action` creates a
   temporary keychain, signs with hardened runtime, notarizes, and staples.
   Linux and Windows legs ignore the variables. A preceding step writes
   `APPLE_API_KEY_P8` to a file and exports `APPLE_API_KEY_PATH`.

3. **`README.md`** — delete the unsigned-build note and the `xattr -cr`
   instruction at lines 43-52.

Notarization adds roughly 2-10 minutes per macOS matrix leg.

## Phase 2 — Mac App Store

### 1. Split the updater out of the `gui` feature

`Cargo.toml:78` currently bundles `tauri-plugin-updater` into `gui`. Apple scans
binaries, so the plugin must be absent, not merely hidden:

```toml
gui     = ["tauri", "tauri-build", ... "custom-protocol"]   # updater removed
updater = ["tauri-plugin-updater"]
default = ["gui", "updater"]
```

`lib.rs:28` and the three call sites in `commands/system.rs` get
`#[cfg(feature = "updater")]`.

`capabilities/default.json` lists `updater:default`, and Tauri scans that
directory at build time regardless of cargo features. A listed permission for an
unregistered plugin is a startup error. The updater permission therefore moves
to its own `capabilities/updater.json`, which the MAS build script deletes
before building — explicit, and it fails loudly if forgotten.

### 2. Certificates and identifiers

- App ID `com.connection-app.desktop`, the same bundle ID as the direct build.
  Having both builds installed simultaneously is undefined behaviour; that is
  acceptable since users choose one.
- Apple Distribution certificate — signs the `.app`
- Mac Installer Distribution certificate — signs the `.pkg`
- Mac App Store provisioning profile, embedded at
  `Contents/embedded.provisionprofile`

### 3. `Entitlements.mas.plist`

The current `Entitlements.plist` kept verbatim: `app-sandbox`,
`network.client`, `network.server` (the tunnel binds a local port),
`files.user-selected.read-write`, `files.bookmarks.app-scope`.

### 4. Info.plist keys

Added through a `tauri.mas.conf.json` overlay merged with `tauri build --config`:

- `LSApplicationCategoryType` = `public.app-category.developer-tools` —
  submission fails without it
- `ITSAppUsesNonExemptEncryption` = `false` — the app uses only standard TLS;
  declaring it once avoids the export-compliance questionnaire every submission
- `NSHumanReadableCopyright`

### 5. `scripts/build-mas.sh`

Tauri has no `mas` bundle target, so packaging is manual: build → copy the
provisioning profile into the bundle → sign nested content, then the app, with
the MAS entitlements → `productbuild --component` signed with the installer
certificate → `altool --validate-app` → `--upload-app`.

`--deep` must not be used; Apple deprecated it and it silently mis-signs nested
code.

**The first submission runs locally, not in CI.** Failure modes here surface as
opaque ITMS errors, and debugging them through Actions logs is expensive. The
script moves to CI once a build has been accepted once.

## Demo mode

An App Store reviewer on a clean Mac has no `~/.aws` directory.
`grant_aws_dir_access()` hard-gates the app on picking that folder and validates
the pick is named `.aws` or contains a `config` file, so a reviewer cannot get
past the first screen — a Guideline 2.1 rejection.

### Design

**Frontend-only.** A `demoMode` flag intercepts backend calls and returns canned
responses without reaching Rust. The reason is not economy of effort: with this
structure demo mode is *incapable* of opening a socket or contacting AWS. Were
the switch in Rust, that would need to be proven by reading code.

- New `src/lib/ipc.js` exporting `call()`, delegating either to `invoke` or to
  the demo response table. Components move from direct `invoke` to `call` —
  a mechanical change across seven files that also makes the frontend testable
  without Tauri.
- Activation: a button on the empty state, reachable **before** the `~/.aws`
  gate. Without that ordering it does not solve the problem it exists for.
- Content: two or three fictional projects with environments, RDS endpoints on
  the `.invalid` TLD (RFC 2606, guaranteed non-resolvable), clearly marked fake
  credentials. Connecting plays a sequence of status transitions with delays and
  yields a fictional local port.
- Marking: a persistent banner and a distinct accent colour while active,
  at WCAG AA contrast on the dark background.

### Constraint

Demo mode is an ordinary product feature — visible to everyone, documented,
always available. No reviewer detection, no hidden flags. Guideline 2.3.1
forbids hidden functionality, so a reviewer-only backdoor would be both a
rejection risk and dishonest. As an onboarding preview it carries its own value:
a user sees what the app does before configuring AWS.

## Listing and pricing

$9.99 base price; Apple generates regional prices, editable. Small Business
Program (15% instead of 30%) is a separate application, available after the Paid
Apps agreement.

| Item | State |
|---|---|
| Category | Developer Tools |
| Icon 1024×1024 | present in `icons/` |
| Privacy policy URL | **blocked** — `PRIVACY.md` is in-repo, Apple requires a URL; needs GitHub Pages |
| Support URL | Issues page |
| Screenshots | **blocked** — macOS requires 1280×800 / 1440×900 / 2560×1600 / 2880×1800; the app window is 650×700, so they must be composed on a backdrop |
| Privacy nutrition labels | "Data Not Collected", matching the empty `NSPrivacyCollectedDataTypes` |
| Export compliance | covered by `ITSAppUsesNonExemptEncryption` |
| Review notes | explain demo mode and the `network.server` entitlement |

Selling at $9.99 what is free on GitHub under MIT is permitted; the author holds
the copyright.

Third-party licenses are all MIT or Apache-2.0 with no GPL or LGPL, so there is
no App Store licensing conflict.

## Verification

Phase 1 is not done until all of the following pass:

```bash
codesign -dv --verbose=4 ConnectionApp.app   # Authority=Developer ID Application: … (XG4FR287W6)
                                             # flags=0x10000(runtime)
spctl -a -vvv -t install ConnectionApp.dmg   # accepted, source=Notarized Developer ID
stapler validate ConnectionApp.app           # and separately the .dmg
```

Three checks that are easy to miss:

1. **Clean-machine test.** The build machine's Gatekeeper remembers the app and
   will admit it even when broken. A Mac that has never seen it is required.
2. **Existing-user data regression.** After updating, `~/.connection-app` must
   remain visible. This is the entire point of dropping the sandbox from the
   direct build.
3. **Auto-update after notarization.** The updater replaces the whole bundle and
   the staple ticket lives inside it, so notarization must survive the update.
   Only a live v3.7.8 → v3.7.9 run proves this.

Phase 2 additionally: `codesign -d --entitlements -` shows the sandbox,
`embedded.provisionprofile` is present, `altool --validate-app` passes, and the
updater is genuinely absent from the binary rather than hidden in the UI.
TestFlight for macOS is available and worth a pass before submission.

## Out of scope

- Windows and Linux signing
- StoreKit, in-app purchase, subscriptions — the app is paid-upfront, which
  requires no code
- Migrating existing users into a sandbox container

## Open items owned by the user

- Agreements, Tax and Banking in App Store Connect — blocks any paid sale.
  Account country is VN, so the tax forms are Vietnamese.
- Small Business Program application
- Hosted privacy policy URL
