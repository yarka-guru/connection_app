#!/usr/bin/env node
/**
 * Checks that demo mode's canned responses match the shapes the UI renders.
 *
 * The first version of src/lib/demo.js returned projects without a `key` and
 * profiles as objects rather than strings. Nothing threw — the dropdowns simply
 * rendered empty, which is indistinguishable from "demo mode is broken" and is
 * exactly what an App Store reviewer would have seen.
 *
 * The contract, read from the components rather than assumed:
 *   ConnectionForm.svelte  <option value={project.key}>{project.name}</option>
 *   ConnectionForm.svelte  <option value={profile}>{profile}</option>
 *   App.svelte             filteredProjects filters on connectionType
 *
 * Run: node scripts/check-demo-data.mjs
 */
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const here = path.dirname(fileURLToPath(import.meta.url))
const { createDemoInvoke } = await import(
  path.join(here, '..', 'src', 'lib', 'demo.js')
)

let failed = 0
const ok = (m) => console.log(`  [32m✓[0m ${m}`)
const bad = (m) => {
  console.log(`  [31m✗[0m ${m}`)
  failed = 1
}

const invoke = createDemoInvoke(() => {})

const projects = await invoke('list_projects')
for (const p of projects) {
  if (typeof p.key === 'string' && p.key) {
    ok(`project "${p.name}" has key="${p.key}"`)
  } else {
    bad(`project ${JSON.stringify(p)} has no usable key — the option value would be undefined`)
  }
  if (typeof p.name !== 'string' || !p.name) bad(`project ${p.key} has no name`)
}

const rds = projects.filter((p) => (p.connectionType || 'rds') === 'rds')
const service = projects.filter((p) => (p.connectionType || 'rds') === 'service')
rds.length
  ? ok(`RDS tab shows ${rds.length}: ${rds.map((p) => p.name).join(', ')}`)
  : bad('RDS tab would be empty')
service.length
  ? ok(`VNC/RDP tab shows ${service.length}: ${service.map((p) => p.name).join(', ')}`)
  : bad('VNC/RDP tab would be empty')

const profiles = await invoke('list_profiles', { projectKey: 'acme-analytics' })
Array.isArray(profiles) && profiles.every((p) => typeof p === 'string')
  ? ok(`profiles are plain strings: ${profiles.join(', ')}`)
  : bad(`profiles must be plain strings, got ${JSON.stringify(profiles)}`)

const other = await invoke('list_profiles', { projectKey: 'northwind-billing' })
JSON.stringify(profiles) !== JSON.stringify(other)
  ? ok(`profiles differ per project: northwind -> ${other.join(', ')}`)
  : bad('the profile list is identical across projects — demo looks static')

for (const cmd of [
  'is_updater_enabled',
  'get_sandbox_status',
  'get_current_version',
  'check_migration_available',
  'load_saved_connections',
]) {
  const v = await invoke(cmd)
  v !== undefined
    ? ok(`${cmd} -> ${JSON.stringify(v)}`)
    : bad(`${cmd} returned undefined`)
}

const conn = await invoke('connect', {
  projectKey: 'acme-analytics',
  profile: 'acme-dev',
})
conn?.localPort && conn?.endpoint
  ? ok(`connect -> port ${conn.localPort}, ${conn.endpoint}`)
  : bad(`connect returned something unusable: ${JSON.stringify(conn)}`)

// Demo mode must not be able to name a host that exists. .invalid is reserved
// by RFC 2606 and never resolves.
String(conn?.endpoint).endsWith('.invalid')
  ? ok('endpoint is on .invalid — cannot resolve to anything real')
  : bad(`endpoint ${conn?.endpoint} is not on .invalid`)

console.log()
console.log(failed ? 'Demo data does NOT match the UI contract.' : 'Demo data matches the UI contract.')
process.exit(failed)
