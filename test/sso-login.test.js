import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, before, describe, it, mock } from 'node:test'

describe('sso-login', () => {
  let testDir
  let originalHome

  before(async () => {
    testDir = join(tmpdir(), `test-sso-login-${Date.now()}`)
    await mkdir(join(testDir, '.aws', 'sso', 'cache'), { recursive: true })
    await mkdir(join(testDir, '.aws'), { recursive: true })
    originalHome = process.env.HOME
  })

  after(async () => {
    process.env.HOME = originalHome
    await rm(testDir, { recursive: true, force: true })
  })

  describe('getSsoTokenFilepath()', () => {
    it('should compute SHA1-based filepath under ~/.aws/sso/cache/', async () => {
      process.env.HOME = testDir
      const { getSsoTokenFilepath } = await import(
        `../src/sso-login.js?t=filepath_${Date.now()}`
      )

      const key = 'https://my-sso.awsapps.com/start'
      const expectedHash = createHash('sha1').update(key).digest('hex')
      const result = getSsoTokenFilepath(key)

      assert.ok(result.endsWith(`${expectedHash}.json`))
      assert.ok(result.includes('.aws/sso/cache'))
    })

    it('should produce different paths for different keys', async () => {
      process.env.HOME = testDir
      const { getSsoTokenFilepath } = await import(
        `../src/sso-login.js?t=filepath2_${Date.now()}`
      )

      const path1 = getSsoTokenFilepath('https://a.awsapps.com/start')
      const path2 = getSsoTokenFilepath('https://b.awsapps.com/start')
      assert.notEqual(path1, path2)
    })
  })

  describe('isSsoTokenValid()', () => {
    it('should return true for a token expiring in the future', async () => {
      const { isSsoTokenValid } = await import(
        `../src/sso-login.js?t=valid1_${Date.now()}`
      )

      const token = {
        accessToken: 'test-token',
        expiresAt: new Date(Date.now() + 60 * 60 * 1000).toISOString(),
      }
      assert.equal(isSsoTokenValid(token), true)
    })

    it('should return false for an expired token', async () => {
      const { isSsoTokenValid } = await import(
        `../src/sso-login.js?t=valid2_${Date.now()}`
      )

      const token = {
        accessToken: 'test-token',
        expiresAt: new Date(Date.now() - 1000).toISOString(),
      }
      assert.equal(isSsoTokenValid(token), false)
    })

    it('should return false for a token expiring within 5-minute buffer', async () => {
      const { isSsoTokenValid } = await import(
        `../src/sso-login.js?t=valid3_${Date.now()}`
      )

      const token = {
        accessToken: 'test-token',
        expiresAt: new Date(Date.now() + 2 * 60 * 1000).toISOString(), // 2 min
      }
      assert.equal(isSsoTokenValid(token), false)
    })

    it('should return false for null token', async () => {
      const { isSsoTokenValid } = await import(
        `../src/sso-login.js?t=valid4_${Date.now()}`
      )
      assert.equal(isSsoTokenValid(null), false)
    })

    it('should return false for token missing expiresAt', async () => {
      const { isSsoTokenValid } = await import(
        `../src/sso-login.js?t=valid5_${Date.now()}`
      )
      assert.equal(isSsoTokenValid({ accessToken: 'test' }), false)
    })
  })

  describe('readSsoToken() and writeSsoToken()', () => {
    it('should round-trip a token through write and read', async () => {
      process.env.HOME = testDir
      const { readSsoToken, writeSsoToken } = await import(
        `../src/sso-login.js?t=rw1_${Date.now()}`
      )

      const key = `https://test-${Date.now()}.awsapps.com/start`
      const tokenData = {
        accessToken: 'my-access-token',
        expiresAt: new Date(Date.now() + 3600000).toISOString(),
        region: 'us-east-1',
        startUrl: key,
      }

      await writeSsoToken(key, tokenData)
      const result = await readSsoToken(key)

      assert.equal(result.accessToken, 'my-access-token')
      assert.equal(result.region, 'us-east-1')
      assert.equal(result.startUrl, key)
    })

    it('should return null for missing token', async () => {
      process.env.HOME = testDir
      const { readSsoToken } = await import(
        `../src/sso-login.js?t=rw2_${Date.now()}`
      )

      const result = await readSsoToken('https://nonexistent.awsapps.com/start')
      assert.equal(result, null)
    })

    it('should return null for malformed JSON', async () => {
      process.env.HOME = testDir
      const { getSsoTokenFilepath, readSsoToken } = await import(
        `../src/sso-login.js?t=rw3_${Date.now()}`
      )

      const key = `https://malformed-${Date.now()}.awsapps.com/start`
      const filepath = getSsoTokenFilepath(key)
      await writeFile(filepath, 'not-json', 'utf-8')

      const result = await readSsoToken(key)
      assert.equal(result, null)
    })

    it('should return null for token missing required fields', async () => {
      process.env.HOME = testDir
      const { getSsoTokenFilepath, readSsoToken } = await import(
        `../src/sso-login.js?t=rw4_${Date.now()}`
      )

      const key = `https://incomplete-${Date.now()}.awsapps.com/start`
      const filepath = getSsoTokenFilepath(key)
      await writeFile(filepath, JSON.stringify({ region: 'us-east-1' }), 'utf-8')

      const result = await readSsoToken(key)
      assert.equal(result, null)
    })
  })

  describe('getSsoConfig()', () => {
    it('should return SSO config for legacy SSO profile', async () => {
      const configContent = `[profile sso-legacy]
sso_start_url = https://my-sso.awsapps.com/start
sso_region = us-east-1
sso_account_id = 123456789012
sso_role_name = AdminRole
region = us-east-1
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)
      process.env.HOME = testDir

      const { getSsoConfig } = await import(
        `../src/credential-resolver.js?t=ssoconfig1_${Date.now()}`
      )
      const config = await getSsoConfig('sso-legacy')

      assert.equal(config.startUrl, 'https://my-sso.awsapps.com/start')
      assert.equal(config.region, 'us-east-1')
      assert.equal(config.accountId, '123456789012')
      assert.equal(config.roleName, 'AdminRole')
    })

    it('should return SSO config for sso-session profile', async () => {
      const configContent = `[profile sso-new]
sso_session = my-session
sso_account_id = 111222333444
sso_role_name = DevRole
region = us-west-2

[sso-session my-session]
sso_start_url = https://new-sso.awsapps.com/start
sso_region = us-west-2
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)
      process.env.HOME = testDir

      const { getSsoConfig } = await import(
        `../src/credential-resolver.js?t=ssoconfig2_${Date.now()}`
      )
      const config = await getSsoConfig('sso-new')

      assert.equal(config.startUrl, 'https://new-sso.awsapps.com/start')
      assert.equal(config.region, 'us-west-2')
      assert.equal(config.accountId, '111222333444')
      assert.equal(config.roleName, 'DevRole')
    })

    it('should return null for non-SSO profile', async () => {
      const configContent = `[profile regular]
region = us-east-1
role_arn = arn:aws:iam::123456789012:role/dev
source_profile = default
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)
      process.env.HOME = testDir

      const { getSsoConfig } = await import(
        `../src/credential-resolver.js?t=ssoconfig3_${Date.now()}`
      )
      const config = await getSsoConfig('regular')

      assert.equal(config, null)
    })

    it('should return null for nonexistent profile', async () => {
      process.env.HOME = testDir

      const { getSsoConfig } = await import(
        `../src/credential-resolver.js?t=ssoconfig4_${Date.now()}`
      )
      const config = await getSsoConfig('does-not-exist')

      assert.equal(config, null)
    })
  })

  describe('registerClient()', () => {
    it('should call RegisterClientCommand and return client credentials', async () => {
      const { registerClient } = await import(
        `../src/sso-login.js?t=register1_${Date.now()}`
      )

      const mockClient = {
        send: mock.fn(async () => ({
          clientId: 'test-client-id',
          clientSecret: 'test-client-secret',
          clientSecretExpiresAt: Date.now() + 7776000000,
        })),
      }

      const result = await registerClient(mockClient)
      assert.equal(result.clientId, 'test-client-id')
      assert.equal(result.clientSecret, 'test-client-secret')
      assert.equal(mockClient.send.mock.calls.length, 1)
    })
  })

  describe('startDeviceAuthorization()', () => {
    it('should call StartDeviceAuthorizationCommand and return auth data', async () => {
      const { startDeviceAuthorization } = await import(
        `../src/sso-login.js?t=startauth1_${Date.now()}`
      )

      const mockClient = {
        send: mock.fn(async () => ({
          deviceCode: 'test-device-code',
          userCode: 'ABCD-EFGH',
          verificationUri: 'https://device.sso.us-east-1.amazonaws.com/',
          verificationUriComplete:
            'https://device.sso.us-east-1.amazonaws.com/?user_code=ABCD-EFGH',
          expiresIn: 600,
          interval: 5,
        })),
      }

      const result = await startDeviceAuthorization(
        mockClient,
        'client-id',
        'client-secret',
        'https://my-sso.awsapps.com/start',
      )

      assert.equal(result.deviceCode, 'test-device-code')
      assert.equal(result.userCode, 'ABCD-EFGH')
      assert.equal(result.interval, 5)
      assert.equal(result.expiresIn, 600)
    })
  })

  describe('pollForToken()', () => {
    it('should handle AuthorizationPending then succeed', async () => {
      const { pollForToken } = await import(
        `../src/sso-login.js?t=poll1_${Date.now()}`
      )

      let callCount = 0
      const mockClient = {
        config: { region: 'us-east-1' },
        send: mock.fn(async () => {
          callCount++
          if (callCount < 3) {
            const err = new Error('Authorization pending')
            err.name = 'AuthorizationPendingException'
            throw err
          }
          return {
            accessToken: 'final-token',
            expiresIn: 28800,
          }
        }),
      }

      const events = []
      const result = await pollForToken(
        mockClient,
        'client-id',
        'client-secret',
        'device-code',
        0.01, // very short interval for tests
        600,
        (event, data) => events.push({ event, data }),
      )

      assert.equal(result.accessToken, 'final-token')
      assert.ok(result.expiresAt)
      assert.ok(events.length >= 2) // at least 2 pending events
    })

    it('should throw on ExpiredTokenException', async () => {
      const { pollForToken } = await import(
        `../src/sso-login.js?t=poll2_${Date.now()}`
      )

      const mockClient = {
        config: { region: 'us-east-1' },
        send: mock.fn(async () => {
          const err = new Error('Token expired')
          err.name = 'ExpiredTokenException'
          throw err
        }),
      }

      await assert.rejects(
        () =>
          pollForToken(
            mockClient,
            'client-id',
            'client-secret',
            'device-code',
            0.01,
            600,
            null,
          ),
        { message: /SSO authorization expired/ },
      )
    })
  })

  describe('ensureSsoSession()', () => {
    it('should skip login when cached token is valid', async () => {
      process.env.HOME = testDir

      // Write a valid SSO config
      const startUrl = `https://valid-cache-${Date.now()}.awsapps.com/start`
      const configContent = `[profile cached-sso]
sso_start_url = ${startUrl}
sso_region = us-east-1
sso_account_id = 123456789012
sso_role_name = Admin
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)

      // Write a valid cached token
      const hash = createHash('sha1').update(startUrl).digest('hex')
      const tokenPath = join(testDir, '.aws', 'sso', 'cache', `${hash}.json`)
      await writeFile(
        tokenPath,
        JSON.stringify({
          accessToken: 'cached-token',
          expiresAt: new Date(Date.now() + 3600000).toISOString(),
          region: 'us-east-1',
          startUrl,
        }),
      )

      const { ensureSsoSession } = await import(
        `../src/sso-login.js?t=ensure1_${Date.now()}`
      )

      const events = []
      const result = await ensureSsoSession('cached-sso', {
        onEvent: (event, data) => events.push({ event, data }),
      })

      assert.equal(result, true)
      // Should report valid session, not trigger login
      const statusEvents = events.filter((e) => e.event === 'sso-status')
      assert.ok(
        statusEvents.some((e) => e.data.message.includes('valid')),
        'Should report session as valid',
      )
    })

    it('should return true for non-SSO profile (no-op)', async () => {
      process.env.HOME = testDir

      const configContent = `[profile non-sso]
region = us-east-1
role_arn = arn:aws:iam::123456789012:role/dev
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)

      const { ensureSsoSession } = await import(
        `../src/sso-login.js?t=ensure2_${Date.now()}`
      )

      const result = await ensureSsoSession('non-sso')
      assert.equal(result, true)
    })
  })
})
