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

The Homebrew tap ships **both** the desktop app and the `connection-app-cli`
companion binary in a single command — the cask (macOS) and the Linux formula
both depend on `connection-app-cli`.

### macOS (Homebrew) — GUI + CLI

```bash
brew tap yarka-guru/tap
brew install --cask connection-app
```

This installs `ConnectionApp.app` and puts `connection-app-cli` on your `$PATH`.

Or download the `.dmg` directly from [GitHub Releases](https://github.com/yarka-guru/connection_app/releases).

> **Signed and notarised**
>
> macOS builds are signed with an Apple Developer ID and notarised by Apple,
> so they open without Gatekeeper warnings. No `xattr` workaround is needed.

### Linux (Homebrew) — GUI + CLI

```bash
# Install Homebrew if not already installed
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
echo 'eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"' >> ~/.bashrc
eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"

# Install the app (also installs connection-app-cli automatically)
brew tap yarka-guru/tap
brew install yarka-guru/tap/connection-app

# Make brew tools visible to the desktop app
echo 'export PATH="/home/linuxbrew/.linuxbrew/bin:$PATH"' | sudo tee /etc/profile.d/linuxbrew.sh
# Log out and back in for this to take effect
```

### CLI only (any platform via Homebrew)

If you only need the terminal tool (no GUI):

```bash
brew tap yarka-guru/tap
brew install connection-app-cli
```

Pre-built binaries are also published as GitHub Release assets per target:
`connection-app-cli-{aarch64,x86_64}-{apple-darwin,unknown-linux-gnu}.tar.gz`.

### Linux — direct `.deb`

```bash
# Replace VERSION with the latest tag (e.g. 3.6.1) from GitHub Releases.
VERSION=3.6.1
ARCH=arm64   # or amd64
wget "https://github.com/yarka-guru/connection_app/releases/download/v${VERSION}/ConnectionApp_${VERSION}_${ARCH}.deb"
sudo dpkg -i "ConnectionApp_${VERSION}_${ARCH}.deb"
```

The `.deb` ships the desktop app only; install `connection-app-cli` separately
via Homebrew or the per-target tarball asset above.

## Usage

### Desktop App

Launch the app, select a project and environment, then click **Connect**. Connection credentials are displayed inline with one-click copy buttons. Save connections for quick access later.

Manage projects and AWS profiles in **Settings** (`Cmd/Ctrl + ,`).

### CLI

`connection-app-cli` is shipped via Homebrew alongside the desktop app, or it
can be installed standalone (see [Installation](#cli-only-any-platform-via-homebrew)).
You can also build from source:

```bash
cd src-tauri
cargo install --path . --no-default-features --bin connection-app-cli
```

Then run it:

```bash
connection-app-cli
```

1. On first run with no projects configured, create one in `~/.connection-app/projects.json`
2. Select a project
3. Select an environment (resolved to an AWS profile by account ID; the list shows `prod — <profile> (<account>)`, or `no profile`)
4. SSO session is validated automatically (opens browser if needed)
5. The tool retrieves credentials from Secrets Manager, finds a bastion instance, and starts native WebSocket port forwarding
6. Use the displayed connection details with your database client (`psql`, `mysql`, pgAdmin, DBeaver, etc.)

The tunnel stays open until you press `Ctrl+C`.

#### CLI Options

```
connection-app-cli [OPTIONS] [COMMAND]

Options:
  -p, --project <NAME>    Project name (skip interactive selection)
  -e, --env <NAME>        Environment name (skip interactive selection)
      --profile <NAME>    AWS profile name; must reach the environment's account
      --port <PORT>       Local port override
      --debug             Enable debug logging (RUST_LOG levels)
  -V, --version           Print version
  -h, --help              Print help

Commands:
  projects    List configured projects
  profiles    List AWS profiles
```

`--profile` without `--env` picks the environment whose account the profile
reaches; if it reaches several environments of the project, pass `--env`.

## How It Works

1. Reads AWS profiles from `~/.aws/config`
2. Loads project configurations from `~/.connection-app/projects.json`
3. Resolves the selected environment to an AWS profile: the profile whose account ID equals the environment's `accountId`
4. Ensures AWS SSO session is valid (OIDC device authorization if needed), then checks the credentials belong to the environment's account
5. Queries AWS Secrets Manager for RDS credentials (project-specific `secretPrefix`;
   if no secret matches, the secret named `rds/<DB instance or cluster identifier>-credentials`)
6. Finds a running bastion instance (tagged `Name=*bastion*`)
7. Gets the RDS endpoint (cluster or instance depending on project `rdsType`)
8. Starts an SSM session and opens a native WebSocket tunnel for port forwarding
9. Displays connection details (host, port, username, password, database)

### PostgreSQL connections: the local auth proxy

For PostgreSQL RDS connections the app runs a small PostgreSQL proxy on the
local port. Your database client signs in to the proxy; the proxy signs in to
RDS over a verified TLS connection (AWS RDS CA bundle, built in) and then
relays the session untouched.

- **Password**: the app shows a stable local password (it never changes, so
  clients can store it). For `secrets` projects the secret's current password
  is accepted too, so clients that already store it keep working. For
  `authType: "iam"` projects only the local password is accepted.
- **Upstream sign-in** is picked automatically: an IAM token when the database
  user is IAM-enabled (RDS asks for PAM), otherwise the secret's password. IAM
  tokens are cached and replaced after 10 of their 15 minutes, so new
  connections never hit an expired token. If RDS rejects the secret's
  password, the secret is read again (at most once a minute) and the sign-in
  retried.
- **SSL on the local hop**: clients that ask for TLS get it with a self-signed
  certificate for `localhost` / `127.0.0.1` / `::1`, generated in memory once
  per app run and never written to disk. TLS to RDS is always verified by the
  proxy.

| Client `sslmode` | Before (raw tunnel to RDS) | Now (auth proxy) |
|---|---|---|
| `disable` | refused by RDS (`rds.force_ssl`: "no pg_hba.conf entry … no encryption") | works (plaintext on loopback, TLS to RDS) |
| `prefer` (libpq/pgjdbc default) | TLS to RDS | TLS on loopback, TLS to RDS |
| `require` (DBeaver/DataGrip "SSL on") | TLS to RDS | TLS on loopback, TLS to RDS |
| `verify-ca` with the RDS CA bundle | worked | **fails**: the loopback certificate is self-signed |
| `verify-full`, `host=localhost` | failed (the certificate names the RDS endpoint) | fails |
| `verify-full`, `host=<endpoint> hostaddr=127.0.0.1` | worked | **fails** |
| pgjdbc `ssl=true` alone (means `verify-full`) | failed without `~/.postgresql/root.crt` | same |
| `channel_binding=require` | worked for password users (RDS offers `SCRAM-SHA-256-PLUS`) | **fails**: the proxy offers only `SCRAM-SHA-256`, without channel binding |
| `sslnegotiation=direct` (libpq 17+) | worked (RDS PostgreSQL 17+ accepts direct TLS) | **fails**: the proxy expects an SSLRequest first |

Use `prefer`, `require` or `disable` against the local port. libpq treats
`require` as `verify-ca` when a root certificate is configured (`sslrootcert`
or `~/.postgresql/root.crt`); drop it for these connections. MySQL projects
still use the raw tunnel.

**Hardening tip.** While the app is not running, anything can listen on the
local port. With libpq 16+ add `require_auth=scram-sha-256` to your connection
settings: the client then refuses to send its password to a server that does
not use SCRAM, so a port squatter cannot collect it (the proxy always uses
SCRAM).

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
| `secretPrefix` | Secrets Manager prefix; if nothing matches, `rds/<identifier>-credentials` is used | `"rds!cluster"` |
| `rdsType` | `"cluster"` or `"instance"` | `"cluster"` |
| `engine` | `"postgres"` or `"mysql"` | `"postgres"` |
| `rdsPattern` | RDS identifier pattern | `"my-app-rds-aurora"` |
| `authType` | `"secrets"` or `"iam"` | `"secrets"` |
| `passwordOnly` | PostgreSQL only: `true` signs in to RDS with the secret's password only | `false` |
| `environments` | Ordered list of environments, see below | |
| `defaultPort` | Suggested port for a new environment | `"5432"` |
| `profileFilter`, `envPortMapping` | Legacy. Ignored when `environments` is present, kept in the file for 3.7.x | |

### Environments

An environment is an AWS account, not an AWS profile name, so the same file
works for everyone whatever their profiles are called.

| Field | Description | Example |
|---|---|---|
| `name` | Unique within the project, shell-safe | `"prod"` |
| `accountId` | 12-digit AWS account ID, stored as a string | `"073813807522"` |
| `port` | Local port, digits only, unique within the project | `"5460"` |
| `rdsPattern`, `region` | Optional overrides for this environment | `null` |
| `targetPattern`, `ecsCluster`, `ecsService`, `bastionPattern` | Optional overrides for service projects | `null` |

The app finds the profile for an account in `~/.aws/config` (`sso_account_id`,
or the account in `role_arn`). For a profile whose config does not name its
account (static keys, `credential_process`, a `source_profile` chain without
`role_arn`), click **Detect account** on the profile in Settings → AWS
Profiles, or on its "account unknown" row in the "Config needs updating"
dialog: the app calls `sts:GetCallerIdentity` with that profile and caches the
account in `~/.connection-app/profile-accounts.json` until the profile's
section changes. With one match it is
used. With several, the first is used until you pick one in the connection
form; the choice is stored per account in `~/.connection-app/preferences.json`.
With none, the environment is greyed out and offers "Create profile". Before
the tunnel opens, the account of the credentials must equal `accountId`,
otherwise the connection is refused. This check is skipped for legacy
(unmigrated) projects, whose environments are profile names with no
authoritative account; it applies once the project is migrated.
`preferences.json` and `profile-accounts.json` are local and never exported.

### Authentication

For PostgreSQL the project form offers three choices:

- **Automatic** (default for new projects): the DB user is the secret's
  username, and the local auth proxy uses an IAM token or the secret's
  password, whichever the database asks for. With no secret configured
  (empty `secretPrefix` and no `secretPath`), it signs in as **IAM DB User**
  (`iamUsername`) with an IAM token, exactly like IAM only. A configured secret
  that is not found fails the connect; it never falls back to `iamUsername`.
- **IAM only**: `authType: "iam"`.
- **Password only**: `authType: "secrets"` with `passwordOnly: true`.

MySQL offers IAM or Password.

### Loading, repair and migration

- `projects.json` is read project by project. A broken project is shown with
  its errors (Edit, Edit JSON, Delete in Settings) and does not hide the others.
  If the file is not valid JSON, Settings shows the line and column, a raw
  editor and Restore backup.
- Projects without `environments` are legacy. On first start a "Config needs
  updating" dialog proposes environments by account, with ports kept as they
  are today. Apply copies `projects.json` to `projects.json.bak-YYYYMMDD-HHMMSS`
  first. "Later" leaves the file alone and legacy projects keep working.
  Import creates the same kind of backup before writing.
- A migrated file still works in 3.7.x because the legacy fields are kept.
  Projects created in 3.8 carry `profileFilter: "__connection-app-3.8__"`, so
  3.7.x hides them. The CLI never writes `projects.json`; legacy
  projects keep resolving by profile name, and it prints a note to open the
  app to update them.

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
    "authType": "secrets",
    "passwordOnly": false,
    "profileFilter": "__connection-app-3.8__",
    "envPortMapping": {},
    "defaultPort": "5432",
    "environments": [
      { "name": "prod", "accountId": "111122223333", "port": "5432" },
      { "name": "staging", "accountId": "444455556666", "port": "5433" }
    ]
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
    project_file.rs   Per-project loading, atomic writes, backups
    accounts.rs       AWS account of each profile
    environments.rs   Environment to profile resolution
    migration.rs      Legacy to account-environment migration
    preferences.rs    Local per-account profile choices
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
    repair.rs         Project file repair, migration and import commands
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
    EnvironmentsEditor.svelte  Environment table in the project form
    ProjectFileRepair.svelte   Broken project / broken file actions
    MigrationDialog.svelte     Legacy config migration
    ImportDialog.svelte        Import with per-project conflict choice
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

- **Smux multiplexing on by default** — reports `client_version: "1.2.0.0"`; when the agent supports smux (>= 3.0.196.0), multiple TCP connections share one tunnel, each on its own smux stream. Basic mode (single TCP connection at a time) is used for older agents or when a project sets `"multiplexed": false`.
- **Smux protocol v1, strictly** — matching the agent and the official plugin (`smux.DefaultConfig`). v1 has no wire-level flow control: `cmdUPD` window updates are v2-only, and xtaci/smux closes the whole session (`ErrInvalidProtocol`) if a v1 peer receives one. The client never sends `cmdUPD` and ignores any received; backpressure relies on the bounded channels along the data path.
- **Manager-level reconnection** — instead of WebSocket-level reconnection with `ResumeSession` API, the tunnel manager performs full reconnection (re-validates bastion/target, new SSM session). Slightly longer recovery but more robust.

## Publishing

- **Desktop**: Multi-platform builds (macOS ARM64/x64, Linux ARM64/x64) via `tauri-action` on git tags. macOS builds are signed with a Developer ID and notarised by Apple
- **CLI**: Per-target binaries for the same four platforms, built with `--no-default-features` and uploaded as `connection-app-cli-{target}.tar.gz` release assets
- **Homebrew**: Auto-updated tap via GitHub Actions — publishes the macOS cask, the Linux GUI formula, and a cross-platform `connection-app-cli` formula. The cask and Linux formula `depends_on` the CLI formula so a single `brew install` brings both

## License

MIT
