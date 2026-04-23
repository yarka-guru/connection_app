# Changelog

All notable changes to this project will be documented in this file.

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
