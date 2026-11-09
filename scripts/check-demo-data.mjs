#!/usr/bin/env node
/**
 * Checks that demo mode's canned responses match the shapes the UI renders.
 *
 * Demo mode is the whole App Store review path, and a shape mismatch does not
 * throw where anyone sees it: the first version returned projects without a
 * `key` and the dropdowns rendered empty; after 3.8 changed `list_projects` to
 * an overview object, the 3.7-era demo left the form stuck on "Loading
 * projects..." with a TypeError in the console. Both are exactly what a
 * reviewer would have seen.
 *
 * The contract, read from the components and the Rust return types:
 *   App.svelte             overview.projects, overview.needsMigration
 *   ConnectionForm.svelte  p.reachable, p.broken, project.environments[]
 *                          resolved.environment.name, resolved.profile,
 *                          resolved.candidates
 *   App.svelte             saved.id after save_connection
 *   App.svelte             result.connectionId, result.connectionInfo.{port,
 *                          environment, profile}
 *
 * Run: node scripts/check-demo-data.mjs
 */
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const here = path.dirname(fileURLToPath(import.meta.url))
const { createDemoInvoke } = await import(path.join(here, '..', 'src', 'lib', 'demo.js'))

let failed = 0
const ok = (m) => console.log(`  [32m✓[0m ${m}`)
const bad = (m) => {
  console.log(`  [31m✗[0m ${m}`)
  failed = 1
}
const check = (cond, good, fail) => (cond ? ok(good) : bad(fail))

const invoke = createDemoInvoke(() => {})

const overview = await invoke('list_projects')
check(
  overview && Array.isArray(overview.projects) && typeof overview.needsMigration === 'boolean',
  `list_projects is an overview with ${overview?.projects?.length} projects`,
  `list_projects must return { projects, needsMigration, ... }, got ${JSON.stringify(overview)?.slice(0, 120)}`,
)
const projects = overview?.projects ?? []

for (const p of projects) {
  check(typeof p.key === 'string' && p.key, `project "${p.name}" has key="${p.key}"`, `project ${JSON.stringify(p)} has no usable key`)
  check(p.reachable === true && !p.broken, `${p.key} is reachable`, `${p.key} is hidden by ConnectionForm (reachable=${p.reachable})`)
  check(
    Array.isArray(p.environments) && p.environments.length > 0,
    `${p.key} has ${p.environments?.length} environments`,
    `${p.key} has no environments — the environment dropdown would be empty`,
  )
  for (const e of p.environments ?? []) {
    const name = e?.environment?.name
    check(
      typeof name === 'string' && typeof e.profile === 'string' && Array.isArray(e.candidates) && e.candidates.includes(e.profile),
      `  ${p.key}/${name} -> ${e.profile}`,
      `  ${p.key}: malformed ResolvedEnvironment ${JSON.stringify(e)}`,
    )
  }
}

const rds = projects.filter((p) => (p.connectionType || 'rds') === 'rds')
const service = projects.filter((p) => p.connectionType === 'service')
check(rds.length, `RDS tab shows ${rds.length}`, 'RDS tab would be empty')
check(service.length, `VNC/RDP tab shows ${service.length}`, 'VNC/RDP tab would be empty')
check(
  projects.some((p) => p.environments.some((e) => e.candidates.length > 1)),
  'at least one environment offers a profile choice',
  'no environment has several candidates — the profile picker never appears',
)

for (const cmd of ['is_updater_enabled', 'get_sandbox_status', 'get_current_version', 'check_migration_available']) {
  const v = await invoke(cmd)
  check(v !== undefined, `${cmd} -> ${JSON.stringify(v)}`, `${cmd} returned undefined`)
}
for (const cmd of ['load_saved_connections', 'list_project_entries', 'list_aws_accounts', 'read_aws_config']) {
  const v = await invoke(cmd)
  check(Array.isArray(v), `${cmd} -> array(${v?.length})`, `${cmd} must return an array, got ${JSON.stringify(v)}`)
}

const saved = await invoke('save_connection', {
  name: 'x',
  projectKey: 'acme-analytics',
  environment: 'dev',
  profile: 'acme-dev',
})
check(saved?.id, `save_connection -> id ${saved?.id}`, 'save_connection must return the saved entry (App.svelte reads saved.id)')

for (const p of projects) {
  const e = p.environments[0].environment.name
  const r = await invoke('connect', { projectKey: p.key, environment: e, profile: null, database: null })
  const info = r?.connectionInfo
  check(
    r?.connectionId && info?.port && info?.environment === e && info?.profile,
    `connect ${p.key}/${e} -> localhost:${info?.port} (${info?.profile})`,
    `connect returned something unusable: ${JSON.stringify(r)}`,
  )
  // Demo mode must not be able to name a host that exists. .invalid is
  // reserved by RFC 2606 and never resolves.
  const remote = info?.rdsEndpoint ?? info?.remoteHost
  check(String(remote).endsWith('.invalid'), `  remote ${remote} is on .invalid`, `  remote ${remote} is not on .invalid`)
}

console.log()
console.log(failed ? 'Demo data does NOT match the UI contract.' : 'Demo data matches the UI contract.')
process.exit(failed)
