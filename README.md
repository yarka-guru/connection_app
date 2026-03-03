# RDS SSM Connect

Secure database tunneling to AWS RDS through SSM port forwarding via bastion hosts. Available as a **desktop app** (Tauri) and a **CLI tool**.

## Features

- **User-configurable projects** — define any number of RDS projects (Aurora clusters or RDS instances, PostgreSQL or MySQL)
- **Multiple simultaneous connections** with strict port availability checks
- **Saved connections** — bookmark frequently used profiles with one-click connect
- **Native WebSocket tunneling** — no external plugins required, SSM protocol implemented in Rust
- **Auto-reconnect** — transparently reconnects on the same port if the session drops unexpectedly
- **TargetNotConnected recovery** — cycles bastion instances via ASG when the SSM agent is disconnected
- **SSO support** — handles AWS SSO (OIDC device authorization) with automatic browser launch
- **Keepalive** — periodic TCP pings prevent SSM idle timeout
- **In-app updates** — checks GitHub releases, downloads and installs signed updates
- **macOS App Sandbox** — supports App Store distribution with security-scoped bookmarks
- **Keyboard shortcuts** — `Cmd/Ctrl + ,` for settings
- **Accessible** — ARIA labels, focus trapping, keyboard navigation, screen reader support

## Prerequisites

- AWS profiles configured in `~/.aws/config`

No additional tools are required — the app uses the AWS SDK natively and implements the SSM WebSocket protocol directly.

## Installation

### macOS (Homebrew)

```bash
brew tap yarka-guru/tap
brew install --cask rds-ssm-connect
```

