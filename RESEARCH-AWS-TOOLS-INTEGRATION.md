# Research: Integrating AWS CLI, aws-vault, and session-manager-plugin

## Executive Summary

**Yes, it is possible to eliminate external dependencies on AWS CLI and aws-vault entirely, and to reduce (but not fully eliminate) the dependency on session-manager-plugin.** Here is a detailed analysis of each tool, integration strategies, and a phased implementation plan that accounts for all edge cases including cross-platform support, SSO vs credential-based profiles, and tool availability detection.

---

## 1. Tool-by-Tool Analysis

### 1.1 AWS CLI — **FULLY REPLACEABLE**

| Attribute | Details |
|-----------|---------|
| **License** | Apache 2.0 (permissive, allows integration) |
| **Written in** | Python |
| **Current usage** | 8 distinct API call types, all shelled out via `execAsync` |
| **Can replace?** | **YES — 100% replaceable with @aws-sdk v3** |

The application already lists `@aws-sdk/client-ec2`, `@aws-sdk/client-rds`, and `@aws-sdk/client-ssm` in `package.json` but doesn't use them. Every AWS CLI command currently used has a direct SDK equivalent:

| Current CLI Command | SDK Package | SDK Command |
|---|---|---|
| `aws sts get-caller-identity` | `@aws-sdk/client-sts` | `GetCallerIdentityCommand` |
| `aws secretsmanager list-secrets` | `@aws-sdk/client-secrets-manager` | `ListSecretsCommand` |
| `aws secretsmanager get-secret-value` | `@aws-sdk/client-secrets-manager` | `GetSecretValueCommand` |
| `aws ec2 describe-instances` | `@aws-sdk/client-ec2` | `DescribeInstancesCommand` |
| `aws ec2 terminate-instances` | `@aws-sdk/client-ec2` | `TerminateInstancesCommand` |
| `aws rds describe-db-clusters` | `@aws-sdk/client-rds` | `DescribeDBClustersCommand` |
| `aws rds describe-db-instances` | `@aws-sdk/client-rds` | `DescribeDBInstancesCommand` |
| `aws ssm describe-instance-information` | `@aws-sdk/client-ssm` | `DescribeInstanceInformationCommand` |
| `aws ssm start-session` | `@aws-sdk/client-ssm` | `StartSessionCommand` (partial — see Section 1.3) |

**Benefits of migration:**
- Eliminates Python/AWS CLI runtime dependency
- Structured responses (no JMESPath/text parsing)
- Typed exceptions for better error handling
- Eliminates shell injection surface (currently mitigated with regex patterns)
- Better performance (no subprocess spawn per API call)

**New dependency needed:** `@aws-sdk/client-secrets-manager` (not currently in package.json)

---

### 1.2 aws-vault — **FULLY REPLACEABLE**

| Attribute | Details |
|-----------|---------|
| **License** | MIT (permissive, allows integration) |
| **Written in** | Go |
| **Status** | Abandoned by 99designs (active fork at ByteNess/aws-vault) |
| **Current usage** | Credential wrapper: `aws-vault exec ${profile} -- aws ...` |
| **Can replace?** | **YES — with @aws-sdk credential providers** |

aws-vault's core function is reading `~/.aws/config`, resolving credentials (including SSO, MFA, and assume-role chains), and injecting them as environment variables. The AWS SDK for JavaScript v3 has built-in credential providers that replicate all of this functionality natively:

#### Credential Provider Mapping

| aws-vault Feature | SDK Equivalent | Package |
|---|---|---|
| Read `~/.aws/config` profiles | `fromIni()` | `@aws-sdk/credential-providers` |
| AWS SSO / Identity Center | `fromSSO()` | `@aws-sdk/credential-provider-sso` |
| Assume role with MFA | `fromIni({ mfaCodeProvider })` | `@aws-sdk/credential-providers` |
| Assume role chain | `fromIni()` (auto-resolved) | `@aws-sdk/credential-providers` |
| Environment variables | `fromEnv()` | `@aws-sdk/credential-providers` |
| Instance metadata (EC2) | `fromInstanceMetadata()` | `@aws-sdk/credential-providers` |
| Credential caching | `fromTemporaryCredentials()` | `@aws-sdk/credential-providers` |
| Default chain (all of above) | `fromNodeProviderChain()` | `@aws-sdk/credential-providers` |

