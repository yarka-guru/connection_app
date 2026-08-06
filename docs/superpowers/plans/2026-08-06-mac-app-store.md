# Mac App Store (Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship ConnectionApp on the Mac App Store as a paid app at $9.99, alongside the existing free notarized build.

**Architecture:** One codebase, two build variants. The direct build stays exactly as it is today. The MAS variant is produced by a config overlay plus a cargo feature that compiles the updater out, signed with Apple Distribution against a sandboxed entitlements file and packaged as a `.pkg`. A demo mode makes the app reviewable by someone with no AWS account.

**Tech Stack:** Tauri 2, cargo features, `productbuild`, `notarytool`/`altool`, Svelte 5, App Store Connect.

**Source spec:** `docs/superpowers/specs/2026-08-06-macos-signing-and-app-store-design.md`

## Global Constraints

- Team ID `XG4FR287W6`. Bundle identifier `com.connection-app.desktop`, unchanged.
- The direct build must keep working untouched. `Entitlements.plist` stays an empty dict; sandbox lives only in `Entitlements.mas.plist`.
- `scripts/verify-signing.sh` is the release gate. Every new invariant gets an assertion there, and every assertion must exercise the real tool, never a proxy. Both Phase 1 release failures got through checks that inspected stand-ins.
- No JS test runner exists. Rust tests: `cargo test` in `src-tauri/` (67 tests). Lint: `npx @biomejs/biome check .`.
- Demo mode is a normal product feature — visible to everyone, documented, always reachable. No reviewer detection, no hidden flags (App Store Guideline 2.3.1 forbids hidden functionality).
- Current version 3.7.8.

## Blockers as of 2026-08-06

Verified in App Store Connect:

| Item | State | Owner |
|---|---|---|
| Free Apps Agreement | **Active** (to 2027-07-06) | — |
| Paid Apps Agreement | **New** — unsigned | user |
| Legal entity information | must be updated *before* signing Paid Apps | user |
| DSA trader status (EU) | not declared; blocks EU availability | user |
| Tax and banking details | not entered | user |
| Apple Distribution cert | not created | can be done now |
| Mac Installer Distribution cert | not created | can be done now |
| App ID + provisioning profile | not created | can be done now |

Tasks 1–6 are unblocked. Task 7 needs the certificates from Task 5. Task 9 requires the user's agreements before a **paid** release; a **free** release is possible today.

**Privacy note for the user:** declaring trader status under the EU DSA publishes name and address on the App Store listing. The account address is currently a residential one in Vietnam. Avoiding that means either not distributing in the EU or registering a legal entity.

---

## File Structure

| File | Responsibility |
|---|---|
| `scripts/generate-icons.js` | **modify** — emit a 1024×1024 PNG for App Store Connect |
| `src-tauri/Cargo.toml` | **modify** — split `updater` out of the `gui` feature |
| `src-tauri/src/lib.rs` | **modify** — conditionally register the updater plugin and its two commands |
| `src-tauri/src/commands/system.rs` | **modify** — gate the two updater commands; add `is_updater_enabled` |
| `src-tauri/capabilities/updater.json` | **create** — updater permission, removed by the MAS build |
| `src-tauri/capabilities/default.json` | **modify** — drop `updater:default` |
| `src-tauri/Entitlements.mas.plist` | **create** — sandboxed entitlements for the MAS build |
| `src-tauri/tauri.mas.conf.json` | **create** — config overlay: category, copyright, export compliance |
| `src/lib/demo.js` | **create** — demo data and a stand-in for `invoke` |
| `src/App.svelte` | **modify** — demo entry point, hide update UI when the updater is absent |
| `scripts/build-mas.sh` | **create** — build, re-sign, package, validate, upload |
| `scripts/verify-signing.sh` | **modify** — assertions for the MAS artifacts |

---

### Task 1: A 1024×1024 icon

App Store Connect rejects submissions without one, and the largest icon in the repo today is 256×256. The generator is programmatic, so this is a size argument rather than design work.

**Files:**
- Modify: `scripts/generate-icons.js`

**Interfaces:**
- Produces: `src-tauri/icons/icon-1024.png`, uploaded manually to App Store Connect

- [ ] **Step 1: Confirm the gap**

```bash
sips -g pixelWidth -g pixelHeight src-tauri/icons/*.png src-tauri/icons/icon.icns 2>/dev/null | grep -B1 pixelWidth
```

Expected: nothing larger than 256. If a 1024 already exists, skip this task.

