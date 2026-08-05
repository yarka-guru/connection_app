# Changelog

All notable changes to this project will be documented in this file.

## [3.7.7] - 2026-08-05

### Changed
- Dependency maintenance. All direct dependencies are now at their latest
  published versions:
  - npm: `@biomejs/biome` 2.5.4→2.5.7, `svelte` 5.56.5→5.56.8,
    `vite` 8.1.5→8.2.0, `@sveltejs/vite-plugin-svelte` 7.1.2→7.2.0,
    `@tauri-apps/cli` 2.11.2→2.11.4, the three `@fontsource*` packages
    5.2.x→5.3.0.
  - cargo: `tokio` 1.52.3→1.53.1, `aws-config` 1.8.18→1.10.1 and the nine
    other `aws-*` crates, `base64` 0.22→0.23, `tokio-tungstenite` 0.29→0.30,
    `serde`, `serde_json`, `thiserror`, `clap`, `tokio-util`, `futures-util`,
    `tauri-plugin-dialog`, `tauri-plugin-store`, plus transitive updates.

### Security
- `postcss` moderate advisory GHSA-fxqj-rqcc-2cmp (arbitrary `.map` file read
  via attacker-controlled `sourceMappingURL`) is resolved by the `vite` bump.
  It reached the tree as a build-time transitive dependency only and was never
  shipped in a release artifact. `npm audit` and `cargo audit` now both report
  zero vulnerabilities. `cargo audit`'s remaining 17 findings are
  unmaintained/unsound notices for Tauri's transitive GTK3 stack
  (`gtk`, `gdk`, `atk`, …), `proc-macro-error` and `unic-*` — not actionable
  from this repo.

## [3.7.6] - 2026-07-16

### Fixed
- Linux auto-update works again. `tauri.conf.json` never listed `appimage` as
  a bundle target, so no AppImage was built, while `generate-update-json.js`
  advertised Linux AppImage URLs unconditionally — `latest.json` pointed at
  files that 404'd, with empty signatures. Tauri's Linux updater supports
  AppImage only; deb/rpm cannot self-update.
- The release workflow's signing step now exits 1 instead of 0 when no updater
  artifact is found. That soft failure is why v3.7.5 shipped green and broken.

### Changed
- `Cargo.toml` declares `rust-version = "1.94.1"` (floor set by
  `aws-smithy-*` 1.2+), so an old local toolchain fails with a clear message
  instead of a cryptic resolution error. CI tracks latest stable and would
  never catch this.

## [3.7.5] - 2026-07-16

### Changed
- Re-enabled the Linux release targets (`x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`) disabled in v3.7.2, repopulating the Linux
  updater signatures that had been empty since. Windows remains disabled.