#### Key Implementation Details

**MFA Handling:**
```javascript
import { fromIni } from '@aws-sdk/credential-providers';
import { input } from '@inquirer/prompts';

const credentials = fromIni({
  profile: selectedProfile,
  mfaCodeProvider: async (serialArn) => {
    return await input({ message: `Enter MFA code for ${serialArn}:` });
  }
});
```

**SSO Handling:**
```javascript
import { fromSSO } from '@aws-sdk/credential-providers';

const credentials = fromSSO({ profile: selectedProfile });
// User must have run `aws sso login --profile <name>` first
// Or we can trigger `SSOOIDCClient.createToken()` flow programmatically
```

**Universal Default Chain:**
```javascript
import { fromNodeProviderChain } from '@aws-sdk/credential-providers';

// Automatically resolves: env vars → SSO → INI file → process → instance metadata
const credentials = fromNodeProviderChain({ profile: selectedProfile });
```

#### Edge Cases for Credential Resolution

| Scenario | How to Handle |
|----------|--------------|
| **SSO profile** (sso_start_url defined) | `fromSSO()` reads cached token from `~/.aws/sso/cache/`. If expired, prompt user to run `aws sso login` or trigger OIDC device auth flow. |
| **MFA + AssumeRole** (mfa_serial defined) | `fromIni()` with `mfaCodeProvider` callback prompts for MFA code interactively. |
| **Simple credentials** (access_key in file) | `fromIni()` reads directly from `~/.aws/credentials`. |
| **Environment variables** (CI/CD) | `fromEnv()` reads `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`. |
| **aws-vault still installed** | Can use `fromProcess()` calling `aws-vault exec <profile> -- env` as a fallback. |
| **Chained roles** (source_profile → role_arn) | `fromIni()` handles recursively, including multi-hop chains. |

---

### 1.3 session-manager-plugin — **PARTIALLY REPLACEABLE** (with significant effort)

| Attribute | Details |
|-----------|---------|
| **License** | Apache 2.0 (permissive, allows integration/bundling) |
| **Written in** | Go |
| **Current usage** | Auto-invoked by `aws ssm start-session` for WebSocket tunnel |
| **Can replace?** | **PARTIAL — see options below** |

This is the most complex component. The session-manager-plugin implements a custom binary protocol over WebSockets for port forwarding. Here's the full picture:

#### What the Plugin Does

1. Receives `StreamUrl` + `TokenValue` from `StartSession` API
2. Opens WebSocket connection to `wss://ssmmessages.<region>.amazonaws.com`
3. Authenticates via JSON token message
4. Switches to custom **binary protocol** (not JSON)
5. Performs handshake with SSM agent on EC2 instance
6. Opens local TCP listener on `localPortNumber`
7. Forwards bidirectional traffic: local TCP ↔ WebSocket ↔ SSM agent ↔ remote host:port
8. Handles keepalives, acknowledgments, sequencing, and optional smux multiplexing

#### The SSM Binary Protocol

