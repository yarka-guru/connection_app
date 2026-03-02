# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rds_ssm_connect** is a Node.js CLI tool and Tauri desktop app that enables secure connections to AWS RDS databases through AWS Systems Manager (SSM) port forwarding via bastion hosts. Projects are user-configurable — the tool reads AWS profiles from `~/.aws/config`, loads project definitions from `~/.rds-ssm-connect/projects.json`, retrieves database credentials from AWS Secrets Manager, and sets up port forwarding through a bastion instance.

## Key Architecture

### Entry Point
- `connect.js` - Main executable (shebang: `#!/usr/bin/env node`)
  - Reads AWS profiles from `~/.aws/config`
  - Loads project configs from `~/.rds-ssm-connect/projects.json` via `configLoader.js`
  - Uses `inquirer` for interactive project/environment selection
  - Includes first-run wizard when no projects are configured
  - Uses AWS SDK v3 directly (STS, EC2, RDS, SSM, Secrets Manager, SSO OIDC)
  - Ensures SSO session is valid before connecting (via `src/sso-login.js`)
  - Establishes SSM port forwarding session with keepalive and auto-reconnect

### Configuration
- `configLoader.js` - Project config CRUD for `~/.rds-ssm-connect/projects.json`
  - `loadProjectConfigs()` / `saveProjectConfig()` / `deleteProjectConfig()`
  - `validateProjectConfig()` - validates required fields, region format, port format, shell-safe patterns
  - Each project defines: name, region, database, secretPrefix, rdsType, engine, rdsPattern, profileFilter, envPortMapping, defaultPort

### GUI Adapter
- `gui-adapter.js` - JSON stdin/stdout IPC bridge for Tauri sidecar
  - Commands: `list-projects`, `list-profiles`, `connect`, `disconnect`, `disconnect-all`, `status`, `list-project-configs`, `save-project-config`, `delete-project-config`, `sso-login`, `ping`
  - Manages multiple simultaneous connections with strict port availability checks
  - SSO pre-flight validation before connecting

### Backend Modules (src/)
- `aws-clients.js` - AWS SDK client factory (STS, EC2, RDS, SSM, Secrets Manager)
- `aws-operations.js` - AWS operations (find bastion, get endpoint, get credentials, start session, etc.)
- `credential-resolver.js` - AWS credential chain resolution (SSO, profiles)
- `sso-login.js` - AWS SSO OIDC device authorization flow
- `plugin-resolver.js` - Locates session-manager-plugin binary

### Tauri Desktop App
- `src-tauri/src/lib.rs` - Tauri commands (connect, disconnect, saved connections CRUD, project config CRUD, AWS profile CRUD, updates, prerequisites check)
- `src-tauri/tauri.conf.json` - App config, plugins, window settings, bundling
- Session Manager Plugin is bundled as an external binary

### Frontend (src/)
- `App.svelte` - Main app shell (Svelte 5 with runes)
- `lib/ConnectionForm.svelte` - Project/environment selector + connect button
- `lib/ActiveConnections.svelte` - Live connection panels with credentials
- `lib/SavedConnections.svelte` - Bookmarked connections list
- `lib/Settings.svelte` - Project management + AWS profile CRUD + raw config editor
- `lib/SessionStatus.svelte` - Connection status indicator
- `lib/PrerequisitesCheck.svelte` - Missing dependency warnings
- `lib/UpdateBanner.svelte` - In-app update notification
- `lib/CopyButton.svelte` - Reusable copy-to-clipboard with feedback
- `lib/ConfirmDialog.svelte` - Reusable confirmation modal
- `lib/utils.js` - Shared utilities (clipboard, timeout, focus trap)

