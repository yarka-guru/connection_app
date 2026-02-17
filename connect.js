#!/usr/bin/env node

import { exec, spawn, spawnSync } from 'node:child_process'
import { EventEmitter } from 'node:events'
import fs from 'node:fs/promises'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'
import { promisify } from 'node:util'
import {
  loadProjectConfigs,
  saveProjectConfig,
  validateProjectConfig,
} from './configLoader.js'

// Package info for version checking
const packageJson = { name: 'rds_ssm_connect', version: '1.8.0' }

const execAsync = promisify(exec)

// Event emitter for IPC communication
const ipcEmitter = new EventEmitter()

// Configuration constants
const RETRY_CONFIG = {
  BASTION_WAIT_MAX_RETRIES: 20,
  BASTION_WAIT_RETRY_DELAY_MS: 15000,
  PORT_FORWARDING_MAX_RETRIES: 2,
  SSM_AGENT_READY_WAIT_MS: 10000,
  KEEPALIVE_INTERVAL_MS: 4 * 60 * 1000,
  AUTO_RECONNECT_MAX_RETRIES: 50,
  AUTO_RECONNECT_DELAY_MS: 3000,
}

// Version check configuration
const VERSION_CHECK_TIMEOUT_MS = 3000
const PACKAGE_NAME = packageJson.name
const CURRENT_VERSION = packageJson.version

async function checkForUpdates() {
  try {
    const controller = new AbortController()
    const timeoutId = setTimeout(
      () => controller.abort(),
      VERSION_CHECK_TIMEOUT_MS,
    )

    const response = await fetch(
      `https://registry.npmjs.org/${PACKAGE_NAME}/latest`,
      {
        signal: controller.signal,
      },
    )
    clearTimeout(timeoutId)

    if (!response.ok) return

    const data = await response.json()
    const latestVersion = data.version

    if (latestVersion && latestVersion !== CURRENT_VERSION) {
      const [latestMajor, latestMinor, latestPatch] = latestVersion
        .split('.')
        .map(Number)
      const [currentMajor, currentMinor, currentPatch] =
        CURRENT_VERSION.split('.').map(Number)

      const isNewer =
        latestMajor > currentMajor ||
        (latestMajor === currentMajor && latestMinor > currentMinor) ||
        (latestMajor === currentMajor &&
          latestMinor === currentMinor &&
          latestPatch > currentPatch)

      if (isNewer) {
      }
    }
  } catch {
    // Silently ignore version check failures
  }
}

// Store active child processes for cleanup
let activeChildProcesses = []

// Recursively collect all descendant PIDs of a process via pgrep.
function getDescendantPids(pid) {
  const descendants = []
  try {
    const { stdout } = spawnSync('pgrep', ['-P', String(pid)], {
      encoding: 'utf-8',
      timeout: 3000,
    })
    if (stdout) {
      const children = stdout.trim().split('\n').filter(Boolean).map(Number)
      for (const childPid of children) {
        descendants.push(childPid, ...getDescendantPids(childPid))
      }
    }
  } catch (_err) {
    // pgrep not available or timed out
  }
  return descendants
}

// Kill entire process tree: shell → aws-vault → aws → session-manager-plugin.
// Uses three strategies because descendants may escape to different process
// groups (aws-vault / AWS CLI behavior on macOS).
function killProcessTree(child) {
  if (!child || !child.pid) return
  const rootPid = child.pid

  // Walk the tree to find every descendant PID
  const descendants = getDescendantPids(rootPid)
  const allPids = [rootPid, ...descendants]

  // Strategy 1: Process group kill (fast, atomic — works when all share PGID)
  try { process.kill(-rootPid, 'SIGTERM') } catch (_err) {}

  // Strategy 2: Individual SIGTERM (handles descendants in different groups)
  for (const pid of allPids) {
    try { process.kill(pid, 'SIGTERM') } catch (_err) {}
  }

  // Strategy 3: SIGKILL survivors (cannot be caught or ignored)
  for (const pid of allPids) {
    try { process.kill(pid, 'SIGKILL') } catch (_err) {}
  }
}