The protocol is defined in [aws/amazon-ssm-agent/agent/session/contracts/agentmessage.go](https://github.com/aws/amazon-ssm-agent/blob/mainline/agent/session/contracts/agentmessage.go):

```
Binary message layout (per AgentMessage):
┌──────────────────────────────┐
│ HeaderLength      (4 bytes)  │
│ MessageType      (32 bytes)  │  UTF-8 string
│ SchemaVersion     (4 bytes)  │
│ CreatedDate       (8 bytes)  │  Epoch millis
│ SequenceNumber    (8 bytes)  │
│ Flags             (8 bytes)  │  SYN=bit0, FIN=bit1
│ MessageId        (40 bytes)  │  UUID as UTF-8
│ PayloadDigest    (32 bytes)  │  SHA-256
│ PayloadType       (4 bytes)  │
│ PayloadLength     (4 bytes)  │
│ Payload      (variable len)  │
└──────────────────────────────┘
```

Port forwarding adds a handshake phase (request → response → complete) and optionally smux multiplexing for concurrent connections.

#### Integration Options

##### Option A: Bundle the session-manager-plugin Binary (Recommended for Phase 1)

**Approach:** Download and bundle pre-compiled binaries for each platform.

| Platform | Binary Location | Architecture |
|----------|----------------|-------------|
| macOS x64 | `sessionmanagerplugin-bundle/bin/session-manager-plugin` | x86_64 |
| macOS ARM | Same path | arm64 (Apple Silicon) |
| Linux x64 | `/usr/local/sessionmanagerplugin/bin/session-manager-plugin` | x86_64 |
| Linux ARM | Same path | aarch64 |
| Windows | `C:\Program Files\Amazon\SessionManagerPlugin\bin\session-manager-plugin.exe` | x86_64/arm64 |

**Implementation strategy:**
1. Check if plugin is already installed system-wide
2. If not found, use a bundled binary from `./bin/<platform>-<arch>/session-manager-plugin`
3. Call `StartSessionCommand` via SDK to get `StreamUrl` + `TokenValue`
4. Spawn the bundled plugin binary directly (bypassing AWS CLI):
```javascript
spawn('session-manager-plugin', [
  JSON.stringify(startSessionResponse),  // API response
  region,
  'StartSession',
  '',                                     // profile (empty)
  JSON.stringify(startSessionParams),     // original request
  `https://ssm.${region}.amazonaws.com`   // endpoint
]);
```

**Pros:** Reliable, battle-tested, cross-platform
**Cons:** Binary distribution (~15MB per platform), update maintenance
**License:** Apache 2.0 — bundling is permitted with attribution

##### Option B: Compile session-manager-plugin from Source

**Approach:** Cross-compile the Go source for all target platforms at build time.

```bash
# Example cross-compilation
GOOS=darwin  GOARCH=arm64  go build -o bin/darwin-arm64/session-manager-plugin  ./src/...
GOOS=linux   GOARCH=amd64  go build -o bin/linux-amd64/session-manager-plugin   ./src/...
GOOS=windows GOARCH=amd64  go build -o bin/windows-amd64/session-manager-plugin.exe ./src/...
```

**Pros:** Full control, can pin exact version, reproducible
**Cons:** Requires Go toolchain in CI/CD, increases build complexity
**License:** Apache 2.0 — compiling from source is permitted

##### Option C: Implement SSM Port Forwarding Protocol in Node.js (Future/Advanced)

**Approach:** Pure JavaScript implementation of the SSM WebSocket data channel protocol, including port forwarding.

**Existing references:**
- [`ssm-session` (npm)](https://github.com/bertrandmartel/aws-ssm-session) — Implements shell sessions only, NOT port forwarding. ~348 weekly downloads, unmaintained.
- [`ssm-session-client` (Go)](https://github.com/mmmorris1975/ssm-session-client) — Full Go implementation including `PortForwardingSession()`. Single-stream only (no multiplexing). Could be used as a reference.
- [Formal.ai blog post](https://www.joinformal.com/blog/down-the-rabbit-hole-implementing-ssh-port-forwarding-over-aws-session-manager/) — Documents the protocol in detail, including bugs they found in AWS's implementation.

**What would need to be implemented:**
1. WebSocket connection management (using `ws` npm package)
2. Binary message serialization/deserialization (144-byte header + variable payload)
3. Sequence number tracking and acknowledgment protocol
4. Handshake flow (request → response → complete)
5. Local TCP server (using Node.js `net` module)
6. Bidirectional data forwarding (TCP ↔ WebSocket)
7. Keepalive/heartbeat mechanism
8. Error recovery and reconnection
9. Optional: smux multiplexing (for concurrent connections)

**Estimated complexity:** ~1500-2500 lines of Node.js code
**Pros:** Zero binary dependencies, full control, true single-binary deployment
**Cons:** High implementation effort, must track AWS protocol changes, no official support
**Risk:** AWS could change the protocol without notice (though it's been stable since 2019)

##### Option D: Hybrid — Use Go via WebAssembly (Experimental)

**Approach:** Compile the session-manager-plugin Go code to WebAssembly and run it in Node.js.

**Pros:** Single-language distribution
**Cons:** WASM doesn't have native TCP socket access, would require complex bridging. Not practical for this use case.

**Verdict: Not recommended.**

---

## 2. Cross-Platform & Edge Case Matrix

### 2.1 Platform Detection & Binary Resolution

```
Platform Detection Strategy:
┌────────────────────────────────────────────────────┐
│ process.platform  │ process.arch  │ Binary Suffix   │
├───────────────────┼───────────────┼─────────────────┤
│ 'darwin'          │ 'arm64'       │ darwin-arm64     │
│ 'darwin'          │ 'x64'         │ darwin-x64       │
│ 'linux'           │ 'x64'         │ linux-x64        │
│ 'linux'           │ 'arm64'       │ linux-arm64      │
│ 'win32'           │ 'x64'         │ win32-x64.exe    │
│ 'win32'           │ 'arm64'       │ win32-arm64.exe  │
└────────────────────────────────────────────────────┘
```

### 2.2 Tool Availability Detection

```
For each external tool:
┌─────────────────────────────────────────────────────────────────┐
│ Tool                 │ Detection Method        │ Fallback       │
├──────────────────────┼─────────────────────────┼────────────────┤
│ aws CLI              │ `which aws`             │ Use SDK (built-in) — NO FALLBACK NEEDED │
│ aws-vault            │ `which aws-vault`       │ Use SDK credential providers — NO FALLBACK NEEDED │
│ session-manager-plugin│ `which session-manager-plugin` │ Use bundled binary │
│                      │ or check known paths:   │                │
│                      │  macOS: /usr/local/sessionmanagerplugin/bin/ │  │
│                      │  Linux: /usr/local/sessionmanagerplugin/bin/ │  │
│                      │  Windows: C:\Program Files\Amazon\...       │  │
└─────────────────────────────────────────────────────────────────┘
```

### 2.3 Credential Type Detection

```
Profile Analysis (from ~/.aws/config):
┌──────────────────────────────────────────────────────────────┐
│ Profile Config              │ Auth Type        │ SDK Provider │
├─────────────────────────────┼──────────────────┼──────────────┤
│ sso_start_url defined       │ AWS SSO/IdC      │ fromSSO()    │
│ sso_session defined         │ AWS SSO (new)    │ fromSSO()    │
│ role_arn + mfa_serial       │ AssumeRole+MFA   │ fromIni() with mfaCodeProvider │
│ role_arn (no mfa)           │ AssumeRole       │ fromIni()    │
│ aws_access_key_id in creds  │ Static creds     │ fromIni()    │
│ credential_process defined  │ External process │ fromProcess()│
│ AWS_* env vars set          │ Environment      │ fromEnv()    │
└──────────────────────────────────────────────────────────────┘