- [ ] **Step 2: Install the generator's dependency**

`canvas` is imported by the script but is not in `package.json`.

```bash
npm install --save-dev canvas
```

- [ ] **Step 3: Add 1024 to the generated sizes**

Open `scripts/generate-icons.js` and find the loop or list that calls `createIcon(size)`. Add `1024` to the sizes it generates, writing to `icon-1024.png`. The existing `createIcon(size)` already scales every element from `size`, so no drawing code changes.

- [ ] **Step 4: Generate and verify**

```bash
npm run generate-icons
sips -g pixelWidth -g pixelHeight src-tauri/icons/icon-1024.png
```

Expected: `pixelWidth: 1024`, `pixelHeight: 1024`.

- [ ] **Step 5: Commit**

```bash
git add scripts/generate-icons.js package.json package-lock.json src-tauri/icons/
git commit -m "feat(icons): generate a 1024x1024 icon for App Store Connect"
```

---

### Task 2: Compile the updater out of the MAS build

App Store Guideline 2.4.5(iv) forbids an app downloading and installing its own code. Hiding the button is not enough — Apple scans binaries, so the plugin must be absent.

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/src/commands/system.rs`, `src-tauri/capabilities/default.json`
- Create: `src-tauri/capabilities/updater.json`

**Interfaces:**
- Produces: cargo feature `updater` (in `default`); Tauri command `is_updater_enabled() -> bool`, always compiled, returning `cfg!(feature = "updater")`

- [ ] **Step 1: Write the failing check**

Add this to `scripts/verify-signing.sh`, before the final `case` block:

```bash
# The MAS build must not contain the updater. Apple scans binaries, so a
# hidden button is not enough — the plugin has to be absent.
check_mas_binary() {
  local app="$1"
  echo "MAS binary:"

  if [ ! -d "$app" ]; then fail "no such app bundle: $app"; return; fi

  local bin="$app/Contents/MacOS/connection-app"
  if [ ! -f "$bin" ]; then fail "no executable at $bin"; return; fi

  if strings "$bin" | grep -q "tauri_plugin_updater"; then
    fail "the updater plugin is compiled into this binary"
    fail "  build the MAS variant with --no-default-features --features gui"
  else
    pass "updater plugin absent from the binary"
  fi

  if [ -f "$app/Contents/embedded.provisionprofile" ]; then
    pass "provisioning profile embedded"
  else
    fail "no Contents/embedded.provisionprofile — App Store upload will be rejected"
  fi

  if codesign -d --entitlements - "$app" 2>/dev/null | grep -q "app-sandbox"; then
    pass "sandbox enabled"
  else
    fail "app-sandbox missing — mandatory for the Mac App Store"
  fi
}
```

Add a `--mas-binary` mode to the `case` block:

```bash
  --mas-binary)
    if [ $# -ne 2 ]; then
      echo "usage: $0 --mas-binary <path/to/App.app>" >&2
      exit 2
    fi
    check_mas_binary "$2"
    ;;
```

- [ ] **Step 2: Split the cargo feature**

In `src-tauri/Cargo.toml`, replace the `[features]` block:

```toml
[features]
default = ["gui", "updater"]
gui = ["tauri", "tauri-build", "tauri-plugin-store", "tauri-plugin-opener", "tauri-plugin-dialog", "tauri-plugin-notification", "custom-protocol"]
updater = ["tauri-plugin-updater"]
custom-protocol = ["tauri/custom-protocol"]
```

Note `tauri-plugin-updater` is removed from `gui` and `updater` is added to `default`, so the ordinary build is unchanged.

- [ ] **Step 3: Gate the plugin registration**

`src-tauri/src/lib.rs` line 28 registers the plugin inside an unbroken method chain, and `#[cfg]` cannot be applied to a single link in a chain. Break the chain into a rebindable `builder` variable.

Delete line 28 (`.plugin(tauri_plugin_updater::Builder::default().build())`) and restructure the head of the chain — currently `tauri::Builder::default()` through `.plugin(tauri_plugin_notification::init())`, followed by `.setup(|app| {` — into:

```rust
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());

    #[cfg(feature = "updater")]
    let builder = builder.plugin(tauri_plugin_updater::Builder::default().build());

    builder
        .setup(|app| {
```

The rest of the chain continues unchanged from `.setup(|app| {`.

- [ ] **Step 4: Gate the two commands**

In `src-tauri/src/commands/system.rs`, add `#[cfg(feature = "updater")]` directly above **both** `#[tauri::command]` attributes — at line 93 (`check_for_updates`) and line 293 (`install_update`):

```rust
#[cfg(feature = "updater")]
#[tauri::command]
pub async fn check_for_updates(app_handle: AppHandle) -> Result<UpdateInfo, AppError> {
```

```rust
#[cfg(feature = "updater")]
#[tauri::command]
pub async fn install_update(app_handle: AppHandle) -> Result<(), AppError> {
```

Then add a new command at the end of the file, **not** gated — the frontend must be able to ask in either build:

```rust
/// Whether this build can update itself. False in Mac App Store builds, where
/// Guideline 2.4.5(iv) forbids an app installing its own code and the plugin is
/// compiled out entirely.
#[tauri::command]
pub fn is_updater_enabled() -> bool {
    cfg!(feature = "updater")
}
```

- [ ] **Step 5: Gate the command registration**

In `src-tauri/src/lib.rs`, lines 108-109 register the two commands inside `tauri::generate_handler![...]`. That macro cannot take `#[cfg]` on individual entries reliably, so register the whole handler conditionally. Replace the single `.invoke_handler(tauri::generate_handler![ ... ])` call with two variants selected by feature, keeping every other command identical in both lists:

```rust
    #[cfg(feature = "updater")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        // ... every existing command ...
        commands::system::check_for_updates,
        commands::system::install_update,
        commands::system::is_updater_enabled,
    ]);

    #[cfg(not(feature = "updater"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        // ... every existing command, minus the two above ...
        commands::system::is_updater_enabled,
    ]);
```

Copy the existing command list verbatim into both arms. Duplication is deliberate: `generate_handler!` builds a match on command names at compile time and does not accept conditional entries.

- [ ] **Step 6: Move the updater capability to its own file**

Create `src-tauri/capabilities/updater.json`:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "updater",
  "description": "Self-update permission. Deleted by scripts/build-mas.sh — the Mac App Store build has no updater plugin, and a permission naming an unregistered plugin is a startup error.",
  "windows": ["main"],
  "permissions": ["updater:default"]
}
```

Then remove the `"updater:default",` line from `src-tauri/capabilities/default.json`.

- [ ] **Step 7: Verify both builds compile**

```bash
cd src-tauri
cargo check --features gui,updater
cargo check --no-default-features --features gui
cargo test
cd ..
```

Expected: both `cargo check` runs succeed, 67 tests pass. The second is the MAS configuration — if it fails, something still references the updater unconditionally.

- [ ] **Step 8: Hide the update UI when there is no updater**

In `src/App.svelte`, add a state variable near the other top-level declarations:

```javascript
let updaterEnabled = $state(true)
```

In the startup sequence — the same place that awaits `invoke('get_current_version')` around line 272 — also fetch the flag:

```javascript
updaterEnabled = await invoke('is_updater_enabled')
```

Then guard both call sites. Line 399 (`check_for_updates`) becomes:

```javascript
if (!updaterEnabled) return
updateInfo = await invoke('check_for_updates')
```

and line 677 (`install_update`) becomes:

```javascript
if (!updaterEnabled) return
await invoke('install_update')
```

Finally, wrap the "Check for Updates" button in the footer with `{#if updaterEnabled}` so App Store users never see a control that cannot work.

- [ ] **Step 9: Verify the direct build is unchanged**

```bash
npm run build:vite
npx @biomejs/biome check .
cd src-tauri && cargo test && cd ..
```

Expected: build succeeds, lint clean, 67 tests pass. Launch `npm run dev:gui` and confirm the "Check for Updates" button is still present and still works — the default feature set includes `updater`, so nothing about the normal build changes.

- [ ] **Step 10: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/lib.rs \
        src-tauri/src/commands/system.rs src-tauri/capabilities/ \
        src/App.svelte scripts/verify-signing.sh
git commit -m "feat(mas): make the updater an optional cargo feature

App Store Guideline 2.4.5(iv) forbids an app installing its own code.
Apple scans binaries, so hiding the button is insufficient — the plugin
must be absent. The updater moves out of the gui feature into its own,
still on by default, and its capability moves to a file the MAS build
deletes. A new is_updater_enabled command lets the frontend hide the UI."
```

---

### Task 3: MAS entitlements and config overlay

**Files:**
- Create: `src-tauri/Entitlements.mas.plist`, `src-tauri/tauri.mas.conf.json`

**Interfaces:**
- Consumes: nothing
- Produces: files referenced by `scripts/build-mas.sh` in Task 7

- [ ] **Step 1: Create the sandboxed entitlements**

Create `src-tauri/Entitlements.mas.plist`. **No XML comments** — a `--` sequence inside one made v3.7.8 unsignable, and `plutil -lint` does not catch it:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.app-sandbox</key>
    <true/>
    <key>com.apple.security.network.client</key>
    <true/>
    <key>com.apple.security.network.server</key>
    <true/>
    <key>com.apple.security.files.user-selected.read-write</key>
    <true/>
    <key>com.apple.security.files.bookmarks.app-scope</key>
    <true/>
</dict>
</plist>
```

`network.server` is required because the tunnel binds a local port. Mention this in the App Store review notes.

- [ ] **Step 2: Prove codesign accepts it**

```bash
cp /bin/echo /tmp/mas-ent-probe
codesign --force --entitlements src-tauri/Entitlements.mas.plist -s - /tmp/mas-ent-probe
rm -f /tmp/mas-ent-probe
```

Expected: no `Failed to parse entitlements`. This is the check that would have caught the v3.7.8 failure.

- [ ] **Step 3: Create the config overlay**

Create `src-tauri/tauri.mas.conf.json`:

```json
{
  "bundle": {
    "category": "DeveloperTool",
    "copyright": "Copyright © 2023-2026 Iaroslav Pyrogov. All rights reserved.",
    "macOS": {
      "entitlements": "./Entitlements.mas.plist"
    }
  }
}
```

Tauri maps `category` to `LSApplicationCategoryType`; submissions fail without it.

- [ ] **Step 4: Add the export-compliance key**

Tauri merges a top-level `src-tauri/Info.plist` into the generated one. Create it:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>ITSAppUsesNonExemptEncryption</key>
    <false/>
</dict>
</plist>
```

The app uses only standard TLS. Declaring this once avoids the export-compliance questionnaire on every submission.

- [ ] **Step 5: Verify the merged config parses**

```bash
python3 -c "import json;json.load(open('src-tauri/tauri.mas.conf.json'));print('overlay ok')"
plutil -lint src-tauri/Info.plist src-tauri/Entitlements.mas.plist
```

Expected: `overlay ok` and `OK` for both plists.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Entitlements.mas.plist src-tauri/tauri.mas.conf.json src-tauri/Info.plist
git commit -m "feat(mas): add sandboxed entitlements and App Store config overlay"
```

---

### Task 4: Demo mode

An App Store reviewer on a clean Mac has no `~/.aws`. `grant_aws_dir_access()` gates the app on picking that folder and validates the choice is named `.aws` or contains a `config` file, so a reviewer cannot get past the first screen — a Guideline 2.1 rejection.

`invoke` is assigned once in `src/App.svelte` (declared line 62, assigned line 143) and passed to `Settings.svelte` as a prop. Demo mode therefore swaps a single reference; the spec's `src/lib/ipc.js` refactor is unnecessary.

**Files:**
- Create: `src/lib/demo.js`
- Modify: `src/App.svelte`

**Interfaces:**
- Consumes: nothing
- Produces: `createDemoInvoke()` returning an async `(cmd, args) => result` with the same shape as Tauri's `invoke`

- [ ] **Step 1: Write the demo backend**

Create `src/lib/demo.js`:

```javascript
/**
 * Demo mode: a stand-in for Tauri's `invoke` that returns fabricated data.
 *
 * This is a real product feature — a preview for anyone who has not configured
 * AWS yet — not a reviewer-only path. App Store Guideline 2.3.1 forbids hidden
 * functionality, and a mode that behaves differently for Apple would be
 * dishonest besides.
 *
 * It lives entirely in the frontend so it is structurally incapable of opening
 * a socket or reaching AWS. Hostnames use the .invalid TLD (RFC 2606), which is
 * guaranteed never to resolve.
 */

const DEMO_PROJECTS = [
  { name: 'Acme Analytics', region: 'eu-central-1', rdsType: 'cluster' },
  { name: 'Northwind Billing', region: 'us-east-1', rdsType: 'instance' },
]

const DEMO_PROFILES = ['acme-dev', 'acme-staging', 'acme-prod']

const DEMO_CONNECTION = {
  id: 'demo-1',
  project: 'Acme Analytics',
  profile: 'acme-dev',
  host: '127.0.0.1',
  port: 54320,
  endpoint: 'acme-analytics-dev.cluster.rds.example.invalid',
  username: 'demo_readonly',
  password: 'not-a-real-password',
  database: 'analytics',
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

export function createDemoInvoke(onStatus) {
  const connections = []

  return async function demoInvoke(cmd, args) {
    switch (cmd) {
      case 'is_updater_enabled':
        return false
      case 'get_sandbox_status':
        return { hasAwsAccess: true }
      case 'get_current_version':
        return 'demo'
      case 'check_migration_available':
        return false
      case 'list_projects':
      case 'list_project_configs':
        return DEMO_PROJECTS
      case 'list_profiles':
        return DEMO_PROFILES
      case 'load_saved_connections':
        return []
      case 'get_active_connections_list':
        return connections
      case 'connect': {
        for (const s of [
          'Checking SSO session…',
          'Fetching credentials from Secrets Manager…',
          'Locating bastion instance…',
          'Resolving RDS endpoint…',
          'Opening tunnel…',
        ]) {
          onStatus?.(s)
          await sleep(600)
        }
        onStatus?.('Ready')
        connections.push(DEMO_CONNECTION)
        return DEMO_CONNECTION
      }
      case 'disconnect':
      case 'disconnect_all':
        connections.length = 0
        return null
      default:
        // Everything else is a no-op in demo mode: nothing is persisted.
        return null
    }
  }
}
```

- [ ] **Step 2: Verify it lints**

```bash
npx @biomejs/biome check src/lib/demo.js
```

Expected: no errors.

- [ ] **Step 3: Wire the entry point**

In `src/App.svelte`, import at the top of the script block:

```javascript
import { createDemoInvoke } from './lib/demo.js'
```

Add state next to the other declarations:

```javascript
let demoMode = $state(false)
```

Add a function that switches the app into demo mode by replacing the single `invoke` reference. The status string is `statusMessage` (line 25):

```javascript
async function enterDemoMode() {
  demoMode = true
  invoke = createDemoInvoke((s) => { statusMessage = s })
  await setupListenersAndLoad()
}
```

**Call `setupListenersAndLoad()`, not `initApp()`.** `initApp()` re-imports the Tauri API and reassigns `invoke = core.invoke` at line 143, which would overwrite the demo implementation the instant it was installed. `setupListenersAndLoad()` is the part of startup that runs *after* that assignment.

Double registration is not a concern here: the demo button only appears on the sandbox gate screen, and reaching that screen means `initApp()` already returned early — at the `sandboxStatus.isSandboxed && !sandboxStatus.hasAwsAccess` branch around line 195 — without ever calling `setupListenersAndLoad()`.

`invoke` is a plain `let` (line 62), deliberately not `$state`; reassigning it is the whole mechanism. `Settings.svelte` receives it as a prop, so it picks up the demo implementation with no change of its own.

- [ ] **Step 4: Add the button to the empty state**

The button must be reachable **before** the `~/.aws` gate, or it does not solve the problem it exists for. Find the sandbox setup screen — the branch rendered when `hasAwsAccess` is false — and add below the existing "grant access" control:

```svelte
<button class="demo-button" onclick={enterDemoMode}>
  Explore with sample data
</button>
<p class="demo-hint">
  See how ConnectionApp works before configuring AWS. Nothing connects to a real
  service in this mode.
</p>
```

- [ ] **Step 5: Add the persistent banner**

While `demoMode` is true it must be impossible to mistake the app for connected. Near the top of the main layout:

```svelte
{#if demoMode}
  <div class="demo-banner" role="status">
    <strong>Demo mode</strong> — sample data only, nothing is connected.
    <button onclick={() => location.reload()}>Exit demo</button>
  </div>
{/if}
```

Style it with a distinct accent. Per the project's contrast notes, non-interactive text on the dark background must be at least `#9e9ea7` to hold WCAG AA.

- [ ] **Step 6: Verify by hand**

```bash
npm run dev:gui
```

Check all of the following:
- The demo button is visible on first run before any AWS folder is chosen
- Clicking it shows sample projects and profiles
- Connecting plays the status sequence and shows a fake connection on port 54320
- The banner stays visible the whole time
- "Exit demo" returns to the real empty state
- No real network request is made (nothing in demo mode reaches Rust)

- [ ] **Step 7: Commit**

```bash
git add src/lib/demo.js src/App.svelte
git commit -m "feat: add demo mode with sample data

An App Store reviewer has no ~/.aws, and the sandbox gate rejects any
folder that is not one — so the app could not be evaluated at all
(Guideline 2.1). Demo mode is reachable before that gate.

It lives entirely in the frontend, replacing the single invoke reference,
so it cannot open a socket or reach AWS by construction. Hostnames use
the .invalid TLD. It is an ordinary feature, visible to everyone: a
reviewer-only path would violate Guideline 2.3.1 and be dishonest."
```

---

### Task 5: Certificates, App ID, provisioning profile

Done through developer.apple.com. No agreement is required for any of it.

**Files:** none — the outputs are credentials

**Interfaces:**
- Produces: `Apple Distribution` and `Mac Installer Distribution` certificates in the login keychain; `ConnectionApp_MAS.provisionprofile` archived alongside the Developer ID material

- [ ] **Step 1: Generate a CSR**

```bash
cd ~/Documents/09\ -\ Security\ \&\ Recovery/Apple\ Developer\ ID
umask 077
openssl req -new -newkey rsa:2048 -nodes \
  -keyout mas-distribution.key \
  -out mas-distribution.certSigningRequest \
  -subj "/CN=Iaroslav Pyrogov/emailAddress=go.go.jar@gmail.com/C=VN"
```

- [ ] **Step 2: Create both certificates**

At `developer.apple.com/account/resources/certificates/add`, create in turn:
- **Apple Distribution** — signs the `.app`
- **Mac Installer Distribution** — signs the `.pkg`

Upload the same CSR for both. Download each `.cer` and double-click to import into the login keychain.

- [ ] **Step 3: Register the App ID**

At `developer.apple.com/account/resources/identifiers/add`, register an App ID of type **App**, description `ConnectionApp`, explicit bundle ID `com.connection-app.desktop`. Enable **App Sandbox** if offered as a capability.

- [ ] **Step 4: Create the provisioning profile**

At `developer.apple.com/account/resources/profiles/add`, choose **Mac App Store** distribution, select the App ID from step 3 and the Apple Distribution certificate. Download as `ConnectionApp_MAS.provisionprofile` and save it next to the other credentials.

- [ ] **Step 5: Verify the identities exist**

```bash
security find-identity -v -p codesigning | grep -E "Apple Distribution|Mac Developer"
security find-identity -v | grep "Mac Installer Distribution"
```

Expected: both appear, each ending in `(XG4FR287W6)`. Record the exact strings — `build-mas.sh` needs them verbatim.

- [ ] **Step 6: Record them**

Append the two identity strings, the profile filename, and the App ID to
`~/Documents/09 - Security & Recovery/Apple Developer ID/README.md`, in the same table style as the Developer ID entries. Note that the `.p12` rebuild command's `-keypbe`/`-macalg` flags apply to these certificates too if they are ever exported.

---

### Task 6: Privacy policy and support URLs

App Store Connect requires a **privacy policy URL**. `PRIVACY.md` exists but only as a repository file.

**Files:**
- Create: `.github/workflows/pages.yml` or equivalent, depending on the approach chosen

**Interfaces:**
- Produces: two public URLs for the App Store listing

- [ ] **Step 1: Publish the privacy policy**

Enable GitHub Pages for the repository, serving `PRIVACY.md`. The lightest approach is Settings → Pages → Deploy from branch → `main` → `/docs`, with `PRIVACY.md` copied to `docs/index.md`. Alternatively use the repository's rendered file URL, though a Pages URL reads better on a store listing.

- [ ] **Step 2: Verify both URLs resolve publicly**

```bash
curl -sSf -o /dev/null -w "privacy: %{http_code}\n" https://yarka-guru.github.io/connection_app/
curl -sSf -o /dev/null -w "support: %{http_code}\n" https://github.com/yarka-guru/connection_app/issues
```

Expected: `200` for both. Check in a private browser window too — a URL that works only while signed in will fail review.

---

### Task 7: `scripts/build-mas.sh`

Tauri has no `mas` bundle target, so packaging is manual.

**Files:**
- Create: `scripts/build-mas.sh`

**Interfaces:**
- Consumes: Tasks 2, 3, 5
- Produces: a signed, validated `ConnectionApp.pkg`

- [ ] **Step 1: Write the script**

Create `scripts/build-mas.sh`:

```bash
#!/usr/bin/env bash
# Builds the Mac App Store variant: no updater, sandboxed, signed with Apple
# Distribution, packaged as a .pkg for App Store Connect.
#
#   ./scripts/build-mas.sh [--upload]
#
# Requires: Apple Distribution + Mac Installer Distribution certificates in the
# login keychain, and ConnectionApp_MAS.provisionprofile alongside the other
# Apple credentials.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CREDS="$HOME/Documents/09 - Security & Recovery/Apple Developer ID"
PROFILE="$CREDS/ConnectionApp_MAS.provisionprofile"
APP_IDENTITY="Apple Distribution: Iaroslav Pyrogov (XG4FR287W6)"
PKG_IDENTITY="3rd Party Mac Developer Installer: Iaroslav Pyrogov (XG4FR287W6)"
TARGET="aarch64-apple-darwin"

cd "$REPO_ROOT"

[ -f "$PROFILE" ] || { echo "ERROR: missing $PROFILE"; exit 1; }

# The capability file names a plugin the MAS build does not register, and Tauri
# scans this directory regardless of cargo features. A listed permission for an
# unregistered plugin is a startup crash, so it has to go before building.
CAP="src-tauri/capabilities/updater.json"
RESTORE=""
if [ -f "$CAP" ]; then
  mv "$CAP" "/tmp/updater.json.bak"
  RESTORE=1
fi
restore() { [ -n "$RESTORE" ] && mv "/tmp/updater.json.bak" "$CAP"; }
trap restore EXIT

npm run build:vite
npx tauri build \
  --target "$TARGET" \
  --config src-tauri/tauri.mas.conf.json \
  --no-bundle -- --no-default-features --features gui

BUNDLE_DIR="src-tauri/target/$TARGET/release/bundle/macos"
npx tauri bundle \
  --target "$TARGET" \
  --config src-tauri/tauri.mas.conf.json \
  --bundles app

APP="$BUNDLE_DIR/ConnectionApp.app"
[ -d "$APP" ] || { echo "ERROR: no app bundle at $APP"; exit 1; }

cp "$PROFILE" "$APP/Contents/embedded.provisionprofile"

# Sign nested code first, then the bundle. --deep is deprecated by Apple and
# silently mis-signs nested content.
find "$APP/Contents/MacOS" -type f -perm +111 -print0 |
  while IFS= read -r -d '' f; do
    codesign --force --timestamp --options runtime \
      --entitlements src-tauri/Entitlements.mas.plist \
      --sign "$APP_IDENTITY" "$f"
  done

codesign --force --timestamp \
  --entitlements src-tauri/Entitlements.mas.plist \
  --sign "$APP_IDENTITY" "$APP"

PKG="$REPO_ROOT/ConnectionApp.pkg"
productbuild --component "$APP" /Applications --sign "$PKG_IDENTITY" "$PKG"

echo "=== verifying ==="
"$REPO_ROOT/scripts/verify-signing.sh" --mas-binary "$APP"
pkgutil --check-signature "$PKG" | head -3

if [ "${1:-}" = "--upload" ]; then
  xcrun altool --upload-app -f "$PKG" -t macos \
    --apiKey GBQH68KN3W --apiIssuer 799a6169-6e5d-4fc9-bac6-38993ccf145e
else
  echo
  echo "Built $PKG — re-run with --upload to send it to App Store Connect."
fi
```

- [ ] **Step 2: Make it executable and run it**

```bash
chmod +x scripts/build-mas.sh
./scripts/build-mas.sh
```

Expected: a `ConnectionApp.pkg` plus all three `--mas-binary` assertions passing.

The `npx tauri build`/`bundle` invocation is the least certain part of this plan — Tauri's CLI flags for splitting compilation from bundling vary between versions. If it errors, run `npx tauri build --help`, adjust, and record what worked in a comment at the top of the script. Do not paper over a failure by dropping `--no-default-features`; that would silently reintroduce the updater.

- [ ] **Step 3: Confirm the direct build still works**

```bash
./scripts/verify-signing.sh
cd src-tauri && cargo test && cd ..
```

Expected: all pass, and `src-tauri/capabilities/updater.json` is back in place — the script's `trap` restores it even on failure.

- [ ] **Step 4: Commit**

```bash
git add scripts/build-mas.sh scripts/verify-signing.sh
git commit -m "feat(mas): add Mac App Store build script

Tauri has no mas bundle target, so this builds without the updater
feature, embeds the provisioning profile, re-signs nested code then the
bundle with the sandboxed entitlements, and wraps it with productbuild.

It runs locally by design. MAS packaging fails with opaque ITMS errors,
and debugging those through Actions logs is far more expensive than
running it by hand until a build has been accepted once."
```

---

### Task 8: App Store Connect listing

**Files:** none

- [ ] **Step 1: Create the app record**

At App Store Connect → Apps → New App: platform macOS, name `ConnectionApp`, primary language English, bundle ID `com.connection-app.desktop`, SKU `connection-app-macos`.

- [ ] **Step 2: Produce screenshots**

macOS screenshots must be exactly 1280×800, 1440×900, 2560×1600 or 2880×1800. The app window is 650×700, so each shot needs compositing onto a backdrop. Capture with `cmd+shift+4`, then space, to get the window with its shadow, and place it on a 2560×1600 canvas.

Take at least three: the connection form with a project selected, an active connection showing credentials, and the saved-connections list. **Use demo mode for these** — real screenshots would leak AWS account details, endpoints and profile names.

- [ ] **Step 3: Fill in the metadata**

- Category: **Developer Tools**
- Privacy policy URL and support URL from Task 6
- Icon: `icon-1024.png` from Task 1
- Description, keywords, promotional text
- Age rating questionnaire
- Privacy nutrition labels: **Data Not Collected**, matching the empty `NSPrivacyCollectedDataTypes` in `PrivacyInfo.xcprivacy`

- [ ] **Step 4: Write the review notes**

Reviewers cannot be expected to know what SSM port forwarding is. State plainly:

> ConnectionApp is a developer tool for engineers who administer their own AWS
> infrastructure. It opens an encrypted tunnel to a database in the user's own
> AWS account, comparable to an SSH or database client.
>
> No AWS account is needed to evaluate it. On first launch, choose "Explore with
> sample data" to walk the entire flow with fabricated data — no network
> connection is made in that mode.
>
> The com.apple.security.network.server entitlement is required because the app
> listens on a local port on the user's own machine, which is the endpoint their
> database client connects to. Nothing is exposed outside the machine.

- [ ] **Step 5: Set pricing**

**Blocked until the user signs the Paid Apps Agreement and completes tax and banking details.**

Once available: price $9.99, and apply separately for the **Small Business Program** (15% instead of 30%).

If those are still outstanding and shipping sooner matters more than revenue, the Free Apps Agreement is already active — the app can ship free and be switched to paid later.

---

### Task 9: Submit

- [ ] **Step 1: Upload the build**

```bash
./scripts/build-mas.sh --upload
```

Expected: `altool` reports success. The build appears in App Store Connect after processing, typically within an hour.

- [ ] **Step 2: Test through TestFlight**

macOS TestFlight works. Install the processed build on a Mac that has never run ConnectionApp and confirm: the app launches sandboxed, demo mode works, granting access to `~/.aws` via the folder picker works, and a real connection succeeds.

This is the first time the sandbox code path (`sandbox.rs`) runs in anger — the direct build has never exercised it, since it has never been sandboxed.

- [ ] **Step 3: Submit for review**

Submit, and expect questions. First submissions of developer tools commonly draw a 2.1 request for clarification. If rejected, read the reviewer's note carefully before changing anything — the guideline they cite is usually precise.

---

## Sequencing

Tasks 1–4 are unblocked and independent of Apple. Task 5 needs the portal but no agreements. Tasks 6 and 8 are preparation. Task 9 requires the user's agreements for a paid release.

## Self-review notes

- **Spec coverage.** Covers the spec's Phase 2, demo mode, and listing sections in full.
- **Correction to the spec.** The spec assumed `invoke` was spread across seven files and proposed `src/lib/ipc.js`. It is used in two, and `Settings.svelte` receives it as a prop, so Task 4 swaps one reference instead. The `ipc.js` refactor is dropped.
- **Correction to the spec.** The spec did not mention the icon; the largest in the repo is 256×256 and App Store Connect requires 1024×1024. Added as Task 1.
- **Least certain step.** Task 7 Step 2 — Tauri's CLI flags for separating build from bundle differ across versions. It is flagged inline rather than presented as certain.
- **Trap avoided in Task 4.** The obvious wiring — call `initApp()` after installing the demo `invoke` — is wrong, because `initApp()` reassigns `invoke` from the Tauri API at line 143 and would silently clobber it. The plan calls `setupListenersAndLoad()` instead.
- **Untested by design.** No step claims the sandbox path works. It has never run in a signed build, and TestFlight (Task 9 Step 2) is the first honest test of it.
