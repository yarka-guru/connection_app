# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rds_ssm_connect** is a Node.js CLI tool that enables secure connections to AWS RDS databases through AWS Systems Manager (SSM) port forwarding via bastion hosts. The tool reads AWS profiles from the user's config, retrieves database credentials from AWS Secrets Manager, and automatically sets up port forwarding through a bastion instance.

## Key Architecture

### Entry Point
- `connect.js` - Main executable (shebang: `#!/usr/bin/env node`)
  - Reads AWS profiles from `~/.aws/config`
  - Uses `inquirer` for interactive environment selection
  - Executes AWS commands via `aws-vault` wrapper
  - Establishes SSM port forwarding session

### Configuration
- `envPortMapping.js` - Environment-to-port mapping configuration
  - Maps environment suffixes (dev, stage, prod, team1-5, qa1-5, etc.) to local port numbers (5433-5452)
  - Defines default database name: `emr`
  - Defines AWS region: `us-east-2`
  - **Critical**: When adding new environments, update this mapping to assign unique ports

### Core Flow
1. Read AWS profiles from `~/.aws/config`
2. Prompt user to select environment (AWS profile)
3. Query Secrets Manager for RDS credentials (secrets starting with `rds!cluster`)
4. Find running bastion instance (tagged with `Name=*bastion*`)
5. Get RDS Aurora cluster endpoint (cluster IDs ending with `-rds-aurora`)
6. Start SSM port forwarding session
7. Display connection details for database client

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
- AWS config parsing
- Port mapping logic
- Credentials validation
- Retry configuration validation

### Local Testing
```bash
node connect.js
```

### Global Installation (for testing as installed package)
```bash
npm install -g .
rds_ssm_connect
```

### Publishing
Package is published to npm via GitHub Actions workflow (`.github/workflows/npm-publish.yml`) when a release is created.

## Prerequisites for Running

- `aws-vault` - Required for AWS credential management
- AWS CLI - Required for AWS API calls
- Node.js (ES modules enabled via `"type": "module"` in package.json)
- Properly configured `~/.aws/config` with named profiles

## AWS Resource Naming Conventions

The application relies on specific AWS resource naming patterns:
- **Secrets**: Must start with `rds!cluster`
- **Bastion instances**: Must be tagged with `Name=*bastion*` and in `running` state
- **RDS clusters**: DBClusterIdentifier must end with `-rds-aurora` and be in `available` state

## Important Notes

- Port assignment is based on longest-matching environment suffix (e.g., `my-team-dev` matches `dev` → port 5433)
- The tool keeps the SSM session running until Ctrl+C is pressed
- Default port is 5432 if no environment suffix matches
- All AWS operations use the `us-east-2` region by default

## Error Handling & Recovery

### TargetNotConnected Recovery
The application automatically handles the race condition where a bastion instance appears running but SSM agent is not connected:

1. Detects `TargetNotConnected` error (exit code 254)
2. Terminates the disconnected bastion instance
3. Waits for ASG to spin up a new instance (max 20 retries @ 15s intervals)
4. Verifies SSM agent is online using `describe-instance-information`
5. Retries port forwarding with new instance (max 2 retries)

### Configuration Constants
All retry/timeout values are configurable in `RETRY_CONFIG`:
- `BASTION_WAIT_MAX_RETRIES`: 20 (time to wait for new instance)
- `BASTION_WAIT_RETRY_DELAY_MS`: 15000 (delay between instance checks)
- `PORT_FORWARDING_MAX_RETRIES`: 2 (connection retry attempts)
- `SSM_AGENT_READY_WAIT_MS`: 10000 (stabilization time after agent online)

### Process Cleanup
The application registers handlers for `SIGINT`, `SIGTERM`, and `exit` events to properly clean up child processes (SSM sessions) to prevent zombie processes.