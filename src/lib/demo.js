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
 */

// Shapes must match what the Rust commands return, or the UI silently renders
// nothing. Project is { key, name, connectionType, databases? } — see
// commands/projects.rs. connectionType drives the RDS / VNC-RDP tab filter in
// App.svelte and is either 'rds' or 'service'. Profiles are plain strings.
const DEMO_PROJECTS = [
  {
    key: 'acme-analytics',
    name: 'Acme Analytics',
    connectionType: 'rds',
    databases: ['analytics', 'reporting'],
  },
  {
    key: 'northwind-billing',
    name: 'Northwind Billing',
    connectionType: 'rds',
    databases: ['billing'],
  },
  {
    key: 'acme-jumphost',
    name: 'Acme Jump Host',
    connectionType: 'service',
  },
]

const DEMO_PROFILES = ['acme-dev', 'acme-staging', 'acme-prod', 'northwind-dev']

const CONNECT_STEPS = [
  'Checking SSO session...',
  'Fetching credentials from Secrets Manager...',
  'Locating bastion instance...',
  'Resolving RDS endpoint...',
  'Opening tunnel...',
]

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function demoConnection(project, profile) {
  return {
    id: `demo-${profile}`,
    connectionId: `demo-${profile}`,
    project,
    profile,
    environment: profile,
    host: '127.0.0.1',
    localPort: 54320,
    port: 54320,
    endpoint: 'acme-analytics-dev.cluster.rds.example.invalid',
    username: 'demo_readonly',
    password: 'sample-value-not-a-real-password',
    database: 'analytics',
    engine: 'postgres',
  }
}

/**
 * Build a demo `invoke`. Pass a callback to receive status strings so the UI
 * can show the same progress sequence a real connection produces.
 */
export function createDemoInvoke(onStatus) {
  let connections = []

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
      case 'list_project_configs':
        return DEMO_PROJECTS
      case 'list_profiles': {
        // The real command filters by project; mirror that so switching
        // projects visibly changes the profile list.
        const key = args?.projectKey ?? ''
        const prefix = key.split('-')[0]
        const matching = DEMO_PROFILES.filter((p) => p.startsWith(prefix))
        return matching.length > 0 ? matching : DEMO_PROFILES
      }
      case 'read_aws_config':
        // Settings expects richer profile objects here. Returning the wrong
        // shape renders a broken list, so return nothing rather than guess.
        return []
      case 'get_raw_aws_config':
        return '# Demo mode — sample data, not your real AWS config.\n'

      // Saved connections and history
      case 'load_saved_connections':
        return []
      case 'get_connection_history':
        return []
      case 'get_active_connections_list':
        return connections
      case 'get_used_ports':
        return connections.map((c) => c.localPort)

      // The connection flow, played out as a sequence
      case 'connect': {
        for (const step of CONNECT_STEPS) {
          onStatus?.(step)
          await sleep(600)
        }
        onStatus?.('Connected (demo)')
        const conn = demoConnection(
          args?.projectKey ?? 'Acme Analytics',
          args?.profile ?? 'acme-dev',
        )
        connections = [...connections, conn]
        return conn
      }
      case 'disconnect':
        connections = connections.filter((c) => c.id !== args?.connectionId)
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
