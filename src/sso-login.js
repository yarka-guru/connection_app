import { createHash } from 'node:crypto'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import {
  CreateTokenCommand,
  RegisterClientCommand,
  SSOOIDCClient,
  StartDeviceAuthorizationCommand,
} from '@aws-sdk/client-sso-oidc'
import { getSsoConfig } from './credential-resolver.js'

const CLIENT_NAME = 'rds-connect-app'
const CLIENT_TYPE = 'public'
const TOKEN_EXPIRY_BUFFER_MS = 5 * 60 * 1000 // 5 minutes
const POLL_TIMEOUT_MS = 10 * 60 * 1000 // 10 minutes max

/**
 * Compute the cache filepath for an SSO token.
 * AWS CLI uses SHA1 of the session key (startUrl or session name).
 */
export function getSsoTokenFilepath(key) {
  const hash = createHash('sha1').update(key).digest('hex')
  return path.join(os.homedir(), '.aws', 'sso', 'cache', `${hash}.json`)
}

/**
 * Read and parse a cached SSO token. Returns null if missing or malformed.
 */
export async function readSsoToken(key) {
  const filepath = getSsoTokenFilepath(key)
  try {
    const content = await fs.readFile(filepath, 'utf-8')
    const token = JSON.parse(content)
    if (!token.accessToken || !token.expiresAt) {
      return null
    }
    return token
  } catch {
    return null
  }
}

/**
 * Check if a cached SSO token is still valid (with buffer).
 */
export function isSsoTokenValid(token) {
  if (!token || !token.expiresAt) return false
  const expiresAt = new Date(token.expiresAt).getTime()
  return expiresAt > Date.now() + TOKEN_EXPIRY_BUFFER_MS
}

/**
 * Write an SSO token to the AWS CLI-compatible cache location.
 */
export async function writeSsoToken(key, tokenData) {
  const filepath = getSsoTokenFilepath(key)
  const dir = path.dirname(filepath)
  await fs.mkdir(dir, { recursive: true, mode: 0o700 })
  await fs.writeFile(filepath, JSON.stringify(tokenData, null, 2), {
    encoding: 'utf-8',
    mode: 0o600,
  })
}

// Cached OIDC client registration (valid ~90 days, keyed by region)
const clientRegistrationCache = new Map()

/**
 * Register an OIDC client with AWS SSO, or return cached registration.
 */
export async function registerClient(ssoOidcClient, ssoRegion) {
  const cached = clientRegistrationCache.get(ssoRegion)
  if (cached && cached.clientSecretExpiresAt * 1000 > Date.now()) {
    return cached
  }

  const response = await ssoOidcClient.send(
    new RegisterClientCommand({
      clientName: CLIENT_NAME,
      clientType: CLIENT_TYPE,
    }),
  )
  const registration = {
    clientId: response.clientId,
    clientSecret: response.clientSecret,
    clientSecretExpiresAt: response.clientSecretExpiresAt,
  }
  clientRegistrationCache.set(ssoRegion, registration)
  return registration
}

/**
 * Start the device authorization flow.
 */
export async function startDeviceAuthorization(
  ssoOidcClient,
  clientId,
  clientSecret,
  startUrl,
) {
  const response = await ssoOidcClient.send(
    new StartDeviceAuthorizationCommand({
      clientId,
      clientSecret,
      startUrl,
    }),
  )
  return {
    deviceCode: response.deviceCode,
    userCode: response.userCode,
    verificationUri: response.verificationUri,
    verificationUriComplete: response.verificationUriComplete,
    expiresIn: response.expiresIn,
    interval: response.interval || 5,
  }
}

/**
 * Poll for token after user authorizes in browser.
 * Handles AuthorizationPendingException, SlowDownException, ExpiredTokenException.
 */
export async function pollForToken(
  ssoOidcClient,
  clientId,
  clientSecret,
  deviceCode,
  interval,
  expiresIn,
  onEvent,
) {
  const deadline = Date.now() + Math.min(expiresIn * 1000, POLL_TIMEOUT_MS)
  let pollInterval = interval * 1000

  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, pollInterval))

    try {
      const response = await ssoOidcClient.send(
        new CreateTokenCommand({
          clientId,
          clientSecret,
          grantType: 'urn:ietf:params:oauth:grant-type:device_code',
          deviceCode,
        }),
      )

      return {
        accessToken: response.accessToken,
        expiresAt: new Date(
          Date.now() + response.expiresIn * 1000,
        ).toISOString(),
      }
    } catch (err) {
      const errorName = err.name || err.constructor?.name || ''

      if (errorName === 'AuthorizationPendingException') {
        onEvent?.('sso-status', {
          message: 'Waiting for authorization in browser...',
        })
        continue
      }

      if (errorName === 'SlowDownException') {
        pollInterval += 5000
        continue
      }

      if (errorName === 'ExpiredTokenException') {
        throw new Error(
          'SSO authorization expired. Please try connecting again.',
        )
      }

      throw err
    }
  }

  throw new Error('SSO authorization timed out. Please try connecting again.')
}

/**
 * Orchestrate the full SSO login flow:
 * register client → start device auth → open browser → poll for token → cache
 */
export async function performSsoLogin(ssoStartUrl, ssoRegion, options = {}) {
  const { onEvent, onOpenUrl } = options

  const ssoOidcClient = new SSOOIDCClient({ region: ssoRegion })

  onEvent?.('sso-status', { message: 'Registering SSO client...' })
  const { clientId, clientSecret } = await registerClient(ssoOidcClient, ssoRegion)

  onEvent?.('sso-status', { message: 'Starting device authorization...' })
  const deviceAuth = await startDeviceAuthorization(
    ssoOidcClient,
    clientId,
    clientSecret,
    ssoStartUrl,
  )

  // Signal to open browser — only allow HTTPS URLs
  const urlToOpen = deviceAuth.verificationUriComplete || deviceAuth.verificationUri
  if (!urlToOpen || !urlToOpen.startsWith('https://')) {
    throw new Error(
      `SSO returned an invalid verification URL: ${urlToOpen || '(empty)'}`,
    )
  }
  onEvent?.('sso-status', {
    message: 'Waiting for SSO authorization in browser...',
  })
  onOpenUrl?.(urlToOpen)

  // Poll until user authorizes
  const tokenData = await pollForToken(
    ssoOidcClient,
    clientId,
    clientSecret,
    deviceAuth.deviceCode,
    deviceAuth.interval,
    deviceAuth.expiresIn,
    onEvent,
  )

  // Add metadata for cache key identification
  tokenData.startUrl = ssoStartUrl
  tokenData.region = ssoRegion

  // Write token to AWS CLI-compatible cache
  await writeSsoToken(ssoStartUrl, tokenData)

  onEvent?.('sso-status', { message: 'SSO login successful' })
  return tokenData
}

/**
 * High-level entry: check if profile needs SSO login, perform if needed.
 * Returns true if SSO was handled (or not needed), throws on failure.
 */
export async function ensureSsoSession(profile, options = {}) {
  const ssoConfig = await getSsoConfig(profile)

  // Not an SSO profile — nothing to do
  if (!ssoConfig) return true

  const { startUrl, region } = ssoConfig

  // Check cached token
  const cachedToken = await readSsoToken(startUrl)
  if (isSsoTokenValid(cachedToken)) {
    options.onEvent?.('sso-status', {
      message: 'SSO session valid',
    })
    return true
  }

  // Token expired or missing — perform login
  options.onEvent?.('sso-status', {
    message: 'SSO session expired, starting login...',
  })
  await performSsoLogin(startUrl, region, options)
  return true
}