Recommended approach: Use fromNodeProviderChain() which tries all of the above
in the correct order automatically.
```

### 2.4 Edge Case Handling

| Edge Case | Current Behavior | New Behavior |
|-----------|-----------------|-------------|
| **aws-vault installed, user prefers it** | Required | Optional — detect and offer as choice |
| **aws-vault NOT installed** | Fatal error | Works natively with SDK credential providers |
| **AWS CLI not installed** | Fatal error | Works — SDK handles all API calls |
| **session-manager-plugin not installed** | Fails at tunnel creation | Auto-detect → use bundled binary |
| **SSO session expired** | aws-vault handles re-auth | Detect `CredentialsProviderError`, prompt user to run `aws sso login` or trigger OIDC device flow |
| **MFA required** | aws-vault prompts | `mfaCodeProvider` callback prompts via inquirer |
| **Profile with chained roles** | aws-vault resolves chain | `fromIni()` resolves chain recursively |
| **Windows PowerShell** | Untested (macOS/Linux only) | Full support with platform-specific binary paths |
| **CI/CD (no interactive TTY)** | Not supported | `fromEnv()` picks up env vars automatically |
| **Apple Silicon (M1/M2/M3)** | Depends on installed binary arch | Detect `process.arch === 'arm64'` and use correct binary |
| **Linux ARM (Graviton)** | Depends on installed binary | Detect and use `linux-arm64` binary |

---

## 3. Phased Implementation Plan

### Phase 1: Replace AWS CLI with SDK (Low risk, high reward)

**Goal:** Eliminate `aws` CLI dependency for all API calls except `start-session`.

**Changes:**
1. Create `src/aws-clients.js` — Factory module that creates SDK clients with proper credentials
2. Create `src/credential-resolver.js` — Unified credential resolution:
   - Parse `~/.aws/config` to detect profile type (SSO, MFA, static, etc.)
   - Use `fromNodeProviderChain()` as default
   - Add `mfaCodeProvider` callback using inquirer
   - Detect if aws-vault is running (check `AWS_VAULT` env var) and use those creds if present
3. Refactor `connect.js` functions to use SDK clients:
   - `checkCredentialsValid()` → `STSClient.send(GetCallerIdentityCommand)`
   - `getConnectionCredentials()` → `SecretsManagerClient.send(ListSecretsCommand)` + `GetSecretValueCommand`
   - `findBastionInstance()` → `EC2Client.send(DescribeInstancesCommand)`
   - `getRdsEndpoint()` / `getRdsPort()` → `RDSClient.send(DescribeDBClustersCommand)` or `DescribeDBInstancesCommand`
   - `terminateBastionInstance()` → `EC2Client.send(TerminateInstancesCommand)`
   - `waitForNewBastionInstance()` → `EC2Client.send(DescribeInstancesCommand)` in loop
   - `waitForSSMAgentReady()` → `SSMClient.send(DescribeInstanceInformationCommand)` in loop
4. Add `@aws-sdk/client-secrets-manager` and `@aws-sdk/client-sts` to dependencies
5. Keep `executePortForwardingCommand()` using shell for now (Phase 2)

**New dependencies:**
- `@aws-sdk/client-secrets-manager`
- `@aws-sdk/client-sts`
- `@aws-sdk/credential-providers` (includes fromIni, fromSSO, fromNodeProviderChain)

**Backward compatibility:**
- If `AWS_VAULT` env var is detected, skip custom credential resolution (already resolved)
- If `aws-vault` is in PATH and user preference is set, offer to use it as wrapper

---

### Phase 2: Replace aws-vault with Native Credential Resolution

**Goal:** Eliminate `aws-vault` as a required dependency.

**Changes:**
1. Implement profile type detection in `credential-resolver.js`:
   ```
   Read ~/.aws/config → parse profile sections → detect auth type → select provider
   ```
2. Add SSO login flow:
   - Detect expired SSO tokens
   - Option A: Prompt user to run `aws sso login` externally
   - Option B: Implement OIDC device authorization flow using `@aws-sdk/client-sso-oidc`
3. Add MFA prompt integration:
   - Detect `mfa_serial` in profile config
   - Prompt using inquirer within the existing interactive flow
4. Update the interactive flow:
   - Currently: Select project → Select profile → aws-vault handles auth → API calls
   - New: Select project → Select profile → Detect auth type → Resolve credentials → API calls
5. Maintain backward compatibility:
   - Detect if running under `aws-vault exec` (check `AWS_VAULT` env var)
   - If yes, use environment credentials directly
   - If no, use SDK credential providers

---

### Phase 3: Direct session-manager-plugin Integration

**Goal:** Eliminate AWS CLI from the `start-session` call, call the plugin binary directly.

**Changes:**
1. Replace `aws-vault exec ${ENV} -- aws ssm start-session ...` with:
   ```javascript
   // Step 1: SDK call
   const session = await ssmClient.send(new StartSessionCommand({
     Target: instanceId,
     DocumentName: 'AWS-StartPortForwardingSessionToRemoteHost',
     Parameters: { host: [rdsEndpoint], portNumber: [remotePort], localPortNumber: [portNumber] }
   }));

   // Step 2: Spawn plugin directly
   const plugin = spawn(pluginBinaryPath, [
     JSON.stringify(session),
     region,
     'StartSession',
     '',
     JSON.stringify(startSessionParams),
     `https://ssm.${region}.amazonaws.com`
   ]);
   ```
2. Implement plugin binary resolution:
   - Check system PATH
   - Check known installation paths per platform
   - Fall back to bundled binary
3. Add plugin availability check at startup with helpful error messages

---

### Phase 4: Bundle session-manager-plugin Binary (Optional)

**Goal:** True zero-external-dependency experience.

**Changes:**
1. Set up CI/CD workflow to download official plugin binaries for all platforms
2. Structure bundled binaries:
   ```
   bin/
   ├── darwin-arm64/session-manager-plugin
   ├── darwin-x64/session-manager-plugin
   ├── linux-arm64/session-manager-plugin
   ├── linux-x64/session-manager-plugin
   ├── win32-arm64/session-manager-plugin.exe
   └── win32-x64/session-manager-plugin.exe
   ```
3. Implement platform-aware binary selection at runtime
4. Add npm postinstall script to set execute permissions on Unix
5. Consider using `optionalDependencies` with platform-specific packages (like esbuild does)

**Alternative packaging strategy (recommended for npm):**
```
@rds-ssm-connect/session-plugin-darwin-arm64
@rds-ssm-connect/session-plugin-darwin-x64
@rds-ssm-connect/session-plugin-linux-x64
@rds-ssm-connect/session-plugin-linux-arm64
@rds-ssm-connect/session-plugin-win32-x64
```
Each package contains only the binary for that platform, listed as `optionalDependencies`.

---

### Phase 5: Pure Node.js SSM Protocol (Future/Advanced)

**Goal:** Eliminate the session-manager-plugin binary entirely.

**Scope:** Implement the SSM WebSocket data channel protocol in pure JavaScript.

**Key modules to implement:**
1. `src/ssm-protocol/connection.js` — WebSocket connection management
2. `src/ssm-protocol/message.js` — Binary message serialization/deserialization
3. `src/ssm-protocol/handshake.js` — Port forwarding handshake flow
4. `src/ssm-protocol/port-forwarder.js` — Local TCP server + bidirectional forwarding
5. `src/ssm-protocol/keepalive.js` — Heartbeat/acknowledgment management

**Reference implementations:**
- [aws/amazon-ssm-agent](https://github.com/aws/amazon-ssm-agent) — Server-side protocol (Go)
- [aws/session-manager-plugin](https://github.com/aws/session-manager-plugin) — Client-side protocol (Go)
- [mmmorris1975/ssm-session-client](https://github.com/mmmorris1975/ssm-session-client) — Independent Go client with port forwarding
- [bertrandmartel/aws-ssm-session](https://github.com/bertrandmartel/aws-ssm-session) — Partial JS implementation (shell only)

**Risk assessment:** Medium-high. The protocol is undocumented by AWS and could change. However, it has been stable since 2019 and the Go source code provides a complete reference. The `mmmorris1975/ssm-session-client` proves that independent implementations work.

---

## 4. Architecture After Full Integration

```
BEFORE (Current):
┌──────────────────────────────────────────────────────────────┐
│  User's Machine Must Have Installed:                         │
│  ✗ Node.js + npm                                             │
│  ✗ aws-vault (Go binary)                                     │
│  ✗ AWS CLI v2 (Python runtime)                               │
│  ✗ session-manager-plugin (Go binary)                        │
│  ✗ Properly configured ~/.aws/config                         │
└──────────────────────────────────────────────────────────────┘

