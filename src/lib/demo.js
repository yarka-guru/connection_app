/**
 * Demo mode: a stand-in for Tauri's `invoke` that returns fabricated data.
 *
 * This is a real product feature — a preview for anyone who has not configured
 * AWS yet — not a reviewer-only path. App Store Guideline 2.3.1 forbids hidden
 * functionality, and a mode that behaved differently for Apple would be
 * dishonest besides.
 *
 * It lives entirely in the frontend, so it is structurally incapable of opening
 * a socket or reaching AWS: nothing here crosses into Rust. Hostnames use the
 * .invalid TLD (RFC 2606), which is guaranteed never to resolve, so even a
 * copy-pasted endpoint cannot accidentally point at something real.
 *
 * Shapes must match what the Rust commands return, or the UI silently renders
 * nothing — or throws. `scripts/check-demo-data.mjs` asserts them; keep it in
 * step whenever a command's return type changes:
 *   list_projects          -> ProjectsOverview  (commands/projects.rs)
 *   ProjectSummary.environments -> ResolvedEnvironment (config/environments.rs)
 *   load_saved_connections -> SavedConnectionDto[] (commands/saved.rs)
 *   connect                -> ConnectResult { connectionId, connectionInfo }
 *                             (commands/connection.rs, tunnel/manager.rs)
 */

const ACCOUNTS = {
  acmeDev: '111111111111',
  acmeStaging: '222222222222',
  acmeProd: '333333333333',
  northwind: '444444444444',
}

function env(name, accountId, port, profile, candidates = [profile]) {
  return {
    environment: { name, accountId, port: String(port) },
    legacy: false,
    profile,
    candidates,
    pinned: false,
  }
}

const DEMO_PROJECTS = [
  {
    key: 'acme-analytics',
    name: 'Acme Analytics',
    connectionType: 'rds',
    databases: ['analytics', 'reporting'],
    environments: [
      env('dev', ACCOUNTS.acmeDev, 15432, 'acme-dev'),
      env('staging', ACCOUNTS.acmeStaging, 15433, 'acme-staging'),
      env('prod', ACCOUNTS.acmeProd, 15434, 'acme-prod-readonly', [
        'acme-prod-readonly',
        'acme-prod-admin',
      ]),
    ],
    problems: [],
    broken: null,
    reachable: true,
  },
  {
    key: 'northwind-billing',
    name: 'Northwind Billing',
    connectionType: 'rds',
    databases: ['billing'],
    environments: [env('dev', ACCOUNTS.northwind, 15532, 'northwind-dev')],
    problems: [],
    broken: null,
    reachable: true,
  },
  {
    key: 'acme-jumphost',
    name: 'Acme Jump Host',
    connectionType: 'service',
    databases: null,
    environments: [
      env('dev', ACCOUNTS.acmeDev, 15900, 'acme-dev'),
      env('prod', ACCOUNTS.acmeProd, 15901, 'acme-prod-admin'),
    ],
    problems: [],
    broken: null,
    reachable: true,
  },
]

const DEMO_SAVED = [
  {
    id: 'demo-saved-1',
    name: 'Analytics — staging',
    projectKey: 'acme-analytics',
    profile: 'acme-staging',
    environment: 'staging',
    database: 'analytics',
    lastUsedAt: null,
    group: null,
    needsAttention: false,
  },
  {
    id: 'demo-saved-2',
    name: 'Billing — dev',
    projectKey: 'northwind-billing',
    profile: 'northwind-dev',
    environment: 'dev',
    database: 'billing',
    lastUsedAt: null,
    group: null,
    needsAttention: false,
  },
]

const CONNECT_STEPS = [
  'Checking SSO session...',
  'Fetching credentials from Secrets Manager...',
  'Locating bastion instance...',
  'Resolving RDS endpoint...',
  'Opening tunnel...',
]

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function demoConnectResult(args, seq) {
  const project =
    DEMO_PROJECTS.find((p) => p.key === args?.projectKey) ?? DEMO_PROJECTS[0]
  const resolved =
    project.environments.find(
      (e) => e.environment.name === args?.environment,
    ) ?? project.environments[0]
  const environment = resolved.environment.name
  const profile = args?.profile || resolved.profile
  const port = resolved.environment.port
  const base = {
    host: 'localhost',
    port,
    environment,
    profile,
    connectionType: project.connectionType,
  }
  const connectionInfo =
    project.connectionType === 'service'
      ? {
          ...base,
          serviceType: 'vnc',
          targetType: 'ec2',
          remoteHost: `jumphost-${environment}.internal.example.invalid`,
        }
      : {
          ...base,
          username: 'demo_readonly',
          password: 'sample-value-not-a-real-password',
          database: args?.database || project.databases?.[0] || 'postgres',
          rdsEndpoint: `${project.key}-${environment}.cluster.rds.example.invalid`,
          engine: 'postgres',
          authMethod: 'password',
        }
  return {
    connectionId: `demo-${project.key}-${environment}-${seq}`,
    connectionInfo,
  }
}

/**
 * Build a demo `invoke`. Pass a callback to receive status strings so the UI
 * can show the same progress sequence a real connection produces.
 */
export function createDemoInvoke(onStatus) {
  let connections = []
  let saved = DEMO_SAVED.map((s) => ({ ...s }))
  let seq = 0

  return async function demoInvoke(cmd, args) {
    switch (cmd) {
      // Anything the app asks about its environment
      case 'is_updater_enabled':
        return false
      case 'get_sandbox_status':
        return { isSandboxed: false, hasAwsAccess: true }
      case 'get_current_version':
        return 'demo'
      case 'check_migration_available':
        return false

      // Configuration
      case 'list_projects':
        return {
          projects: DEMO_PROJECTS,
          fileError: null,
          needsMigration: false,
          backups: [],
        }
      case 'list_project_entries':
        // Settings shows the project editor from these; an empty list renders
        // an empty editor rather than a fake config that cannot be saved.
        return []
      case 'list_aws_accounts':
      case 'read_aws_config':
        return []
      case 'get_raw_aws_config':
        return '# Demo mode — sample data, not your real AWS config.\n'

      // Saved connections
      case 'load_saved_connections':
        return saved
      case 'save_connection': {
        const entry = {
          id: `demo-saved-${Date.now()}`,
          name: args?.name || 'Saved connection',
          projectKey: args?.projectKey,
          profile: args?.profile,
          environment: args?.environment ?? null,
          database: args?.database ?? null,
          lastUsedAt: null,
          group: null,
          needsAttention: false,
        }
        saved = [...saved, entry]
        return entry
      }
      case 'delete_saved_connection':
        saved = saved.filter((s) => s.id !== args?.id)
        return null

      // The connection flow, played out as a sequence
      case 'connect': {
        for (const step of CONNECT_STEPS) {
          onStatus?.(step)
          await sleep(600)
        }
        onStatus?.('Connected (demo)')
        seq += 1
        const result = demoConnectResult(args, seq - 1)
        connections = [...connections, result.connectionId]
        return result
      }
      case 'disconnect':
        connections = connections.filter((id) => id !== args?.connectionId)
        onStatus?.('Disconnected (demo)')
        return null
      case 'disconnect_all':
        connections = []
        onStatus?.('Disconnected (demo)')
        return null

      // Everything else is a no-op. Demo mode persists nothing: saving a
      // project or profile here must not reach disk, so these silently
      // succeed and change nothing.
      default:
        return null
    }
  }
}
