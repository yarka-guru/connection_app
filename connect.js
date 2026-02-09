#!/usr/bin/env node

import inquirer from 'inquirer'
import fs from 'fs/promises'
import os from 'os'
import path from 'path'
import { exec } from 'child_process'
import { promisify } from 'util'
import { PROJECT_CONFIGS } from './envPortMapping.js'

const execAsync = promisify(exec)

// Configuration constants
const RETRY_CONFIG = {
  BASTION_WAIT_MAX_RETRIES: 20,
  BASTION_WAIT_RETRY_DELAY_MS: 15000,
  PORT_FORWARDING_MAX_RETRIES: 2,
  SSM_AGENT_READY_WAIT_MS: 10000
}

// Store active child processes for cleanup
let activeChildProcesses = []

// Handle graceful shutdown
function setupProcessCleanup () {
  const cleanup = () => {
    console.log('\nCleaning up active connections...')
    activeChildProcesses.forEach(child => {
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

async function readAwsConfig () {
  const awsConfigPath = path.join(os.homedir(), '.aws', 'config')
  try {
    const awsConfig = await fs.readFile(awsConfigPath, { encoding: 'utf-8' })
    return awsConfig
      .split(/\r?\n/)
      .map(line => line.trim())
      .filter(line => line.startsWith('[') && line.endsWith(']'))
      .map(line => line.slice(1, -1))
      .map(line => line.replace('profile ', '').trim())
  } catch (error) {
    console.error('Error reading AWS config file:', error)
    return []
  }
}

async function runCommand (command) {
  try {
    const { stdout } = await execAsync(command)
    return stdout.trim()
  } catch (error) {
    console.error(`Error executing command: ${command}`)
    console.error(error)
    return null
  }
}

async function sleep (ms) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

async function terminateBastionInstance (ENV, instanceId, region) {
  console.log(`Terminating disconnected bastion instance: ${instanceId}`)
  const terminateCommand = `aws-vault exec ${ENV} -- aws ec2 terminate-instances --region ${region} --instance-ids ${instanceId}`
  await runCommand(terminateCommand)
  console.log('Bastion instance terminated. ASG will spin up a new instance...')
}

async function waitForNewBastionInstance (ENV, oldInstanceId, region, maxRetries = RETRY_CONFIG.BASTION_WAIT_MAX_RETRIES, retryDelay = RETRY_CONFIG.BASTION_WAIT_RETRY_DELAY_MS) {
  console.log('Waiting for new bastion instance to be ready...')

  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    console.log(`Checking for new bastion instance (attempt ${attempt}/${maxRetries})...`)

    const instanceIdCommand = `aws-vault exec ${ENV} -- aws ec2 describe-instances --region ${region} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId] | [0][0]" --output text`
    const newInstanceId = await runCommand(instanceIdCommand)

    if (newInstanceId && newInstanceId !== oldInstanceId && newInstanceId !== 'None') {
      console.log(`New bastion instance found: ${newInstanceId}`)

      // Verify SSM agent is ready
      const isReady = await waitForSSMAgentReady(ENV, newInstanceId, region)
      if (isReady) {
        return newInstanceId
      } else {
        console.log('SSM agent not ready yet, will retry...')
      }
    }

    if (attempt < maxRetries) {
      await sleep(retryDelay)
    }
  }

  return null
}

async function waitForSSMAgentReady (ENV, instanceId, region, maxRetries = 10, retryDelay = 3000) {
  console.log('Verifying SSM agent status...')

  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    const ssmStatusCommand = `aws-vault exec ${ENV} -- aws ssm describe-instance-information --region ${region} --filters "Key=InstanceIds,Values=${instanceId}" --query "InstanceInformationList[0].PingStatus" --output text`
    const status = await runCommand(ssmStatusCommand)

    if (status === 'Online') {
      console.log('SSM agent is online and ready.')
      // Additional wait for agent to stabilize
      await sleep(RETRY_CONFIG.SSM_AGENT_READY_WAIT_MS)
      return true
    }

    console.log(`SSM agent status: ${status || 'Unknown'} (attempt ${attempt}/${maxRetries})`)

    if (attempt < maxRetries) {
      await sleep(retryDelay)
    }
  }

  console.log('Warning: SSM agent did not report Online status, but proceeding anyway.')
  return false
}

function monitorPortForwardingSession (child) {
  const state = {
    stderrOutput: '',
    targetNotConnectedError: false,
    sessionEstablished: false
  }

  child.stdout.on('data', (data) => {
    const output = data.toString()

    // Detect when session is actually established
    if (output.includes('Starting session with SessionId:')) {
      state.sessionEstablished = true
      console.log('Port forwarding session established.')
      console.log('Press Ctrl+C to end the session.')
    }

    if (!output.includes('Starting session with SessionId:') && !output.includes('Port 5433 opened for sessionId')) {
      console.log(output)
    }
  })

  child.stderr.on('data', (data) => {
    const errorOutput = data.toString()
    state.stderrOutput += errorOutput

    // Check for TargetNotConnected error
    if (errorOutput.includes('TargetNotConnected') || errorOutput.includes('is not connected')) {
      state.targetNotConnectedError = true
    }

    console.error(errorOutput)
  })

  return state
}

async function handleTargetNotConnectedError (ENV, instanceId, rdsEndpoint, portNumber, remotePort, region, retryCount, maxRetries) {
  console.log(`\nDetected TargetNotConnected error. Attempting recovery (retry ${retryCount + 1}/${maxRetries})...`)

  // Terminate the disconnected instance
  await terminateBastionInstance(ENV, instanceId, region)

  // Wait for new instance to be ready
  const newInstanceId = await waitForNewBastionInstance(ENV, instanceId, region)

  if (!newInstanceId) {
    throw new Error('Failed to find new bastion instance after waiting.')
  }

  // Retry with new instance
  console.log('Retrying port forwarding with new bastion instance...')
  return await startPortForwardingWithConfig(ENV, newInstanceId, rdsEndpoint, portNumber, remotePort, region, retryCount + 1, maxRetries)
}

function executePortForwardingCommand (ENV, instanceId, rdsEndpoint, portNumber, remotePort, region) {
  const portForwardingCommand = `aws-vault exec ${ENV} -- aws ssm start-session --region ${region} --target ${instanceId} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${rdsEndpoint},portNumber='${remotePort}',localPortNumber='${portNumber}'" --cli-connect-timeout 0`

  console.log('Starting port forwarding session...')
  const child = exec(portForwardingCommand)

  // Register child process for cleanup
  activeChildProcesses.push(child)

  return child
}

async function startPortForwardingWithConfig (ENV, instanceId, rdsEndpoint, portNumber, remotePort, region, retryCount = 0, maxRetries = RETRY_CONFIG.PORT_FORWARDING_MAX_RETRIES) {
  return new Promise((resolve, reject) => {
    const child = executePortForwardingCommand(ENV, instanceId, rdsEndpoint, portNumber, remotePort, region)
    const sessionState = monitorPortForwardingSession(child)

    child.on('close', async (code) => {
      // Remove from active processes
      activeChildProcesses = activeChildProcesses.filter(p => p !== child)

      try {
        // Handle TargetNotConnected error with retry
        if (code === 254 && sessionState.targetNotConnectedError && retryCount < maxRetries) {
          await handleTargetNotConnectedError(ENV, instanceId, rdsEndpoint, portNumber, remotePort, region, retryCount, maxRetries)
          resolve()
        } else if (code !== 0) {
          console.log(`Port forwarding session ended with code ${code}`)
          reject(new Error(`Port forwarding failed with code ${code}`))
        } else {
          console.log(`Port forwarding session ended with code ${code}`)
          resolve()
        }
      } catch (error) {
        console.error('Error during recovery:', error)
        reject(error)
      }
    })
  })
}

// Legacy function for backward compatibility
async function startPortForwarding (ENV, instanceId, rdsEndpoint, portNumber, retryCount = 0, maxRetries = RETRY_CONFIG.PORT_FORWARDING_MAX_RETRIES) {
  return startPortForwardingWithConfig(ENV, instanceId, rdsEndpoint, portNumber, '5432', 'us-east-2', retryCount, maxRetries)
}

async function getRdsEndpoint (ENV, projectConfig) {
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

async function getRdsPort (ENV, projectConfig) {
  const { region, rdsType, rdsPattern } = projectConfig

  if (rdsType === 'cluster') {
    const portCommand = `aws-vault exec ${ENV} -- aws rds describe-db-clusters --region ${region} --query "DBClusters[?Status=='available' && ends_with(DBClusterIdentifier, '${rdsPattern}')].Port | [0]" --output text`
    return await runCommand(portCommand) || '5432'
  } else {
    const portCommand = `aws-vault exec ${ENV} -- aws rds describe-db-instances --region ${region} --query "DBInstances[?DBInstanceStatus=='available' && contains(DBInstanceIdentifier, '${rdsPattern}')].Endpoint.Port | [0]" --output text`
    return await runCommand(portCommand) || '5432'
  }
}

function getProfilesForProject (allProfiles, projectConfig, allProjectConfigs) {
  const { profileFilter } = projectConfig

  if (profileFilter) {
    // Project has explicit filter - return profiles starting with filter
    return allProfiles.filter(env => env.startsWith(profileFilter))
  } else {
    // No filter (legacy project like TLN) - return profiles that don't match any other project's filter
    const otherFilters = Object.values(allProjectConfigs)
      .filter(config => config.profileFilter)
      .map(config => config.profileFilter)

    return allProfiles.filter(env =>
      !otherFilters.some(filter => env.startsWith(filter))
    )
  }
}

async function main () {
  // Setup process cleanup handlers
  setupProcessCleanup()

  try {
    // Read all AWS profiles first
    const allProfiles = await readAwsConfig()

    if (allProfiles.length === 0) {
      console.error('No environments found in AWS config file.')
      return
    }

    // Step 1: Filter projects based on available profiles
    const projectChoices = Object.entries(PROJECT_CONFIGS)
      .filter(([key, config]) => {
        const matchingProfiles = getProfilesForProject(allProfiles, config, PROJECT_CONFIGS)
        return matchingProfiles.length > 0
      })
      .map(([key, config]) => ({
        name: config.name,
        value: key
      }))

    if (projectChoices.length === 0) {
      console.error('No projects available for the configured AWS profiles.')
      return
    }

    // Skip project selection if only one project available
    let projectKey
    if (projectChoices.length === 1) {
      projectKey = projectChoices[0].value
      console.log(`Auto-selected project: ${projectChoices[0].name}`)
    } else {
      const projectAnswer = await inquirer.prompt([
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
    const { region, database, secretPrefix, envPortMapping, defaultPort } = projectConfig

    // Step 2: Get profiles for selected project
    let ENVS = getProfilesForProject(allProfiles, projectConfig, PROJECT_CONFIGS)

    if (ENVS.length === 0) {
      console.error('No AWS profiles found for this project.')
      return
    }

    const envAnswer = await inquirer.prompt([
      {
        type: 'select',
        name: 'ENV',
        message: 'Please select the environment:',
        choices: ENVS,
      },
    ])

    const ENV = envAnswer.ENV

    // Determine local port number
    const allEnvSuffixes = Object.keys(envPortMapping).sort((a, b) => b.length - a.length)
    const matchedSuffix = allEnvSuffixes.find(suffix => ENV.endsWith(suffix)) ||
                          allEnvSuffixes.find(suffix => ENV === suffix)
    const portNumber = envPortMapping[matchedSuffix] || defaultPort

    // Get RDS credentials from Secrets Manager
    const secretsListCommand = `aws-vault exec ${ENV} -- aws secretsmanager list-secrets --region ${region} --query "SecretList[?starts_with(Name, '${secretPrefix}')].Name | [0]" --output text`
    const SECRET_NAME = await runCommand(secretsListCommand)

    if (!SECRET_NAME || SECRET_NAME === 'None') {
      console.error(`No secret found with name starting with '${secretPrefix}'.`)
      return
    }

    const secretsGetCommand = `aws-vault exec ${ENV} -- aws secretsmanager get-secret-value --region ${region} --secret-id "${SECRET_NAME}" --query SecretString --output text`
    const secretString = await runCommand(secretsGetCommand)

    if (!secretString) {
      console.error('Failed to retrieve secret value from Secrets Manager.')
      return
    }

    let CREDENTIALS
    try {
      CREDENTIALS = JSON.parse(secretString)
      if (!CREDENTIALS.username || !CREDENTIALS.password) {
        throw new Error('Missing username or password in credentials')
      }
    } catch (error) {
      console.error('Failed to parse credentials from Secrets Manager:', error.message)
      return
    }

    const USERNAME = CREDENTIALS.username
    const PASSWORD = CREDENTIALS.password

    console.log(`\nYour connection details:
      Host: localhost
      Port: ${portNumber}
      User: ${USERNAME}
      Database: ${database}
      Password: ${PASSWORD}\n`)

    // Find bastion instance
    const instanceIdCommand = `aws-vault exec ${ENV} -- aws ec2 describe-instances --region ${region} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId] | [0][0]" --output text`
    const INSTANCE_ID = await runCommand(instanceIdCommand)

    if (!INSTANCE_ID || INSTANCE_ID === 'None') {
      console.error('Failed to find a running instance with tag Name=*bastion*.')
      return
    }

    // Get RDS endpoint
    const RDS_ENDPOINT = await getRdsEndpoint(ENV, projectConfig)

    if (!RDS_ENDPOINT || RDS_ENDPOINT === 'None') {
      console.error('Failed to find the RDS endpoint.')
      return
    }

    // Get RDS port (remote port)
    const rdsPort = await getRdsPort(ENV, projectConfig)

    await startPortForwardingWithConfig(ENV, INSTANCE_ID, RDS_ENDPOINT, portNumber, rdsPort, region)
  } catch (error) {
    console.error(`Error: ${error.message}`)
    console.error('Exiting due to unhandled error')
    setImmediate(() => {
      throw new Error('Forcing exit due to unhandled error')
    })
  }
}

main().catch((error) => {
  console.error('Unhandled error in main function:', error)
})