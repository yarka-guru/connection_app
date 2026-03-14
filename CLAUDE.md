# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**ConnectionApp** is a Tauri desktop app and standalone Rust CLI that enables secure tunneling through AWS Systems Manager (SSM) port forwarding. Supports RDS databases and service connections (VNC/RDP) via bastion hosts. The backend is 100% Rust — no Node.js sidecar, no external plugins. Projects are user-configurable — the tool reads AWS profiles from `~/.aws/config`, loads project definitions from `~/.connection-app/projects.json`, retrieves database credentials from AWS Secrets Manager, and sets up native WebSocket-based port forwarding through a bastion instance.

## Key Architecture

### Rust Backend (`src-tauri/src/`)

#### Core Modules
- `aws/credentials.rs` — AWS SDK client factory (STS, EC2, RDS, SSM, Secrets Manager)
- `aws/operations.rs` — AWS operations (find bastion, get endpoint, get credentials, start session)
- `aws/sso.rs` — AWS SSO OIDC device authorization flow with trait-based handler (`GuiSsoHandler`, `CliSsoHandler`)
- `config/aws_config.rs` — Reads/writes `~/.aws/config`, respects `AWS_CONFIG_FILE` env var for sandbox
- `config/projects.rs` — Project config CRUD for `~/.connection-app/projects.json`
- `config/validation.rs` — Validates required fields, region format, port format, shell-safe patterns
- `error.rs` — Unified `AppError` enum with Tauri serialization

#### Tunnel (Native WebSocket)
- `tunnel/native.rs` — Native SSM port forwarding over WebSocket (replaces session-manager-plugin)
- `tunnel/websocket.rs` — WebSocket client with SigV4-signed connection
- `tunnel/protocol.rs` — SSM binary protocol (channel open, data, ack, etc.)
- `tunnel/manager.rs` — Multi-connection lifecycle manager with Tauri event emission

#### macOS App Sandbox
- `sandbox.rs` — Security-scoped bookmark management via CoreFoundation FFI
  - `grant_aws_dir_access()` — folder picker + bookmark creation
  - `activate_aws_dir_access()` — bookmark resolution + `startAccessingSecurityScopedResource`
  - `AwsDirAccess` — RAII guard that calls `stopAccessingSecurityScopedResource` on drop

#### Tauri Commands (`commands/`)
- `connection.rs` — connect, disconnect, disconnect_all, get_active_connections_list, sso_login
- `profiles.rs` — list_profiles, read_aws_config, save/delete_aws_profile, raw config editor
- `projects.rs` — list_projects, list_project_configs, save/delete_project_config
- `saved.rs` — Saved connections CRUD (bookmarked connections with one-click connect)
- `system.rs` — Updates, version, open_url, quit, sandbox status, grant_aws_access

#### CLI Binary
- `cli.rs` — Standalone CLI (`connection-app-cli`) using clap + dialoguer for interactive selection

### Frontend (`src/`)
- `App.svelte` — Main app shell (Svelte 5 with runes), sandbox setup screen
- `lib/ConnectionForm.svelte` — Project/environment selector + connect button
- `lib/ActiveConnections.svelte` — Live connection panels with credentials
- `lib/SavedConnections.svelte` — Bookmarked connections list
- `lib/Settings.svelte` — Project management + AWS profile CRUD + raw config editor
- `lib/SessionStatus.svelte` — Connection status indicator
- `lib/UpdateBanner.svelte` — In-app update notification
- `lib/CopyButton.svelte` — Reusable copy-to-clipboard with feedback
- `lib/ConfirmDialog.svelte` — Reusable confirmation modal
- `lib/utils.js` — Shared utilities (clipboard, timeout, focus trap)

### Core Flow
1. Read AWS profiles from `~/.aws/config`
2. Load project configs from `~/.connection-app/projects.json`
3. Select project (filtered by available profiles)
4. Filter environments (AWS profiles) based on project's `profileFilter`
5. Ensure SSO session is valid (OIDC device authorization if needed)
6. Query Secrets Manager for RDS credentials (project-specific `secretPrefix`)
7. Find running bastion instance (tagged with `Name=*bastion*`)
8. Get RDS endpoint (cluster or instance based on project's `rdsType`)
9. Start SSM session and open native WebSocket port forwarding
10. Display connection details for database client

## Development Commands

### Setup
```bash
npm install           # Frontend dependencies only
```

### Desktop App
```bash
npm run dev:gui       # Tauri dev mode (full app with hot reload)
npm run build:gui     # Build Tauri desktop app
```

### Frontend Only
```bash
npm run dev:vite      # Vite dev server (frontend only, no Rust backend)
npm run build:vite    # Build frontend assets
```

### Rust
```bash
cd src-tauri
cargo check                    # Type-check
cargo test                     # Run Rust tests
cargo check --no-default-features  # Verify CLI compiles without gui features
```

### Linting
```bash
npx @biomejs/biome check .     # Lint JS/TS/Svelte
npx @biomejs/biome check --write .  # Lint + auto-fix
```

### Publishing
- **Desktop**: Multi-platform builds (macOS ARM64/x64, Linux ARM64/x64, Windows x64) via `tauri-action` on git tags
- **Homebrew**: Auto-updated tap via `.github/workflows/update-homebrew.yml`

## Prerequisites for Running

- Rust toolchain (for building)
- Node.js 22+ (for frontend build tooling only)
- Properly configured `~/.aws/config` with named profiles

Note: No external tools required at runtime — the app uses AWS SDK v1 (Rust) natively and implements the SSM WebSocket protocol directly.

## AWS Resource Naming Conventions

The application relies on AWS resource naming patterns defined per project in `~/.connection-app/projects.json`:

- **Secrets**: Must start with the project's `secretPrefix` (e.g., `rds!cluster`, `rds!db`)
- **Bastion instances**: Must be tagged with `Name=*bastion*` and in `running` state
- **RDS clusters**: DBClusterIdentifier must match the project's `rdsPattern` and be `available` (when `rdsType` is `cluster`)
- **RDS instances**: DBInstanceIdentifier must match the project's `rdsPattern` and be `available` (when `rdsType` is `instance`)

## Important Notes

- Projects are user-configurable via `~/.connection-app/projects.json` (no hardcoded defaults)
- Port assignment is based on project-specific `envPortMapping` with `defaultPort` fallback
- AWS region is determined by the selected project's `region` field
- SSO sessions are validated before connecting; browser opens automatically if needed
- Frontend communicates with Rust backend via Tauri IPC (`invoke`)
- Rust backend emits named events: `sso-status`, `sso-open-url`, `status`, `disconnected`, `connection-error`
- macOS App Sandbox supported via security-scoped bookmarks (setup screen on first launch)
- `AWS_CONFIG_FILE` env var is set by sandbox module so all code transparently reads from the bookmarked path

## Cargo Features

- `gui` (default) — Tauri desktop app with all plugins and commands
- No features — CLI-only binary (`connection-app-cli`)