// Handle graceful shutdown
function setupProcessCleanup() {
  const killAll = () => {
    activeChildProcesses.forEach((child) => {
      killProcessTree(child)
    })
    activeChildProcesses = []
  }

  process.on('SIGINT', () => { killAll(); process.exit(0) })
  process.on('SIGTERM', () => { killAll(); process.exit(0) })
  process.on('exit', killAll)
}

async function readAwsConfig() {
  const awsConfigPath = path.join(os.homedir(), '.aws', 'config')
  try {
    const awsConfig = await fs.readFile(awsConfigPath, { encoding: 'utf-8' })
    return awsConfig
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line.startsWith('[') && line.endsWith(']'))
      .map((line) => line.slice(1, -1))
      .map((line) => line.replace('profile ', '').trim())
  } catch (_error) {
    return []
  }
}

async function runCommand(command) {
  try {
    const { stdout } = await execAsync(command)
    return stdout.trim()
  } catch (_error) {
    return null
  }
}

async function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

// Keepalive: periodic TCP ping through the tunnel to prevent SSM idle timeout.
// Each connection attempt generates traffic on the SSM WebSocket channel,
// resetting the server-side idle timer (default 20 min).
function startKeepalive(localPort) {
  const timer = setInterval(() => {
    const socket = new net.Socket()
    socket.setTimeout(5000)
    socket.connect(parseInt(localPort, 10), '127.0.0.1', () => {
      socket.destroy()
    })
    socket.on('error', () => socket.destroy())
    socket.on('timeout', () => socket.destroy())
  }, RETRY_CONFIG.KEEPALIVE_INTERVAL_MS)
  return () => clearInterval(timer)
}

async function terminateBastionInstance(ENV, instanceId, region) {
  const terminateCommand = `aws-vault exec ${ENV} -- aws ec2 terminate-instances --region ${region} --instance-ids ${instanceId}`
  await runCommand(terminateCommand)
}

async function waitForNewBastionInstance(
  ENV,
  oldInstanceId,
  region,
  maxRetries = RETRY_CONFIG.BASTION_WAIT_MAX_RETRIES,
  retryDelay = RETRY_CONFIG.BASTION_WAIT_RETRY_DELAY_MS,
) {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    const instanceIdCommand = `aws-vault exec ${ENV} -- aws ec2 describe-instances --region ${region} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId] | [0][0]" --output text`
    const newInstanceId = await runCommand(instanceIdCommand)

    if (
      newInstanceId &&
      newInstanceId !== oldInstanceId &&
      newInstanceId !== 'None'
    ) {
      // Verify SSM agent is ready
      const isReady = await waitForSSMAgentReady(ENV, newInstanceId, region)
      if (isReady) {
        return newInstanceId
      } else {
      }
    }

    if (attempt < maxRetries) {
      await sleep(retryDelay)
    }
  }

  return null
}

async function waitForSSMAgentReady(
  ENV,
  instanceId,
  region,
  maxRetries = 10,
  retryDelay = 3000,
) {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    const ssmStatusCommand = `aws-vault exec ${ENV} -- aws ssm describe-instance-information --region ${region} --filters "Key=InstanceIds,Values=${instanceId}" --query "InstanceInformationList[0].PingStatus" --output text`
    const status = await runCommand(ssmStatusCommand)

    if (status === 'Online') {
      // Additional wait for agent to stabilize
      await sleep(RETRY_CONFIG.SSM_AGENT_READY_WAIT_MS)
      return true
    }

    if (attempt < maxRetries) {
      await sleep(retryDelay)
    }
  }
  return false
}

function monitorPortForwardingSession(child) {
  const state = {
    stderrOutput: '',
    targetNotConnectedError: false,
    sessionEstablished: false,
  }

  child.stdout.on('data', (data) => {
    const output = data.toString()

    // Detect when session is actually established
    if (output.includes('Starting session with SessionId:')) {
      state.sessionEstablished = true
    }

    if (
      !output.includes('Starting session with SessionId:') &&
      !output.includes('Port 5433 opened for sessionId')
    ) {
    }
  })

  child.stderr.on('data', (data) => {
    const errorOutput = data.toString()
    state.stderrOutput += errorOutput

    // Check for TargetNotConnected error
    if (
      errorOutput.includes('TargetNotConnected') ||
      errorOutput.includes('is not connected')
    ) {
      state.targetNotConnectedError = true
    }
  })

  return state
}

