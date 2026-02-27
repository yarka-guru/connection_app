import { execSync } from 'node:child_process'
import { spawn } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

/**
 * Locates the session-manager-plugin binary on the system.
 *
 * Search order:
 *   1. PATH lookup via `which` (or `where` on Windows)
 *   2. Known platform-specific installation paths
 *   3. Bundled binary at <project-root>/bin/<platform>-<arch>/
 *
 * @returns {string|null} Absolute path to the binary, or null if not found.
 */
export function findPluginBinary() {
  // 1. Check PATH using which/where
  const pathResult = findOnPath()
  if (pathResult) return pathResult

  // 2. Check known platform-specific installation paths
  const platformResult = findAtKnownPaths()
  if (platformResult) return platformResult

  // 3. Check for bundled binary (future-proofing for Phase 4)
  const bundledResult = findBundledBinary()
  if (bundledResult) return bundledResult

  return null
}

/**
 * Spawns the session-manager-plugin binary directly for port forwarding.
 *
 * @param {string} pluginPath - Absolute path to the plugin binary.
 * @param {object} sessionResponse - Response from StartSession API
 *   (contains SessionId, StreamUrl, TokenValue).
 * @param {string} region - AWS region string.
 * @param {object} sessionRequest - Original StartSession request parameters
 *   (contains Target, DocumentName, Parameters).
 * @returns {import('node:child_process').ChildProcess} The spawned child process.
 */
export function spawnPlugin(pluginPath, sessionResponse, region, sessionRequest) {
  const ssmEndpoint = `https://ssm.${region}.amazonaws.com`

  const child = spawn(pluginPath, [
    JSON.stringify(sessionResponse),
    region,
    'StartSession',
    '',
    JSON.stringify(sessionRequest),
    ssmEndpoint,
  ], {
    detached: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  })

  return child
}

/**
 * Runs `session-manager-plugin --version` and returns the version string.
 *
 * @param {string} pluginPath - Absolute path to the plugin binary.
 * @returns {string|null} Version string, or null if the command fails.
 */
export function checkPluginVersion(pluginPath) {
  try {
    const output = execSync(`"${pluginPath}" --version`, {
      encoding: 'utf-8',
      timeout: 5000,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    return output.trim() || null
  } catch {
    return null
  }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/**
 * Attempts to locate the binary on PATH via `which` (Unix) or `where` (Windows).
 */
function findOnPath() {
  const command = process.platform === 'win32'
    ? 'where session-manager-plugin'
    : 'which session-manager-plugin'

  try {
    const result = execSync(command, {
      encoding: 'utf-8',
      timeout: 5000,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    const resolvedPath = result.trim().split(/\r?\n/)[0]
    if (resolvedPath && isExecutable(resolvedPath)) {
      return resolvedPath
    }
  } catch {
    // Not found on PATH
  }

  return null
}

/**
 * Checks known platform-specific installation paths.
 */
function findAtKnownPaths() {
  const knownPaths = []

  if (process.platform === 'darwin' || process.platform === 'linux') {
    knownPaths.push('/usr/local/sessionmanagerplugin/bin/session-manager-plugin')
  }

  if (process.platform === 'win32') {
    knownPaths.push('C:\\Program Files\\Amazon\\SessionManagerPlugin\\bin\\session-manager-plugin.exe')
  }

  for (const candidate of knownPaths) {
    if (isExecutable(candidate)) {
      return candidate
    }
  }

  return null
}

/**
 * Checks for a bundled binary at <project-root>/bin/<platform>-<arch>/.
 * Future-proofing for Phase 4 bundled distribution.
 */
function findBundledBinary() {
  const platform = process.platform === 'win32' ? 'win32' : process.platform
  const arch = process.arch
  const binaryName = process.platform === 'win32'
    ? 'session-manager-plugin.exe'
    : 'session-manager-plugin'

  // Resolve project root relative to this file's location (src/)
  const projectRoot = path.resolve(path.dirname(import.meta.url.replace('file://', '')), '..')
  const candidate = path.join(projectRoot, 'bin', `${platform}-${arch}`, binaryName)

  if (isExecutable(candidate)) {
    return candidate
  }

  return null
}

/**
 * Checks whether a file exists and is executable.
 */
function isExecutable(filePath) {
  try {
    fs.accessSync(filePath, fs.constants.X_OK)
    return true
  } catch {
    return false
  }
}