Or download the `.dmg` directly from [GitHub Releases](https://github.com/yarka-guru/connection_app/releases).

### Linux

#### Option A: Homebrew

```bash
# Install Homebrew if not already installed
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
echo 'eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"' >> ~/.bashrc
eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"

# Install the app
brew tap yarka-guru/tap
brew install yarka-guru/tap/rds-ssm-connect

# Make brew tools visible to the desktop app
echo 'export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"' | sudo tee /etc/profile.d/linuxbrew.sh
# Log out and back in for this to take effect
```

#### Option B: Direct .deb install

```bash
# Download and install the app (check GitHub Releases for the latest version)
# ARM64:
wget https://github.com/yarka-guru/connection_app/releases/latest/download/RDS.SSM.Connect_2.0.2_arm64.deb
sudo dpkg -i RDS.SSM.Connect_2.0.2_arm64.deb
# x86_64:
# wget https://github.com/yarka-guru/connection_app/releases/latest/download/RDS.SSM.Connect_2.0.2_amd64.deb
# sudo dpkg -i RDS.SSM.Connect_2.0.2_amd64.deb
```

### Windows

Download the `.msi` or `.exe` installer from [GitHub Releases](https://github.com/yarka-guru/connection_app/releases).

## Usage

### Desktop App

Launch the app, select a project and environment, then click **Connect**. Connection credentials are displayed inline with one-click copy buttons. Save connections for quick access later.

Manage projects and AWS profiles in **Settings** (`Cmd/Ctrl + ,`).

### CLI

The CLI binary is included alongside the desktop app or can be built separately:

```bash
rds-ssm-connect-cli
```

1. On first run with no projects configured, create one in `~/.rds-ssm-connect/projects.json`
2. Select a project
3. Select an environment (AWS profile)
4. SSO session is validated automatically (opens browser if needed)
5. The tool retrieves credentials from Secrets Manager, finds a bastion instance, and starts native WebSocket port forwarding
6. Use the displayed connection details with your database client (`psql`, `mysql`, pgAdmin, DBeaver, etc.)

The tunnel stays open until you press `Ctrl+C`.

#### CLI Options

```
rds-ssm-connect-cli [OPTIONS] [COMMAND]

Options:
  -p, --project <NAME>    Project name (skip interactive selection)
      --profile <NAME>    AWS profile name (skip interactive selection)
      --port <PORT>       Local port override

Commands:
  projects    List configured projects
  profiles    List AWS profiles
```

## How It Works

1. Reads AWS profiles from `~/.aws/config`
2. Loads project configurations from `~/.rds-ssm-connect/projects.json`
3. Filters profiles based on the selected project's `profileFilter`
4. Ensures AWS SSO session is valid (OIDC device authorization if needed)
5. Queries AWS Secrets Manager for RDS credentials (project-specific `secretPrefix`)
6. Finds a running bastion instance (tagged `Name=*bastion*`)
7. Gets the RDS endpoint (cluster or instance depending on project `rdsType`)
8. Starts an SSM session and opens a native WebSocket tunnel for port forwarding
9. Displays connection details (host, port, username, password, database)

### Error Recovery

**TargetNotConnected** — when a bastion instance appears running but SSM agent is disconnected:

1. Terminates the disconnected instance
2. Waits for ASG to launch a replacement (up to 20 retries, 15s intervals)
3. Verifies the SSM agent is online
4. Retries port forwarding (up to 2 attempts)

**Auto-reconnect** — when an established session drops unexpectedly (idle timeout, network issue):

1. Verifies AWS credentials are still valid
2. Re-discovers infrastructure (bastion may have been replaced by ASG)
3. Reconnects on the same local port (up to 3 attempts with 3s delay)

**Keepalive** — periodic TCP pings every 4 minutes prevent SSM from timing out idle connections.

## Project Configuration

Projects are stored in `~/.rds-ssm-connect/projects.json` and can be managed through the desktop app's Settings UI.

Each project defines:

| Field | Description | Example |
|---|---|---|
| `name` | Display name | `"My Project"` |
| `region` | AWS region | `"us-east-2"` |
| `database` | Database name | `"mydb"` |
| `secretPrefix` | Secrets Manager prefix | `"rds!cluster"` |
| `rdsType` | `"cluster"` or `"instance"` | `"cluster"` |
| `engine` | `"postgres"` or `"mysql"` | `"postgres"` |
| `rdsPattern` | RDS identifier pattern | `"my-app-rds-aurora"` |
| `profileFilter` | AWS profile prefix filter (optional) | `"my-app"` |
| `envPortMapping` | Environment suffix to local port mapping | `{"-staging": "5433"}` |
| `defaultPort` | Fallback local port | `"5432"` |

Example `projects.json`:

```json
{
  "my-project": {
    "name": "My Project",
    "region": "us-east-2",
    "database": "mydb",
    "secretPrefix": "rds!cluster",
    "rdsType": "cluster",
    "engine": "postgres",
    "rdsPattern": "my-app-rds-aurora",
    "profileFilter": null,
    "envPortMapping": {
      "-prod": "5432",
      "-staging": "5433"
    },
    "defaultPort": "5432"
  }
}
```

## Development

### Setup

```bash
npm install
```

### Commands

```bash
npm run dev:vite      # Vite dev server (frontend only)
npm run dev:gui       # Tauri dev mode (full app)
npm run build:vite    # Build frontend
npm run build:gui     # Build Tauri desktop app
```

### Rust

```bash
cd src-tauri
cargo check           # Type-check
cargo test            # Run tests
```

### Architecture

```
src-tauri/src/
  lib.rs              Tauri app setup, plugin registration, command handlers
  cli.rs              Standalone CLI binary (rds-ssm-connect-cli)
  sandbox.rs          macOS App Sandbox support (security-scoped bookmarks)
  error.rs            Unified AppError enum
  aws/
    credentials.rs    AWS SDK client factory (STS, EC2, RDS, SSM, Secrets Manager)
    operations.rs     AWS operations (find bastion, get endpoint, get credentials)
    sso.rs            AWS SSO OIDC device authorization flow
  config/
    aws_config.rs     ~/.aws/config reader/writer
    projects.rs       Project config CRUD (~/.rds-ssm-connect/projects.json)
    validation.rs     Project config validation
  tunnel/
    native.rs         Native SSM port forwarding over WebSocket
    websocket.rs      WebSocket client with SigV4-signed connection
    protocol.rs       SSM binary protocol implementation
    manager.rs        Multi-connection lifecycle manager
  commands/
    connection.rs     Connect/disconnect Tauri commands
    profiles.rs       AWS profile management commands
    projects.rs       Project config management commands
    saved.rs          Saved connections CRUD commands
    system.rs         Updates, version, sandbox, quit commands
src/
  App.svelte          Main app shell (Svelte 5 with runes)
  lib/
    utils.js          Shared utilities (clipboard, timeout, focus trap)
    CopyButton.svelte     Reusable copy-to-clipboard with feedback
    ConfirmDialog.svelte  Reusable confirmation modal
    ConnectionForm.svelte Project/environment selector + connect button
    SavedConnections.svelte  Bookmarked connections list
    ActiveConnections.svelte Live connection panels with credentials
    SessionStatus.svelte    Connection status indicator
    Settings.svelte       Project management + AWS profile CRUD + raw config editor
    UpdateBanner.svelte   In-app update notification
```

### Tech Stack

- **Frontend**: Svelte 5 (runes), Vite
- **Desktop**: Tauri v2
- **Backend**: Rust (pure — no Node.js sidecar)
- **AWS SDK**: Rust SDK v1 (STS, EC2, RDS, SSM, Secrets Manager, SSO OIDC)
- **Tunneling**: Native WebSocket (SSM protocol implemented in Rust)
- **Linter**: Biome

## Publishing

- **Desktop**: Multi-platform builds (macOS ARM64/x64, Linux ARM64/x64, Windows x64) via `tauri-action` on git tags
- **Homebrew**: Auto-updated tap via GitHub Actions

## License

ISC