async function handleTargetNotConnectedError(
  ENV,
  instanceId,
  rdsEndpoint,
  portNumber,
  remotePort,
  region,
  retryCount,
  maxRetries,
  onChild,
) {
  // Terminate the disconnected instance
  await terminateBastionInstance(ENV, instanceId, region)

  // Wait for new instance to be ready
  const newInstanceId = await waitForNewBastionInstance(ENV, instanceId, region)

  if (!newInstanceId) {
    throw new Error('Failed to find new bastion instance after waiting.')
  }
  return await startPortForwardingWithConfig(
    ENV,
    newInstanceId,
    rdsEndpoint,
    portNumber,
    remotePort,
    region,
    retryCount + 1,
    maxRetries,
    onChild,
  )
}

function executePortForwardingCommand(
  ENV,
  instanceId,
  rdsEndpoint,
  portNumber,
  remotePort,
  region,
) {
  const portForwardingCommand = `aws-vault exec ${ENV} -- aws ssm start-session --region ${region} --target ${instanceId} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${rdsEndpoint},portNumber='${remotePort}',localPortNumber='${portNumber}'" --cli-connect-timeout 0`
  const child = spawn(portForwardingCommand, {
    shell: true,
    detached: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  })

  // Register child process for cleanup
  activeChildProcesses.push(child)

  return child
}

async function startPortForwardingWithConfig(
  ENV,
  instanceId,
  rdsEndpoint,
  portNumber,
  remotePort,
  region,
  retryCount = 0,
  maxRetries = RETRY_CONFIG.PORT_FORWARDING_MAX_RETRIES,
  onChild,
) {
  return new Promise((resolve, reject) => {
    const child = executePortForwardingCommand(
      ENV,
      instanceId,
      rdsEndpoint,
      portNumber,
      remotePort,
      region,
    )
    // Notify caller of the new child (used for per-connection tracking).
    // If the connection was already disconnected, the callback kills this
    // child immediately so it doesn't linger as an orphan.
    if (onChild) onChild(child)

    const sessionState = monitorPortForwardingSession(child)

    child.on('close', async (code) => {
      // Remove from active processes
      activeChildProcesses = activeChildProcesses.filter((p) => p !== child)

      try {
        // Handle TargetNotConnected error with retry
        if (
          code === 254 &&
          sessionState.targetNotConnectedError &&
          retryCount < maxRetries
        ) {
          await handleTargetNotConnectedError(
            ENV,
            instanceId,
            rdsEndpoint,
            portNumber,
            remotePort,
            region,
            retryCount,
            maxRetries,
            onChild,
          )
          resolve()
        } else if (code !== 0) {
          reject(new Error(`Port forwarding failed with code ${code}`))
        } else {
          resolve()
        }
      } catch (error) {
        reject(error)
      }
    })
  })
}

// Legacy function for backward compatibility
async function _startPortForwarding(
  ENV,
  instanceId,
  rdsEndpoint,
  portNumber,
  retryCount = 0,
  maxRetries = RETRY_CONFIG.PORT_FORWARDING_MAX_RETRIES,
) {
  return startPortForwardingWithConfig(
    ENV,
    instanceId,
    rdsEndpoint,
    portNumber,
    '5432',
    'us-east-2',
    retryCount,
    maxRetries,
  )
}

// Validation patterns for security
const PROFILE_SAFE_PATTERN = /^[a-zA-Z0-9._-]+$/
const INSTANCE_ID_PATTERN = /^i-[a-f0-9]{8,17}$/
const HOSTNAME_PATTERN = /^[a-zA-Z0-9.-]+$/

function getDefaultPortForEngine(projectConfig) {
  return projectConfig.engine === 'mysql' ? '3306' : '5432'
}

async function getRdsEndpoint(ENV, projectConfig) {
  const { region, rdsType, rdsPattern } = projectConfig

  if (rdsType === 'cluster') {
    // Aurora cluster lookup
    const rdsEndpointCommand = `aws-vault exec ${ENV} -- aws rds describe-db-clusters --region ${region} --query "DBClusters[?Status=='available' && ends_with(DBClusterIdentifier, '${rdsPattern}')].Endpoint | [0]" --output text`
    return await runCommand(rdsEndpointCommand)
  } else {
    // Single RDS instance lookup
    const rdsEndpointCommand = `aws-vault exec ${ENV} -- aws rds describe-db-instances --region ${region} --query "DBInstances[?DBInstanceStatus=='available' && contains(DBInstanceIdentifier, '${rdsPattern}')].Endpoint.Address | [0]" --output text`
    return await runCommand(rdsEndpointCommand)
  }
}