### Core Flow
1. Read AWS profiles from `~/.aws/config`
2. Load project configs from `~/.rds-ssm-connect/projects.json`
3. Prompt user to select project (filtered by available profiles)
4. Filter and prompt for environment (AWS profile) based on project's `profileFilter`
5. Ensure SSO session is valid (OIDC device authorization if needed)
6. Query Secrets Manager for RDS credentials (project-specific `secretPrefix`)
7. Find running bastion instance (tagged with `Name=*bastion*`)
8. Get RDS endpoint (cluster or instance based on project's `rdsType`)
9. Start SSM port forwarding session with correct local/remote ports
10. Display connection details for database client

## Development Commands

### Installation
```bash
npm install
```

### Testing
```bash
npm test
```
Tests cover:
- Project config loading, validation, and CRUD (`configLoader.test.js`)
- AWS operations (`aws-operations.test.js`)
- Credential resolution (`credential-resolver.test.js`)
- SSO login flow (`sso-login.test.js`)
- Plugin resolution (`plugin-resolver.test.js`)
- Port mapping and config parsing (`connect.test.js`)

### Local Testing
```bash
node connect.js
```

### Desktop App Development
```bash
npm run dev:gui       # Tauri dev mode (full app)
npm run build:gui     # Build Tauri desktop app
```

### Global Installation (for testing as installed package)
```bash
npm install -g .
rds_ssm_connect
```

### Publishing
- **npm**: Published via GitHub Actions workflow (`.github/workflows/npm-publish.yml`) when a release is created
- **Desktop**: Multi-platform builds (macOS ARM64/x64, Linux ARM64/x64, Windows x64) via `tauri-action` on git tags

## Prerequisites for Running

- AWS CLI - Required for AWS API calls
- Node.js (ES modules enabled via `"type": "module"` in package.json)
- Properly configured `~/.aws/config` with named profiles
- Session Manager Plugin (bundled in desktop app; CLI requires separate install)

## AWS Resource Naming Conventions

The application relies on AWS resource naming patterns defined per project in `~/.rds-ssm-connect/projects.json`:

- **Secrets**: Must start with the project's `secretPrefix` (e.g., `rds!cluster`, `rds!db`)
- **Bastion instances**: Must be tagged with `Name=*bastion*` and in `running` state
- **RDS clusters**: DBClusterIdentifier must match the project's `rdsPattern` and be `available` (when `rdsType` is `cluster`)
- **RDS instances**: DBInstanceIdentifier must match the project's `rdsPattern` and be `available` (when `rdsType` is `instance`)

## Important Notes

- Projects are user-configurable via `~/.rds-ssm-connect/projects.json` (no hardcoded defaults)
- Port assignment is based on project-specific `envPortMapping` with `defaultPort` fallback
- The tool keeps the SSM session running until Ctrl+C is pressed
- AWS region is determined by the selected project's `region` field
- SSO sessions are validated before connecting; browser opens automatically if needed
- Desktop app bundles Session Manager Plugin as an external binary

## Error Handling & Recovery

### TargetNotConnected Recovery
The application automatically handles the race condition where a bastion instance appears running but SSM agent is not connected:

1. Detects `TargetNotConnected` error (exit code 254)
2. Terminates the disconnected bastion instance
3. Waits for ASG to spin up a new instance (max 20 retries @ 15s intervals)
4. Verifies SSM agent is online using `describe-instance-information`
5. Retries port forwarding with new instance (max 2 retries)

### Auto-Reconnect
When an established session drops unexpectedly (idle timeout, network issue):

1. Verifies AWS credentials are still valid (avoids opening SSO tabs when user is away)
2. Re-discovers infrastructure (bastion may have been replaced by ASG)
3. Reconnects on the same local port (up to 3 attempts with 3s delay)

### Keepalive
Periodic TCP pings every 4 minutes prevent SSM from timing out idle connections.

### Configuration Constants
All retry/timeout values are configurable in `RETRY_CONFIG` (`connect.js`):
- `BASTION_WAIT_MAX_RETRIES`: 20 (time to wait for new instance)
- `BASTION_WAIT_RETRY_DELAY_MS`: 15000 (delay between instance checks)
- `PORT_FORWARDING_MAX_RETRIES`: 2 (connection retry attempts)
- `SSM_AGENT_READY_WAIT_MS`: 10000 (stabilization time after agent online)
- `KEEPALIVE_INTERVAL_MS`: 240000 (TCP ping interval, 4 minutes)
- `AUTO_RECONNECT_MAX_RETRIES`: 3 (auto-reconnect attempts)
- `AUTO_RECONNECT_DELAY_MS`: 3000 (delay between reconnect attempts)
- `CREDENTIAL_CHECK_TIMEOUT_MS`: 60000 (credential validation timeout)

### Process Cleanup
The application registers handlers for `SIGINT`, `SIGTERM`, and `exit` events to properly clean up child processes (SSM sessions) using a three-strategy kill approach (process group, individual SIGTERM, SIGKILL) to prevent zombie processes.
