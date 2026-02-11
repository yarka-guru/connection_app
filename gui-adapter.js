#!/usr/bin/env node

/**
 * GUI Adapter - JSON IPC bridge for Tauri sidecar
 *
 * Reads JSON commands from stdin, dispatches to connect.js functions,
 * and writes JSON responses/events to stdout.
 *
 * Commands:
 * - list-projects: Get available projects
 * - list-profiles: Get profiles for a project
 * - connect: Connect to RDS through bastion (supports multiple simultaneous connections)
 * - disconnect: Disconnect a specific or all sessions
 * - disconnect-all: Disconnect all active sessions
 */

import { randomUUID } from 'node:crypto'
import net from 'node:net'
import readline from 'node:readline'
import {
  connect,
  getAvailableProjects,
  getLocalPort,
  getProfilesForProjectKey,
  loadProjectConfigs,
} from './connect.js'
import {
  deleteProjectConfig,
  saveProjectConfig,
  validateProjectConfig,
} from './configLoader.js'

// Active connections Map - connectionId -> connection control object
const activeConnections = new Map()

// Send JSON response to stdout
function sendResponse(id, type, data) {
  process.stdout.write(JSON.stringify({ id, type, ...data }) + '\n')
}

// Send event to stdout
function sendEvent(event, data) {
  process.stdout.write(JSON.stringify({ type: 'event', event, ...data }) + '\n')
}

// Check if a port is available
async function isPortAvailable(port) {
  return new Promise((resolve) => {
    const server = net.createServer()
    server.once('error', () => resolve(false))
    server.once('listening', () => {
      server.close()
      resolve(true)
    })
    server.listen(port, '127.0.0.1')
  })
}