async function getRdsPort(ENV, projectConfig) {
  const { region, rdsType, rdsPattern } = projectConfig
  const fallbackPort = getDefaultPortForEngine(projectConfig)

  if (rdsType === 'cluster') {
    const portCommand = `aws-vault exec ${ENV} -- aws rds describe-db-clusters --region ${region} --query "DBClusters[?Status=='available' && ends_with(DBClusterIdentifier, '${rdsPattern}')].Port | [0]" --output text`
    return (await runCommand(portCommand)) || fallbackPort
  } else {
    const portCommand = `aws-vault exec ${ENV} -- aws rds describe-db-instances --region ${region} --query "DBInstances[?DBInstanceStatus=='available' && contains(DBInstanceIdentifier, '${rdsPattern}')].Endpoint.Port | [0]" --output text`
    return (await runCommand(portCommand)) || fallbackPort
  }
}

function getProfilesForProject(allProfiles, projectConfig, allProjectConfigs) {
  const { profileFilter, envPortMapping } = projectConfig

  let filtered
  if (profileFilter) {
    // Project has explicit filter - return profiles starting with filter
    filtered = allProfiles.filter((env) => env.startsWith(profileFilter))
  } else {
    // No filter (legacy project like TLN) - return profiles that don't match any other project's filter
    const otherFilters = Object.values(allProjectConfigs)
      .filter((config) => config.profileFilter)
      .map((config) => config.profileFilter)

    filtered = allProfiles.filter(
      (env) => !otherFilters.some((filter) => env.startsWith(filter)),
    )
  }

  // Further restrict to profiles matching an envPortMapping suffix
  if (envPortMapping && Object.keys(envPortMapping).length > 0) {
    const suffixes = Object.keys(envPortMapping).sort(
      (a, b) => b.length - a.length,
    )
    filtered = filtered.filter((env) =>
      suffixes.some((suffix) => env.endsWith(suffix) || env === suffix),
    )
  }

  return filtered
}

// Get local port number based on environment suffix
function getLocalPort(ENV, projectConfig) {
  const { envPortMapping, defaultPort } = projectConfig
  const allEnvSuffixes = Object.keys(envPortMapping).sort(
    (a, b) => b.length - a.length,
  )
  const matchedSuffix =
    allEnvSuffixes.find((suffix) => ENV.endsWith(suffix)) ||
    allEnvSuffixes.find((suffix) => ENV === suffix)
  return envPortMapping[matchedSuffix] || defaultPort
}

// Get RDS credentials from Secrets Manager
async function getConnectionCredentials(ENV, projectConfig) {
  const { region, secretPrefix, database } = projectConfig

  const secretsListCommand = `aws-vault exec ${ENV} -- aws secretsmanager list-secrets --region ${region} --query "SecretList[?contains(Name, '${secretPrefix}')].Name | [0]" --output text`
  const SECRET_NAME = await runCommand(secretsListCommand)

  if (!SECRET_NAME || SECRET_NAME === 'None') {
    throw new Error(
      `No secret found with name containing '${secretPrefix}'.`,
    )
  }

  const secretsGetCommand = `aws-vault exec ${ENV} -- aws secretsmanager get-secret-value --region ${region} --secret-id "${SECRET_NAME}" --query SecretString --output text`
  const secretString = await runCommand(secretsGetCommand)

  if (!secretString) {
    throw new Error('Failed to retrieve secret value from Secrets Manager.')
  }

  let credentials
  try {
    credentials = JSON.parse(secretString)
    if (!credentials.username || !credentials.password) {
      throw new Error('Missing username or password in credentials')
    }
  } catch (error) {
    throw new Error(
      `Failed to parse credentials from Secrets Manager: ${error.message}`,
    )
  }

  return {
    username: credentials.username,
    password: credentials.password,
    database,
    secretName: SECRET_NAME,
  }
}