AFTER Phase 3 (Recommended Target):
┌──────────────────────────────────────────────────────────────┐
│  User's Machine Must Have Installed:                         │
│  ✗ Node.js + npm                                             │
│  ✗ session-manager-plugin (Go binary) — auto-detected        │
│  ✗ Properly configured ~/.aws/config                         │
│                                                              │
│  Optional (backward-compatible):                             │
│  ○ aws-vault (if user prefers)                               │
│  ○ AWS CLI (no longer required)                              │
└──────────────────────────────────────────────────────────────┘

AFTER Phase 4 (Zero-dependency Target):
┌──────────────────────────────────────────────────────────────┐
│  User's Machine Must Have Installed:                         │
│  ✗ Node.js + npm (plugin binary bundled)                     │
│  ✗ Properly configured ~/.aws/config                         │
└──────────────────────────────────────────────────────────────┘

AFTER Phase 5 (Pure Node.js — Future):
┌──────────────────────────────────────────────────────────────┐
│  User's Machine Must Have Installed:                         │
│  ✗ Node.js + npm                                             │
│  ✗ Properly configured ~/.aws/config                         │
│  (Everything else is pure JavaScript)                        │
└──────────────────────────────────────────────────────────────┘
```

---

## 5. Risk Assessment & Recommendations

| Risk | Impact | Mitigation |
|------|--------|-----------|
| AWS changes SSM protocol | Phase 5 breaks | Pin to known-good protocol version; Phase 4 (bundled binary) is safer |
| SSO token refresh complexity | User frustration | Start with "prompt user to run `aws sso login`", add OIDC flow later |
| Plugin binary size (~15MB per platform) | Large npm package | Use platform-specific optional dependencies |
| Windows support gaps | Reduced user base | Test early with Windows CI; process management differs significantly |
| Credential caching complexity | Slower auth | Implement in-memory cache with TTL matching STS token duration |

### Recommendation

**Implement Phases 1-3 first.** This eliminates AWS CLI and aws-vault dependencies with moderate effort and low risk. Phase 4 (bundling) is a packaging concern. Phase 5 (pure JS protocol) is high effort but provides the best long-term user experience — consider it as a separate project.

---

## 6. Sources

- [aws-vault (GitHub, MIT License)](https://github.com/99designs/aws-vault)
- [session-manager-plugin (GitHub, Apache 2.0)](https://github.com/aws/session-manager-plugin)
- [AWS CLI (GitHub, Apache 2.0)](https://github.com/aws/aws-cli)
- [@aws-sdk/credential-providers (npm)](https://www.npmjs.com/package/@aws-sdk/credential-providers)
- [@aws-sdk/credential-provider-sso (npm)](https://www.npmjs.com/package/@aws-sdk/credential-provider-sso)
- [ssm-session (npm) — JS SSM protocol](https://github.com/bertrandmartel/aws-ssm-session)
- [ssm-session-client (Go) — Port forwarding](https://github.com/mmmorris1975/ssm-session-client)
- [amazon-ssm-agent — Binary protocol spec](https://github.com/aws/amazon-ssm-agent/blob/mainline/agent/session/contracts/agentmessage.go)
- [SSM StartSession API](https://docs.aws.amazon.com/systems-manager/latest/APIReference/API_StartSession.html)
- [AWS re:Post — SSM port forwarding via SDK](https://repost.aws/questions/QU5pDJCPALRlGKtGyuHjMJaw/aws-ssm-port-forwarding-session-using-aws-sdk)
- [Formal.ai — SSM protocol deep dive](https://www.joinformal.com/blog/down-the-rabbit-hole-implementing-ssh-port-forwarding-over-aws-session-manager/)
- [DeepWiki — session-manager-plugin analysis](https://deepwiki.com/aws/session-manager-plugin)