- Dependency refresh: npm (`@biomejs/biome`, `svelte`, `vite`) and 29
  transitive cargo crates, plus `aws-smithy-http-client` 1.1.13→1.2.0 (#35).

## [3.7.4] - 2026-07-09

### Fixed
- Random session deaths at ~32s, the second and final root cause behind the
  recurring "tunnel dies 30s in" failures. The SSM data-channel handshake loop
  had no sequence-number deduplication, so an ack delayed by ordinary network
  jitter made the client advance `expected_incoming` past reality and answer a
  retransmitted `HandshakeRequest` with a *second* `HandshakeResponse` bearing
  a fresh sequence number — a protocol violation that jammed the agent's
  handshake state machine until it closed the session. Duplicates are now
  always acked (that is what stops the agent retransmitting), processed only
  when the sequence number is the expected one, and a retransmitted
  `HandshakeRequest` is answered with the cached, byte-identical response.
- The 30s connection health check no longer opens a real TCP connection to the
  local port. In multiplexed mode each probe created a smux stream, making the
  agent dial and drop the remote host (RDS/VNC) once per connection, forever.
  It now uses a bind-conflict check instead.

## [3.7.3] - 2026-07-08

### Fixed
- Idle sessions killed at 30s: the smux keepalive interval equalled the agent's
  30s idle timeout, so the two raced. NOP frames are now sent every 10s.

## [3.7.2] - 2026-07-08

### Fixed
- The app no longer terminates the shared bastion instance.

### Security
- Patched high-severity CVEs in dependencies.

## [3.7.1] - 2026-06-22

### Fixed
- Homebrew cask no longer emits a deprecation warning on `brew` operations:
  `depends_on macos` now uses the symbol form (`:monterey`) instead of the
  deprecated string-comparison form (`">= :monterey"`). The fix lands in both
  the published cask and the `update-homebrew` workflow that regenerates it,
  so future releases keep it. The macOS minimum (Monterey or newer) is
  unchanged.

### Changed
- Dependency maintenance: grouped Cargo (9 crates), npm (2 packages), and
  GitHub Actions (`actions/checkout` 6→7) updates (#26, #27, #28).

## [3.7.0] - 2026-06-10

### Fixed
- Smux protocol v1 compliance — three tunnel-killing bugs (#24):
  - Stop sending `cmdUPD` window updates: the SSM agent runs xtaci/smux
    protocol v1 where `cmdUPD` is v2-only; receiving one made the agent close
    the entire mux session. Every bulk transfer (`pg_dump`, large `SELECT`s)
    died deterministically at ~2 MiB, killing all tunnel connections with it
  - Uploads larger than 4 MiB no longer hang: `send_data` waited on window
    refills a v1 agent never sends
  - Idle tunnels no longer self-disconnect after 60s: the smux keepalive
    expected NOP frames modern agents never send; dead-tunnel detection now
    belongs solely to the SSM-level watchdogs
- Dead-WebSocket watchdog counts any inbound frame (pong or data) as
  liveness — previously a sustained bulk download could starve pong
  processing and the watchdog killed sessions mid-transfer (#24)
- CLI command reader terminates at stdin EOF instead of busy-looping on a
  CPU core when stdin is closed (piped/backgrounded runs) (#24)

### Changed
- Credentials are pinned to the explicitly selected profile: environment
  credentials inherited from the launching shell (`aws-vault exec`, CI) can
  no longer silently override the profile choice and connect a "prod"-labeled
  tunnel to a different account (#24)
- Per-stream channel sends no longer hold the streams lock across await —
  one slow stream cannot stall dispatch for other streams (#24)

### Verified
- Live 4-way stress test: two concurrent full prod dumps (62.8 MB each) plus
  two concurrent staging dumps (21 MB each) through two mux tunnels
  simultaneously — ~167 MB total, zero errors

## [3.5.1] - 2026-04-23

### Security
- Disable default `rustls` feature on every `aws-sdk-*` crate. The feature
  activated `aws-smithy-http-client/legacy-rustls-ring`, which shipped the
  legacy `rustls 0.21` / `rustls-webpki 0.101.7` / `hyper-rustls 0.24` stack
  alongside the modern `rustls-aws-lc` path we actually use. Clears the final
  two Dependabot alerts and trims the release binary.

## [3.5.0] - 2026-04-23

### Added
- New default theme "Aubergine Nebula" (plum + copper/lavender) with drifting
  orb ambient background and SVG noise overlay
- Self-hosted Geist, Geist Mono, and Instrument Serif fonts via
  `@fontsource-variable` packages
- Consolidated design tokens in `src/app.css` (typography scale, radii scale,
  motion easings, glass blur tiers)

### Changed
- Themes are now applied via `data-theme` / `data-theme-variant` attributes;
  tokens live in CSS rather than JS
- Legacy Obsidian theme preserved as the `obsidian-classic` variant
- Stored `forest` / `cream` / `frost` theme preferences auto-migrate to the new
  keys on launch

### Security
- Patch 4 high-severity and 1 low-severity `openssl` advisories by bumping
  0.10.76 → 0.10.78 (buffer overflows in `Deriver::derive`, AES key wrap, PSK
  cookie trampolines, `MdCtxRef::digest_final`, PEM password callback)
- Bump `rustls-webpki` 0.103.10 → 0.103.13 and `rand` 0.9.2 → 0.9.4

## [2.1.7] - 2026-03-04

### Fixed
- SSO token polling: use typed SDK error matching instead of fragile string matching
- Homebrew post-install: delete existing .desktop file before writing (fixes reinstall failure)
- Replace debug format `{:?}` with display format `{}` in user-facing error messages
- Replace `unwrap()` calls with proper error handling in CLI and SSO modules

### Added
- CI workflow: lint-frontend and check-rust jobs on push/PR
- GitHub issue templates and pull request template
- CHANGELOG.md with version history
- Cargo metadata: license, repository, homepage, keywords

### Changed
- SECURITY.md: replace placeholder with real security policy
- README.md: fix license from ISC to MIT
- Clippy: fix all warnings (collapsible_if, sort_by_key, too_many_arguments)
- Gate GUI binary behind `required-features = ["gui"]` for clean `--no-default-features` builds

## [2.1.6] - 2026-03-04

### Fixed
- Linux Homebrew update: run `brew update` first, correct restart, desktop icon

## [2.1.5] - 2026-03-03

### Fixed
- SSM protocol: Jacobson/Karels RTT estimation and flag retransmission

## [2.1.4] - 2026-03-03

### Fixed
- Linux install-method-aware updates and TCP reconnection
- Homebrew desktop icon support

## [2.1.3] - 2026-03-03

### Fixed
- 3 high-severity Dependabot alerts (aws-lc-sys updated to 0.38.0)

## [2.1.2] - 2026-03-03

### Fixed
- SSM tunnel data forwarding: sequence numbers continue from handshake
- Homebrew workflow: add `workflow_dispatch` trigger and version input

## [2.1.1] - 2026-03-03

### Fixed
- Auto-update system: publish releases, progress UI, Linux pkexec fallback
- Version sync across tauri.conf.json and package.json

## [2.1.0] - 2026-03-03

### Added
- Theme switcher with 5 selectable themes
- Forest palette UI retheme

### Fixed
- SSO token `expiresAt` format (use Z suffix instead of +00:00)
- CLI output: dynamic-width box, simplified SSO handler format strings

## [2.0.2] - 2026-03-02

### Changed
- Remove prerequisites check — Session Manager Plugin is bundled
- Remove AWS CLI from prerequisites — app uses SDK natively

## [2.0.1] - 2026-03-02

### Added
- Pure Rust backend — native SSM WebSocket port forwarding (no Node.js sidecar)
- macOS App Sandbox support with security-scoped bookmarks
- Standalone CLI binary (`connection-app-cli`)
- Saved connections with one-click connect
- Multi-connection support with port availability checks
- Auto-reconnect and TargetNotConnected recovery
- In-app updates with signed releases

### Changed
- Complete rewrite from Node.js/Python to Rust backend
- AWS SDK v1 (Rust) replaces AWS CLI subprocess calls