// Find running bastion instance
async function findBastionInstance(ENV, region) {
  const instanceIdCommand = `aws-vault exec ${ENV} -- aws ec2 describe-instances --region ${region} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId] | [0][0]" --output text`
  const instanceId = await runCommand(instanceIdCommand)

  if (!instanceId || instanceId === 'None') {
    throw new Error(
      'Failed to find a running instance with tag Name=*bastion*.',
    )
  }

  return instanceId
}

// Get available projects based on AWS profiles
async function getAvailableProjects() {
  const allProfiles = await readAwsConfig()

  if (allProfiles.length === 0) {
    return []
  }

  const PROJECT_CONFIGS = await loadProjectConfigs()

  return Object.entries(PROJECT_CONFIGS)
    .filter(([_key, config]) => {
      const matchingProfiles = getProfilesForProject(
        allProfiles,
        config,
        PROJECT_CONFIGS,
      )
      return matchingProfiles.length > 0
    })
    .map(([key, config]) => ({
      key,
      name: config.name,
    }))
}

// Get profiles for a specific project
async function getProfilesForProjectKey(projectKey) {
  const allProfiles = await readAwsConfig()
  const PROJECT_CONFIGS = await loadProjectConfigs()
  const projectConfig = PROJECT_CONFIGS[projectKey]

  if (!projectConfig) {
    throw new Error(`Unknown project: ${projectKey}`)
  }

  return getProfilesForProject(allProfiles, projectConfig, PROJECT_CONFIGS)
}

