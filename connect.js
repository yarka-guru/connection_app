#!/usr/bin/env node

import { exec } from 'node:child_process'
import { EventEmitter } from 'node:events'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { promisify } from 'node:util'
import { PROJECT_CONFIGS } from './envPortMapping.js'

// Package info for version checking
const packageJson = { name: 'rds_ssm_connect', version: '1.6.2' }

const execAsync = promisify(exec)

// Event emitter for IPC communication
const ipcEmitter = new EventEmitter()

// Configuration constants
const RETRY_CONFIG = {
  BASTION_WAIT_MAX_RETRIES: 20,
  BASTION_WAIT_RETRY_DELAY_MS: 15000,
  PORT_FORWARDING_MAX_RETRIES: 2,
  SSM_AGENT_READY_WAIT_MS: 10000,
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

// Handle graceful shutdown
function setupProcessCleanup() {
  const cleanup = () => {
    activeChildProcesses.forEach((child) => {
      if (child && !child.killed) {
        child.kill('SIGTERM')
      }
    })
    process.exit(0)
  }

  process.on('SIGINT', cleanup)
  process.on('SIGTERM', cleanup)
  process.on('exit', cleanup)
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
  const child = exec(portForwardingCommand)

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

  if (rdsType === 'cluster') {
    const portCommand = `aws-vault exec ${ENV} -- aws rds describe-db-clusters --region ${region} --query "DBClusters[?Status=='available' && ends_with(DBClusterIdentifier, '${rdsPattern}')].Port | [0]" --output text`
    return (await runCommand(portCommand)) || '5432'
  } else {
    const portCommand = `aws-vault exec ${ENV} -- aws rds describe-db-instances --region ${region} --query "DBInstances[?DBInstanceStatus=='available' && contains(DBInstanceIdentifier, '${rdsPattern}')].Endpoint.Port | [0]" --output text`
    return (await runCommand(portCommand)) || '5432'
  }
}

function getProfilesForProject(allProfiles, projectConfig, allProjectConfigs) {
  const { profileFilter } = projectConfig

  if (profileFilter) {
    // Project has explicit filter - return profiles starting with filter
    return allProfiles.filter((env) => env.startsWith(profileFilter))
  } else {
    // No filter (legacy project like TLN) - return profiles that don't match any other project's filter
    const otherFilters = Object.values(allProjectConfigs)
      .filter((config) => config.profileFilter)
      .map((config) => config.profileFilter)

    return allProfiles.filter(
      (env) => !otherFilters.some((filter) => env.startsWith(filter)),
    )
  }
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

  const secretsListCommand = `aws-vault exec ${ENV} -- aws secretsmanager list-secrets --region ${region} --query "SecretList[?starts_with(Name, '${secretPrefix}')].Name | [0]" --output text`
  const SECRET_NAME = await runCommand(secretsListCommand)

  if (!SECRET_NAME || SECRET_NAME === 'None') {
    throw new Error(
      `No secret found with name starting with '${secretPrefix}'.`,
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
  const projectConfig = PROJECT_CONFIGS[projectKey]

  if (!projectConfig) {
    throw new Error(`Unknown project: ${projectKey}`)
  }

  return getProfilesForProject(allProfiles, projectConfig, PROJECT_CONFIGS)
}

// Connect to RDS through bastion - returns connection info and control object
async function connect(projectKey, profile, options = {}) {
  const projectConfig = PROJECT_CONFIGS[projectKey]
  if (!projectConfig) {
    throw new Error(`Unknown project: ${projectKey}`)
  }

  const { region, database } = projectConfig
  // Use provided localPort or fall back to computed port from profile
  const localPort = options.localPort || getLocalPort(profile, projectConfig)

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
  const instanceId = await findBastionInstance(profile, region)

  emit('status', { message: 'Getting RDS endpoint...' })
  const rdsEndpoint = await getRdsEndpoint(profile, projectConfig)

  if (!rdsEndpoint || rdsEndpoint === 'None') {
    throw new Error('Failed to find the RDS endpoint.')
  }

  emit('status', { message: 'Getting RDS port...' })
  const rdsPort = await getRdsPort(profile, projectConfig)

  const connectionInfo = {
    host: 'localhost',
    port: localPort,
    username: credentials.username,
    password: credentials.password,
    database,
    rdsEndpoint,
    instanceId,
  }

  emit('credentials', connectionInfo)
  emit('status', { message: 'Starting port forwarding...' })

  // Start port forwarding
  const portForwardingPromise = startPortForwardingWithConfig(
    profile,
    instanceId,
    rdsEndpoint,
    localPort,
    rdsPort,
    region,
    0,
    RETRY_CONFIG.PORT_FORWARDING_MAX_RETRIES,
  )

  // Return connection control object
  return {
    connectionInfo,
    disconnect: () => {
      // Kill all active child processes
      activeChildProcesses.forEach((child) => {
        if (child && !child.killed) {
          child.kill('SIGTERM')
        }
      })
      activeChildProcesses = []
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

    // Determine local port number
    const allEnvSuffixes = Object.keys(envPortMapping).sort(
      (a, b) => b.length - a.length,
    )
    const matchedSuffix =
      allEnvSuffixes.find((suffix) => ENV.endsWith(suffix)) ||
      allEnvSuffixes.find((suffix) => ENV === suffix)
    const portNumber = envPortMapping[matchedSuffix] || defaultPort

    // Get RDS credentials from Secrets Manager
    const secretsListCommand = `aws-vault exec ${ENV} -- aws secretsmanager list-secrets --region ${region} --query "SecretList[?starts_with(Name, '${secretPrefix}')].Name | [0]" --output text`
    const SECRET_NAME = await runCommand(secretsListCommand)

    if (!SECRET_NAME || SECRET_NAME === 'None') {
      return
    }

    const secretsGetCommand = `aws-vault exec ${ENV} -- aws secretsmanager get-secret-value --region ${region} --secret-id "${SECRET_NAME}" --query SecretString --output text`
    const secretString = await runCommand(secretsGetCommand)

    if (!secretString) {
      return
    }

    let CREDENTIALS
    try {
      CREDENTIALS = JSON.parse(secretString)
      if (!CREDENTIALS.username || !CREDENTIALS.password) {
        throw new Error('Missing username or password in credentials')
      }
    } catch (_error) {
      return
    }

    const _USERNAME = CREDENTIALS.username
    const _PASSWORD = CREDENTIALS.password

    // Find bastion instance
    const instanceIdCommand = `aws-vault exec ${ENV} -- aws ec2 describe-instances --region ${region} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId] | [0][0]" --output text`
    const INSTANCE_ID = await runCommand(instanceIdCommand)

    if (!INSTANCE_ID || INSTANCE_ID === 'None') {
      return
    }

    // Get RDS endpoint
    const RDS_ENDPOINT = await getRdsEndpoint(ENV, projectConfig)

    if (!RDS_ENDPOINT || RDS_ENDPOINT === 'None') {
      return
    }

    // Get RDS port (remote port)
    const rdsPort = await getRdsPort(ENV, projectConfig)

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
  getLocalPort,
  connect,
  ipcEmitter,
  PROJECT_CONFIGS,
  RETRY_CONFIG,
}
