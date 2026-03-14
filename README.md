# ConnectionApp

Secure tunneling through AWS SSM port forwarding. Supports RDS databases (Aurora/RDS, PostgreSQL/MySQL) and service connections (VNC/RDP via EC2 or ECS). Available as a **desktop app** (Tauri) and a **CLI tool**.

## Features

- **User-configurable projects** — define RDS databases or service connections (VNC/RDP)
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
brew install --cask connection-app
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
brew install yarka-guru/tap/connection-app

# Make brew tools visible to the desktop app
echo 'export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"' | sudo tee /etc/profile.d/linuxbrew.sh
# Log out and back in for this to take effect
```

#### Option B: Direct .deb install

```bash
# Download and install the app (check GitHub Releases for the latest version)
# ARM64:
wget https://github.com/yarka-guru/connection_app/releases/latest/download/ConnectionApp_3.1.0_arm64.deb
sudo dpkg -i ConnectionApp_3.1.0_arm64.deb
# x86_64:
# wget https://github.com/yarka-guru/connection_app/releases/latest/download/ConnectionApp_3.1.0_amd64.deb
# sudo dpkg -i ConnectionApp_3.1.0_amd64.deb
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
connection-app-cli
```

1. On first run with no projects configured, create one in `~/.connection-app/projects.json`
2. Select a project
3. Select an environment (AWS profile)
4. SSO session is validated automatically (opens browser if needed)
5. The tool retrieves credentials from Secrets Manager, finds a bastion instance, and starts native WebSocket port forwarding
6. Use the displayed connection details with your database client (`psql`, `mysql`, pgAdmin, DBeaver, etc.)

The tunnel stays open until you press `Ctrl+C`.

#### CLI Options

```
connection-app-cli [OPTIONS] [COMMAND]

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
2. Loads project configurations from `~/.connection-app/projects.json`
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

Projects are stored in `~/.connection-app/projects.json` and can be managed through the desktop app's Settings UI.

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
  cli.rs              Standalone CLI binary (connection-app-cli)
  sandbox.rs          macOS App Sandbox support (security-scoped bookmarks)
  error.rs            Unified AppError enum
  aws/
    credentials.rs    AWS SDK client factory (STS, EC2, RDS, SSM, Secrets Manager)
    operations.rs     AWS operations (find bastion, get endpoint, get credentials)
    sso.rs            AWS SSO OIDC device authorization flow
  config/
    aws_config.rs     ~/.aws/config reader/writer
    projects.rs       Project config CRUD (~/.connection-app/projects.json)
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

## SSM Protocol Compatibility

The native WebSocket tunnel implements the SSM port forwarding protocol in pure Rust, verified against the [official AWS session-manager-plugin](https://github.com/aws/session-manager-plugin) source code.

### Wire Format (exact match)

- 116-byte header + 4-byte PayloadLength + variable payload
- Big-endian encoding, 32-byte space-padded MessageType, SHA-256 payload digest
- UUID byte-swapping (low 8 bytes first on wire), SchemaVersion = 1
- All field offsets match: HL:0, MT:4, SV:36, CD:40, SN:48, FL:56, MID:64, PD:80, PT:112, PL:116

### Protocol Messages (exact match)

- Message types: `input_stream_data`, `output_stream_data`, `acknowledge`, `channel_closed`, `pause_publication`, `start_publication`
- Payload types: Output(1), Error(2), HandshakeRequest(5), HandshakeResponse(6), HandshakeComplete(7), Flag(10)
- Flag values: DisconnectToPort(1), TerminateSession(2), ConnectToPortError(3)
- StreamDataPayloadSize: 1024 bytes

### Handshake (exact match)

- Sends `OpenDataChannelInput` as initial WebSocket text message (MessageSchemaVersion "1.0")
- Processes HandshakeRequest → sends HandshakeResponse (ActionStatus Success=1)
- Waits for HandshakeComplete, acknowledges all `output_stream_data` messages
- Sequence numbers carry over from handshake into data forwarding

### Retransmission (exact match)

- Jacobson/Karels RTT estimation (RFC 6298): SRTT, RTTVAR, clock granularity 10ms
- Default RTT 100ms, default RTO 200ms, max RTO 1000ms
- Retransmission check every 100ms, max 3000 attempts
- Karn's algorithm: only first-transmission samples update RTT
- BTreeMap-based outgoing buffer with overflow protection (10,000 max)

### Improvements Over Official Plugin

| Feature | Official | ConnectionApp |
|---|---|---|
| Payload digest validation | Not validated | SHA-256 verified, corrupted messages dropped |
| Dead connection detection | Relies on read errors | Pong watchdog (3 missed = dead after 90s) |
| WebSocket ping interval | 5 minutes | 30 seconds (prevents Linux conntrack timeout) |
| SSM-level keepalive | None (relies on WS ping) | No-op ACK every 5 min (prevents SSM idle timeout) |
| SO_REUSEADDR | Not set | Set (prevents port rebind failures on Linux) |
| TCP_NODELAY | Not set | Set (critical for DB protocol small packets) |
| TCP keepalive | Not set | Enabled (60s/10s, detects dead connections) |
| Dual-stack binding | IPv4 only | IPv4 + IPv6 (supports clients resolving localhost to ::1) |

### Intentional Differences

- **No smux multiplexing** — reports `client_version: "1.0.0.0"` to force basic mode (single TCP connection per tunnel). Multiple connections use multiple tunnels, each with its own port.
- **Manager-level reconnection** — instead of WebSocket-level reconnection with `ResumeSession` API, the tunnel manager performs full reconnection (re-validates bastion/target, new SSM session). Slightly longer recovery but more robust.

## Publishing

- **Desktop**: Multi-platform builds (macOS ARM64/x64, Linux ARM64/x64, Windows x64) via `tauri-action` on git tags
- **Homebrew**: Auto-updated tap via GitHub Actions

## License

MIT
