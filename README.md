# RDS SSM Connect

Secure database tunneling to AWS RDS through SSM port forwarding via bastion hosts. Available as a **desktop app** (Tauri) and a **CLI tool** (Node.js).

## Features

- **Multi-project support** — TLN (Aurora clusters, us-east-2) and Covered (RDS instances, us-west-1)
- **Multiple simultaneous connections** with automatic port assignment
- **Saved connections** — bookmark frequently used profiles with one-click connect
- **Auto-reconnect** — handles `TargetNotConnected` errors by cycling bastion instances via ASG
- **In-app updates** — checks GitHub releases, downloads and installs signed updates
- **Prerequisites validation** — detects missing `aws-vault` and AWS CLI on launch
- **Keyboard shortcuts** — `Cmd/Ctrl + ,` for settings
- **Accessible** — ARIA labels, focus trapping, keyboard navigation, screen reader support

## Prerequisites

- [aws-vault](https://github.com/99designs/aws-vault) — AWS credential management
- [AWS CLI](https://aws.amazon.com/cli/) — AWS API access
- [Node.js](https://nodejs.org/) 22+ (CLI only)
- AWS profiles configured in `~/.aws/config`

## Installation

### Desktop App

Download the latest installer for your platform from [GitHub Releases](https://github.com/yarka-guru/connection_app/releases):

| Platform | Format |
|----------|--------|
| macOS (Apple Silicon + Intel) | `.dmg` |
| Windows | `.msi` / `.exe` |
| Linux | `.deb` / `.AppImage` |

### CLI

```bash
npm install -g rds_ssm_connect
```

## Usage

### Desktop App

Launch the app, select a project and environment, then click **Connect**. Connection credentials are displayed inline with one-click copy buttons. Save connections for quick access later.

### CLI

```bash
rds_ssm_connect
```

1. Select a project (TLN or Covered)
2. Select an environment (AWS profile)
3. The tool retrieves credentials from Secrets Manager, finds a bastion instance, and starts SSM port forwarding
4. Use the displayed connection string with your database client (`psql`, pgAdmin, DBeaver, etc.)

The tunnel stays open until you press `Ctrl+C`.

## How It Works

1. Reads AWS profiles from `~/.aws/config`
2. Filters profiles based on the selected project
3. Queries AWS Secrets Manager for RDS credentials (project-specific prefix)
4. Finds a running bastion instance (tagged `Name=*bastion*`)
5. Gets the RDS endpoint (cluster or instance depending on project)
6. Starts an SSM port forwarding session with the correct local port
7. Displays connection details (host, port, username, password, database)

### Error Recovery

When a bastion instance appears running but SSM agent is disconnected (`TargetNotConnected`, exit code 254):

1. Terminates the disconnected instance
2. Waits for ASG to launch a replacement (up to 20 retries, 15s intervals)
3. Verifies the SSM agent is online
4. Retries port forwarding (up to 2 attempts)

## Project Configuration

| | TLN (EMR) | Covered Healthcare |
|---|---|---|
| Region | us-east-2 | us-west-1 |
| Database | emr | covered_db |
| RDS type | Aurora cluster | RDS instance |
| Secret prefix | `rds!cluster` | `rds!db` |
| Port range | 5432–5452 | 5460–5461 |

Port assignments are based on environment suffix mappings defined in `envPortMapping.js`.

## Development

### Setup

```bash
npm install
```

### Commands

```bash
npm test              # Run tests
npm run dev:vite      # Vite dev server (frontend only)
npm run dev:gui       # Tauri dev mode (full app)
npm run build:vite    # Build frontend
npm run build:gui     # Build Tauri desktop app
```

### Architecture

```
connect.js              CLI entry point (shebang, runs standalone)
gui-adapter.js          IPC bridge — JSON stdin/stdout protocol for Tauri sidecar
envPortMapping.js       Multi-project configuration (regions, ports, patterns)
src-tauri/
  src/lib.rs            Tauri commands (connect, disconnect, save, update, etc.)
  tauri.conf.json       App config, plugins, window settings, bundling
src/
  App.svelte            Main app shell (Svelte 5 with runes)
  lib/
    utils.js            Shared utilities (clipboard, timeout, focus trap)
    CopyButton.svelte   Reusable copy-to-clipboard with feedback
    ConfirmDialog.svelte Reusable confirmation modal
    ConnectionForm.svelte  Project/environment selector + connect button
    SavedConnections.svelte  Bookmarked connections list
    ActiveConnections.svelte  Live connection panels with credentials
    SessionStatus.svelte    Connection status indicator
    Settings.svelte       AWS profile management (CRUD + raw config editor)
    PrerequisitesCheck.svelte  Missing dependency warnings
    UpdateBanner.svelte   In-app update notification
```

### Tech Stack

- **Frontend**: Svelte 5 (runes), Vite
- **Desktop**: Tauri v2 (Rust)
- **Backend**: Node.js sidecar bundled with esbuild + pkg
- **AWS SDK**: v3 (EC2, RDS, SSM, Secrets Manager)
- **Linter**: Biome

## Publishing

- **npm**: Published automatically via GitHub Actions when a release is created
- **Desktop**: Multi-platform builds (macOS ARM64/x64, Linux x64, Windows x64) via `tauri-action` on git tags

## License

ISC