// Handle incoming commands
async function handleCommand(command) {
  const { id, action, ...params } = command

  try {
    switch (action) {
      case 'list-projects': {
        const projects = await getAvailableProjects()
        sendResponse(id, 'success', { projects })
        break
      }

      case 'list-profiles': {
        const { projectKey } = params
        if (!projectKey) {
          sendResponse(id, 'error', { message: 'projectKey is required' })
          break
        }
        const profiles = await getProfilesForProjectKey(projectKey)
        sendResponse(id, 'success', { profiles })
        break
      }

      case 'connect': {
        const { projectKey, profile, localPort, usedPorts = [] } = params
        if (!projectKey || !profile) {
          sendResponse(id, 'error', {
            message: 'projectKey and profile are required',
          })
          break
        }

        // Generate unique connection ID
        const connectionId = `conn_${randomUUID().slice(0, 8)}`

        // Determine port to use
        const configs = await loadProjectConfigs()
        const projectConfig = configs[projectKey]
        if (!projectConfig) {
          sendResponse(id, 'error', {
            message: `Unknown project: ${projectKey}`,
          })
          break
        }

        const portToUse = localPort || getLocalPort(profile, projectConfig)
        const portNum = parseInt(portToUse, 10)

        // Strict port check — never silently increment
        const allUsedPorts = new Set([
          ...usedPorts.map((p) => parseInt(p, 10)),
          ...Array.from(activeConnections.values())
            .map((c) => parseInt(c.connectionInfo?.port, 10))
            .filter(Boolean),
        ])
        if (allUsedPorts.has(portNum) || !(await isPortAvailable(portNum))) {
          sendResponse(id, 'error', {
            message: `Port ${portToUse} is not available. Close the application using it or change the port in project settings.`,
          })
          break
        }

        // Start new connection with the determined port
        const connectionControl = await connect(projectKey, profile, {
          localPort: portToUse,
          onEvent: (event, data) => {
            sendEvent(event, { ...data, connectionId })
          },
        })

        // Store in active connections
        activeConnections.set(connectionId, connectionControl)

        sendResponse(id, 'success', {
          connectionId,
          connectionInfo: connectionControl.connectionInfo,
        })

        // Set up connection close handler
        connectionControl
          .waitForClose()
          .then(() => {
            sendEvent('disconnected', { connectionId, reason: 'session_ended' })
            activeConnections.delete(connectionId)
          })
          .catch((error) => {
            sendEvent('error', { connectionId, message: error.message })
            sendEvent('disconnected', { connectionId, reason: 'error' })
            activeConnections.delete(connectionId)
          })

        break
      }

      case 'disconnect': {
        const { connectionId } = params

        if (connectionId) {
          // Disconnect specific connection
          const connection = activeConnections.get(connectionId)
          if (connection) {
            connection.disconnect()
            activeConnections.delete(connectionId)
            sendResponse(id, 'success', {
              message: `Disconnected ${connectionId}`,
            })
          } else {
            sendResponse(id, 'success', {
              message: `Connection ${connectionId} not found`,
            })
          }
        } else {
          // Disconnect all connections (legacy behavior)
          for (const [_connId, connection] of activeConnections) {
            connection.disconnect()
          }
          activeConnections.clear()
          sendResponse(id, 'success', {
            message: 'Disconnected all connections',
          })
        }
        break
      }

      case 'disconnect-all': {
        for (const [_connId, connection] of activeConnections) {
          connection.disconnect()
        }
        activeConnections.clear()
        sendResponse(id, 'success', { message: 'Disconnected all connections' })
        break
      }

      case 'status': {
        const connections = Array.from(activeConnections.entries()).map(
          ([connId, conn]) => ({
            connectionId: connId,
            connectionInfo: conn.connectionInfo,
          }),
        )
        sendResponse(id, 'success', {
          connectionCount: activeConnections.size,
          connections,
        })
        break
      }

      case 'list-project-configs': {
        const configs = await loadProjectConfigs()
        sendResponse(id, 'success', { configs })
        break
      }

      case 'save-project-config': {
        const { key, config } = params
        if (!key || !config) {
          sendResponse(id, 'error', {
            message: 'key and config are required',
          })
          break
        }
        const validation = validateProjectConfig(config)
        if (!validation.valid) {
          sendResponse(id, 'error', {
            message: `Validation failed: ${validation.errors.join(', ')}`,
          })
          break
        }
        await saveProjectConfig(key, config)
        sendResponse(id, 'success', { message: 'Project config saved' })
        break
      }

      case 'delete-project-config': {
        const { key: deleteKey } = params
        if (!deleteKey) {
          sendResponse(id, 'error', { message: 'key is required' })
          break
        }
        await deleteProjectConfig(deleteKey)
        sendResponse(id, 'success', { message: 'Project config deleted' })
        break
      }

      case 'ping': {
        sendResponse(id, 'success', { message: 'pong' })
        break
      }

      default:
        sendResponse(id, 'error', { message: `Unknown action: ${action}` })
    }
  } catch (error) {
    sendResponse(id, 'error', { message: error.message })
  }
}

// Handle process signals for cleanup
function setupCleanup() {
  const cleanup = () => {
    for (const [_connId, connection] of activeConnections) {
      connection.disconnect()
    }
    activeConnections.clear()
    process.exit(0)
  }

  process.on('SIGINT', cleanup)
  process.on('SIGTERM', cleanup)
  process.on('exit', cleanup)
}

// Main entry point
async function main() {
  setupCleanup()

  // Signal that adapter is ready
  sendEvent('ready', { version: '2.0.0' })

  // Set up readline for stdin
  const rl = readline.createInterface({
    input: process.stdin,
    output: process.stdout,
    terminal: false,
  })

  rl.on('line', async (line) => {
    try {
      const command = JSON.parse(line)
      await handleCommand(command)
    } catch (error) {
      sendEvent('error', {
        message: `Failed to parse command: ${error.message}`,
      })
    }
  })

  rl.on('close', () => {
    for (const [_connId, connection] of activeConnections) {
      connection.disconnect()
    }
    activeConnections.clear()
    process.exit(0)
  })
}

main().catch((error) => {
  sendEvent('error', { message: `Adapter error: ${error.message}` })
  process.exit(1)
})