// Connect to RDS through bastion - returns connection info and control object.
// Includes keepalive (prevents SSM idle timeout) and auto-reconnect
// (transparently reconnects on the same port if session drops unexpectedly).
async function connect(projectKey, profile, options = {}) {
  if (!PROFILE_SAFE_PATTERN.test(profile)) {
    throw new Error(`Invalid profile name: ${profile}`)
  }

  const PROJECT_CONFIGS = await loadProjectConfigs()
  const projectConfig = PROJECT_CONFIGS[projectKey]
  if (!projectConfig) {
    throw new Error(`Unknown project: ${projectKey}`)
  }

  const { region, database } = projectConfig
  // Use provided localPort or fall back to computed port from profile
  const localPort = options.localPort || getLocalPort(profile, projectConfig)

  let manualDisconnect = false
  let stopKeepalive = null
  let currentChild = null // per-connection child tracking

  // Called whenever a new child process is spawned for this connection.
  // If disconnect() was already called, kills the child immediately so
  // it doesn't linger as an orphan during retry chains.
  const onChild = (child) => {
    currentChild = child
    if (manualDisconnect) {
      killProcessTree(child)
      activeChildProcesses = activeChildProcesses.filter((p) => p !== child)
    }
  }

  // Emit status updates
  const emit = (event, data) => {
    ipcEmitter.emit(event, data)
    if (options.onEvent) {
      options.onEvent(event, data)
    }
  }

  emit('status', { message: 'Getting credentials...' })
  const credentials = await getConnectionCredentials(profile, projectConfig)

  emit('status', { message: 'Finding bastion instance...' })
  let currentInstanceId = await findBastionInstance(profile, region)

  if (!INSTANCE_ID_PATTERN.test(currentInstanceId)) {
    throw new Error(`Invalid instance ID format: ${currentInstanceId}`)
  }

  emit('status', { message: 'Getting RDS endpoint...' })
  let currentRdsEndpoint = await getRdsEndpoint(profile, projectConfig)

  if (!currentRdsEndpoint || currentRdsEndpoint === 'None') {
    throw new Error('Failed to find the RDS endpoint.')
  }

  if (!HOSTNAME_PATTERN.test(currentRdsEndpoint)) {
    throw new Error(`Invalid RDS endpoint format: ${currentRdsEndpoint}`)
  }

  emit('status', { message: 'Getting RDS port...' })
  const rdsPort = await getRdsPort(profile, projectConfig)

  const connectionInfo = {
    host: 'localhost',
    port: localPort,
    username: credentials.username,
    password: credentials.password,
    database,
    rdsEndpoint: currentRdsEndpoint,
    instanceId: currentInstanceId,
  }

  emit('credentials', connectionInfo)
  emit('status', { message: 'Starting port forwarding...' })

  // Auto-reconnect session management loop.
  // Keeps the tunnel alive on the SAME local port. Only exits on
  // manual disconnect or after exhausting reconnection attempts.
  const portForwardingPromise = (async () => {
    let reconnectCount = 0

    while (!manualDisconnect) {
      try {
        stopKeepalive = startKeepalive(localPort)

        await startPortForwardingWithConfig(
          profile,
          currentInstanceId,
          currentRdsEndpoint,
          localPort,
          rdsPort,
          region,
          0,
          RETRY_CONFIG.PORT_FORWARDING_MAX_RETRIES,
          onChild,
        )

        // Session ended — clean up keepalive
        stopKeepalive?.()
        stopKeepalive = null

        if (manualDisconnect) break

        // Unexpected disconnect (idle timeout, network issue) — auto-reconnect
        reconnectCount++
        if (reconnectCount > RETRY_CONFIG.AUTO_RECONNECT_MAX_RETRIES) {
          throw new Error('Maximum auto-reconnection attempts reached.')
        }

        emit('status', {
          message: `Session ended. Reconnecting... (${reconnectCount})`,
        })
        await sleep(RETRY_CONFIG.AUTO_RECONNECT_DELAY_MS)

        if (manualDisconnect) break

        // Re-discover infrastructure (bastion may have been replaced by ASG)
        emit('status', { message: 'Finding bastion instance...' })
        currentInstanceId = await findBastionInstance(profile, region)

        emit('status', { message: 'Getting RDS endpoint...' })
        currentRdsEndpoint = await getRdsEndpoint(profile, projectConfig)

        if (!currentRdsEndpoint || currentRdsEndpoint === 'None') {
          throw new Error(
            'Failed to find the RDS endpoint during reconnection.',
          )
        }

        emit('status', { message: 'Reconnecting port forwarding...' })
      } catch (error) {
        stopKeepalive?.()
        stopKeepalive = null

        if (manualDisconnect) break

        reconnectCount++
        if (reconnectCount > RETRY_CONFIG.AUTO_RECONNECT_MAX_RETRIES) {
          throw error
        }

        emit('status', {
          message: `Connection error. Retrying... (${reconnectCount}/${RETRY_CONFIG.AUTO_RECONNECT_MAX_RETRIES})`,
        })
        await sleep(RETRY_CONFIG.AUTO_RECONNECT_DELAY_MS * 2)

        if (manualDisconnect) break

        try {
          currentInstanceId = await findBastionInstance(profile, region)
          currentRdsEndpoint = await getRdsEndpoint(profile, projectConfig)
          if (!currentRdsEndpoint || currentRdsEndpoint === 'None') {
            throw new Error('Failed to find RDS endpoint')
          }
        } catch (_innerError) {
          // Will retry on next loop iteration
        }
      }
    }
  })()

  // Return connection control object
  return {
    connectionInfo,
    disconnect: () => {
      manualDisconnect = true
      stopKeepalive?.()
      // Kill only THIS connection's child process group
      if (currentChild) {
        killProcessTree(currentChild)
        activeChildProcesses = activeChildProcesses.filter((p) => p !== currentChild)
        currentChild = null
      }
    },
    waitForClose: () => portForwardingPromise,
  }
}

