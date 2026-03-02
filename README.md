# RDS SSM Connect

Secure database tunneling to AWS RDS through SSM port forwarding via bastion hosts. Available as a **desktop app** (Tauri) and a **CLI tool** (Node.js).

## Features

- **User-configurable projects** — define any number of RDS projects (Aurora clusters or RDS instances, PostgreSQL or MySQL)
- **Multiple simultaneous connections** with strict port availability checks
- **Saved connections** — bookmark frequently used profiles with one-click connect
- **Auto-reconnect** — transparently reconnects on the same port if the session drops unexpectedly
- **TargetNotConnected recovery** — cycles bastion instances via ASG when the SSM agent is disconnected
- **SSO support** — handles AWS SSO (OIDC device authorization) with automatic browser launch
- **Keepalive** — periodic TCP pings prevent SSM idle timeout
- **In-app updates** — checks GitHub releases, downloads and installs signed updates
- **Prerequisites validation** — detects missing dependencies on launch
- **Keyboard shortcuts** — `Cmd/Ctrl + ,` for settings
- **Accessible** — ARIA labels, focus trapping, keyboard navigation, screen reader support

## Prerequisites

- [Node.js](https://nodejs.org/) 22+ (CLI only)
- AWS profiles configured in `~/.aws/config`
- [Session Manager Plugin](https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html) (bundled with desktop app; CLI requires separate install)

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
wget https://github.com/yarka-guru/connection_app/releases/latest/download/RDS.SSM.Connect_2.0.1_arm64.deb
sudo dpkg -i RDS.SSM.Connect_2.0.1_arm64.deb
# x86_64:
# wget https://github.com/yarka-guru/connection_app/releases/latest/download/RDS.SSM.Connect_2.0.1_amd64.deb
# sudo dpkg -i RDS.SSM.Connect_2.0.1_amd64.deb
```

### Windows

Download the `.msi` or `.exe` installer from [GitHub Releases](https://github.com/yarka-guru/connection_app/releases).

No additional prerequisites — the app uses AWS SDK v3 natively.

### CLI (all platforms)

```bash
npm install -g rds_ssm_connect
```

The CLI requires the [Session Manager Plugin](https://docs.aws.amazon.com/systems-manager/latest/userguide/session-manager-working-with-install-plugin.html) to be installed separately.

## Usage

### Desktop App

Launch the app, select a project and environment, then click **Connect**. Connection credentials are displayed inline with one-click copy buttons. Save connections for quick access later.

Manage projects and AWS profiles in **Settings** (`Cmd/Ctrl + ,`).

### CLI

```bash
rds_ssm_connect
```

1. On first run with no projects configured, an interactive wizard walks you through creating one
2. Select a project
3. Select an environment (AWS profile)
4. SSO session is validated automatically (opens browser if needed)
5. The tool retrieves credentials from Secrets Manager, finds a bastion instance, and starts SSM port forwarding
6. Use the displayed connection details with your database client (`psql`, `mysql`, pgAdmin, DBeaver, etc.)

The tunnel stays open until you press `Ctrl+C`.

## How It Works

1. Reads AWS profiles from `~/.aws/config`
2. Loads project configurations from `~/.rds-ssm-connect/projects.json`
3. Filters profiles based on the selected project's `profileFilter`
4. Ensures AWS SSO session is valid (OIDC device authorization if needed)
5. Queries AWS Secrets Manager for RDS credentials (project-specific `secretPrefix`)
6. Finds a running bastion instance (tagged `Name=*bastion*`)
7. Gets the RDS endpoint (cluster or instance depending on project `rdsType`)
8. Starts an SSM port forwarding session with the correct local/remote ports
9. Displays connection details (host, port, username, password, database)

### Error Recovery

**TargetNotConnected** — when a bastion instance appears running but SSM agent is disconnected (exit code 254):

1. Terminates the disconnected instance
2. Waits for ASG to launch a replacement (up to 20 retries, 15s intervals)
3. Verifies the SSM agent is online
4. Retries port forwarding (up to 2 attempts)

**Auto-reconnect** — when an established session drops unexpectedly (idle timeout, network issue):

1. Verifies AWS credentials are still valid (avoids opening SSO tabs when user is away)
2. Re-discovers infrastructure (bastion may have been replaced by ASG)
3. Reconnects on the same local port (up to 3 attempts with 3s delay)

**Keepalive** — periodic TCP pings every 4 minutes prevent SSM from timing out idle connections.

## Project Configuration

Projects are stored in `~/.rds-ssm-connect/projects.json` and can be managed through the desktop app's Settings UI or the CLI's first-run wizard.

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
npm test              # Run tests
npm run dev:vite      # Vite dev server (frontend only)
npm run dev:gui       # Tauri dev mode (full app)
npm run build:vite    # Build frontend
npm run build:gui     # Build Tauri desktop app
```

### Architecture

```
connect.js              CLI entry point + core connection logic
gui-adapter.js          IPC bridge — JSON stdin/stdout protocol for Tauri sidecar
configLoader.js         Project config CRUD (~/.rds-ssm-connect/projects.json)
src/
  aws-clients.js        AWS SDK client factory (STS, EC2, RDS, SSM, Secrets Manager)
  aws-operations.js     AWS operations (find bastion, get endpoint, get credentials, etc.)
  credential-resolver.js  AWS credential chain resolution (SSO, profiles)
  sso-login.js          AWS SSO OIDC device authorization flow
  plugin-resolver.js    Locates session-manager-plugin binary
src-tauri/
  src/lib.rs            Tauri commands (connect, disconnect, saved connections, project
                        config CRUD, AWS profile CRUD, updates, prerequisites check)
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
    Settings.svelte       Project management + AWS profile CRUD + raw config editor
    PrerequisitesCheck.svelte  Missing dependency warnings
    UpdateBanner.svelte   In-app update notification
```

### Tech Stack

- **Frontend**: Svelte 5 (runes), Vite
- **Desktop**: Tauri v2 (Rust)
- **Backend**: Node.js sidecar bundled with esbuild + pkg
- **AWS SDK**: v3 (STS, EC2, RDS, SSM, Secrets Manager, SSO OIDC)
- **Linter**: Biome

## Publishing

- **npm**: Published automatically via GitHub Actions when a release is created
- **Desktop**: Multi-platform builds (macOS ARM64/x64, Linux ARM64/x64, Windows x64) via `tauri-action` on git tags

## License

ISC