async function main() {
  // Dynamic import of inquirer (CLI only, not needed for GUI adapter)
  const inquirer = await import('inquirer')

  // Setup process cleanup handlers
  setupProcessCleanup()

  // Check for updates (non-blocking)
  await checkForUpdates()

  try {
    // Load project configs from user config file
    let PROJECT_CONFIGS = await loadProjectConfigs()

    // First-run wizard: if no projects configured, prompt to create one
    if (Object.keys(PROJECT_CONFIGS).length === 0) {
      const { setupNow } = await inquirer.default.prompt([
        {
          type: 'confirm',
          name: 'setupNow',
          message:
            'No projects configured. Would you like to set up a project now?',
          default: true,
        },
      ])

      if (!setupNow) {
        console.log(
          `\nTo configure manually, create ~/.rds-ssm-connect/projects.json\nSee the README for the config schema.\n`,
        )
        return
      }

      const answers = await inquirer.default.prompt([
        {
          type: 'input',
          name: 'key',
          message: 'Project key (lowercase, hyphens):',
          validate: (v) =>
            /^[a-z][a-z0-9-]*$/.test(v) || 'Lowercase letters, digits, hyphens',
        },
        { type: 'input', name: 'name', message: 'Display name:' },
        {
          type: 'input',
          name: 'region',
          message: 'AWS region:',
          default: 'us-east-1',
        },
        { type: 'input', name: 'database', message: 'Database name:' },
        {
          type: 'input',
          name: 'secretPrefix',
          message: 'Secret prefix (e.g. rds!cluster):',
        },
        {
          type: 'select',
          name: 'rdsType',
          message: 'RDS type:',
          choices: ['cluster', 'instance'],
        },
        {
          type: 'select',
          name: 'engine',
          message: 'Database engine:',
          choices: ['postgres', 'mysql'],
        },
        {
          type: 'input',
          name: 'rdsPattern',
          message: 'RDS identifier pattern:',
        },
        {
          type: 'input',
          name: 'profileFilter',
          message: 'Profile filter prefix (leave empty for none):',
          default: '',
        },
        {
          type: 'input',
          name: 'defaultPort',
          message: 'Default local port:',
          default: (ctx) => ctx.engine === 'mysql' ? '3306' : '5432',
        },
      ])

      // Collect port mappings
      const envPortMappingInput = {}
      let addMore = true
      while (addMore) {
        const mapping = await inquirer.default.prompt([
          { type: 'input', name: 'suffix', message: 'Environment suffix:' },
          { type: 'input', name: 'port', message: 'Local port:' },
          {
            type: 'confirm',
            name: 'more',
            message: 'Add another port mapping?',
            default: false,
          },
        ])
        envPortMappingInput[mapping.suffix] = mapping.port
        addMore = mapping.more
      }

      const newConfig = {
        name: answers.name,
        region: answers.region,
        database: answers.database,
        secretPrefix: answers.secretPrefix,
        rdsType: answers.rdsType,
        engine: answers.engine,
        rdsPattern: answers.rdsPattern,
        profileFilter: answers.profileFilter || null,
        envPortMapping: envPortMappingInput,
        defaultPort: answers.defaultPort,
      }

      const validation = validateProjectConfig(newConfig)
      if (!validation.valid) {
        console.log('\nValidation errors:')
        validation.errors.forEach((e) => console.log(`  - ${e}`))
        return
      }

      await saveProjectConfig(answers.key, newConfig)
      console.log(`\nProject "${answers.name}" saved!\n`)
      PROJECT_CONFIGS = await loadProjectConfigs()
    }

    // Read all AWS profiles first
    const allProfiles = await readAwsConfig()

    if (allProfiles.length === 0) {
      return
    }

    // Step 1: Filter projects based on available profiles
    const projectChoices = Object.entries(PROJECT_CONFIGS)
      .filter(([_key, config]) => {
        const matchingProfiles = getProfilesForProject(
          allProfiles,
          config,
          PROJECT_CONFIGS,
        )
        return matchingProfiles.length > 0
      })
      .map(([key, config]) => ({
        name: config.name,
        value: key,
      }))

    if (projectChoices.length === 0) {
      return
    }

    // Skip project selection if only one project available
    let projectKey
    if (projectChoices.length === 1) {
      projectKey = projectChoices[0].value
    } else {
      const projectAnswer = await inquirer.default.prompt([
        {
          type: 'select',
          name: 'project',
          message: 'Please select the project:',
          choices: projectChoices,
        },
      ])
      projectKey = projectAnswer.project
    }

    const projectConfig = PROJECT_CONFIGS[projectKey]
    const { region, secretPrefix, envPortMapping, defaultPort } = projectConfig

    // Step 2: Get profiles for selected project
    const ENVS = getProfilesForProject(
      allProfiles,
      projectConfig,
      PROJECT_CONFIGS,
    )

    if (ENVS.length === 0) {
      return
    }

    const envAnswer = await inquirer.default.prompt([
      {
        type: 'select',
        name: 'ENV',
        message: 'Please select the environment:',
        choices: ENVS,
      },
    ])

    const ENV = envAnswer.ENV

    if (!PROFILE_SAFE_PATTERN.test(ENV)) {
      console.error('❌ Invalid profile name:', ENV)
      return
    }

    // Determine local port number
    const allEnvSuffixes = Object.keys(envPortMapping).sort(
      (a, b) => b.length - a.length,
    )
    const matchedSuffix =
      allEnvSuffixes.find((suffix) => ENV.endsWith(suffix)) ||
      allEnvSuffixes.find((suffix) => ENV === suffix)
    const portNumber = envPortMapping[matchedSuffix] || defaultPort

    // Get RDS credentials from Secrets Manager
    console.log('\n⏳ Getting credentials...')
    const secretsListCommand = `aws-vault exec ${ENV} -- aws secretsmanager list-secrets --region ${region} --query "SecretList[?contains(Name, '${secretPrefix}')].Name | [0]" --output text`
    const SECRET_NAME = await runCommand(secretsListCommand)

    if (!SECRET_NAME || SECRET_NAME === 'None') {
      console.error('❌ No secret found with prefix:', secretPrefix)
      return
    }

    const secretsGetCommand = `aws-vault exec ${ENV} -- aws secretsmanager get-secret-value --region ${region} --secret-id "${SECRET_NAME}" --query SecretString --output text`
    const secretString = await runCommand(secretsGetCommand)

    if (!secretString) {
      console.error('❌ Failed to retrieve secret value')
      return
    }

    let CREDENTIALS
    try {
      CREDENTIALS = JSON.parse(secretString)
      if (!CREDENTIALS.username || !CREDENTIALS.password) {
        throw new Error('Missing username or password in credentials')
      }
    } catch (_error) {
      console.error('❌ Failed to parse credentials')
      return
    }

    // Find bastion instance
    console.log('⏳ Finding bastion instance...')
    const instanceIdCommand = `aws-vault exec ${ENV} -- aws ec2 describe-instances --region ${region} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId] | [0][0]" --output text`
    const INSTANCE_ID = await runCommand(instanceIdCommand)

    if (!INSTANCE_ID || INSTANCE_ID === 'None') {
      console.error('❌ No running bastion instance found')
      return
    }

    if (!INSTANCE_ID_PATTERN.test(INSTANCE_ID)) {
      console.error('❌ Invalid instance ID format:', INSTANCE_ID)
      return
    }

    // Get RDS endpoint
    console.log('⏳ Getting RDS endpoint...')
    const RDS_ENDPOINT = await getRdsEndpoint(ENV, projectConfig)

    if (!RDS_ENDPOINT || RDS_ENDPOINT === 'None') {
      console.error('❌ Failed to find RDS endpoint')
      return
    }

    if (!HOSTNAME_PATTERN.test(RDS_ENDPOINT)) {
      console.error('❌ Invalid RDS endpoint format:', RDS_ENDPOINT)
      return
    }

    // Get RDS port (remote port)
    const rdsPort = await getRdsPort(ENV, projectConfig)

    // Print connection details
    console.log('\n✅ Connection details:')
    console.log(`   Host:     localhost`)
    console.log(`   Port:     ${portNumber}`)
    console.log(`   Username: ${CREDENTIALS.username}`)
    console.log(`   Password: ${CREDENTIALS.password}`)
    console.log(`   Database: ${projectConfig.database}`)
    console.log(`\n⏳ Starting port forwarding...`)
    console.log('   Press Ctrl+C to disconnect\n')

    await startPortForwardingWithConfig(
      ENV,
      INSTANCE_ID,
      RDS_ENDPOINT,
      portNumber,
      rdsPort,
      region,
    )
  } catch (_error) {
    setImmediate(() => {
      throw new Error('Forcing exit due to unhandled error')
    })
  }
}

// Only run main() when executed directly (not when imported as a module)
const isMainModule =
  process.argv[1] &&
  (process.argv[1].endsWith('connect.js') ||
    process.argv[1].endsWith('rds_ssm_connect'))

if (isMainModule) {
  main().catch((_error) => {})
}

// Exports for GUI adapter
export {
  readAwsConfig,
  getProfilesForProject,
  getAvailableProjects,
  getProfilesForProjectKey,
  getConnectionCredentials,
  findBastionInstance,
  getRdsEndpoint,
  getRdsPort,
  getDefaultPortForEngine,
  getLocalPort,
  connect,
  ipcEmitter,
  loadProjectConfigs,
  RETRY_CONFIG,
}
