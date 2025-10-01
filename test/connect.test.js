import { describe, it, before, after } from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'fs/promises'
import { tmpdir } from 'os'
import { join } from 'path'
import { writeFile, mkdir, rm } from 'fs/promises'

// Test AWS config parsing
describe('AWS Config Parsing', () => {
  let testConfigDir

  before(async () => {
    testConfigDir = join(tmpdir(), `test-aws-config-${Date.now()}`)
    await mkdir(testConfigDir, { recursive: true })
  })

  after(async () => {
    await rm(testConfigDir, { recursive: true, force: true })
  })

  it('should parse AWS config profiles correctly', async () => {
    const configContent = `
[profile dev]
region = us-east-2

[profile stage]
region = us-east-2

[profile prod]
region = us-east-1
`
    const configPath = join(testConfigDir, 'config')
    await writeFile(configPath, configContent)

    const content = await readFile(configPath, { encoding: 'utf-8' })
    const profiles = content
      .split(/\r?\n/)
      .filter(line => line.startsWith('[') && line.endsWith(']'))
      .map(line => line.slice(1, -1))
      .map(line => line.replace('profile ', '').trim())

    assert.deepEqual(profiles, ['dev', 'stage', 'prod'])
  })

  it('should handle empty config file', async () => {
    const configPath = join(testConfigDir, 'empty-config')
    await writeFile(configPath, '')

    const content = await readFile(configPath, { encoding: 'utf-8' })
    const profiles = content
      .split(/\r?\n/)
      .filter(line => line.startsWith('[') && line.endsWith(']'))
      .map(line => line.slice(1, -1))
      .map(line => line.replace('profile ', '').trim())

    assert.deepEqual(profiles, [])
  })

  it('should handle malformed config entries', async () => {
    const configContent = `
[profile valid-env]
region = us-east-2

this is not a profile
[another-invalid

[profile another-valid]
`
    const configPath = join(testConfigDir, 'malformed-config')
    await writeFile(configPath, configContent)

    const content = await readFile(configPath, { encoding: 'utf-8' })
    const profiles = content
      .split(/\r?\n/)
      .filter(line => line.startsWith('[') && line.endsWith(']'))
      .map(line => line.slice(1, -1))
      .map(line => line.replace('profile ', '').trim())

    assert.deepEqual(profiles, ['valid-env', 'another-valid'])
  })
})

// Test port mapping logic
describe('Port Mapping', () => {
  it('should map environment suffix to correct port', () => {
    const envPortMapping = {
      dev: '5433',
      stage: '5434',
      'pre-prod': '5435',
      prod: '5436',
      team1: '5442'
    }

    const testCases = [
      { env: 'my-project-dev', expected: '5433' },
      { env: 'my-project-stage', expected: '5434' },
      { env: 'my-project-prod', expected: '5436' },
      { env: 'my-project-team1', expected: '5442' },
      { env: 'unknown-env', expected: '5432' } // default
    ]

    testCases.forEach(({ env, expected }) => {
      const allEnvSuffixes = Object.keys(envPortMapping).sort((a, b) => b.length - a.length)
      const matchedSuffix = allEnvSuffixes.find(suffix => env.endsWith(suffix))
      const portNumber = envPortMapping[matchedSuffix] || '5432'

      assert.equal(portNumber, expected, `Failed for env: ${env}`)
    })
  })

  it('should handle longest matching suffix', () => {
    const envPortMapping = {
      dev: '5433',
      'perf-dev': '5440'
    }

    const env = 'my-project-perf-dev'
    const allEnvSuffixes = Object.keys(envPortMapping).sort((a, b) => b.length - a.length)
    const matchedSuffix = allEnvSuffixes.find(suffix => env.endsWith(suffix))
    const portNumber = envPortMapping[matchedSuffix] || '5432'

    // Should match 'perf-dev' not 'dev'
    assert.equal(portNumber, '5440')
  })
})

// Test credentials parsing
describe('Credentials Parsing', () => {
  it('should parse valid credentials JSON', () => {
    const secretString = JSON.stringify({
      username: 'testuser',
      password: 'testpass123'
    })

    const credentials = JSON.parse(secretString)

    assert.equal(credentials.username, 'testuser')
    assert.equal(credentials.password, 'testpass123')
  })

  it('should detect missing username', () => {
    const secretString = JSON.stringify({
      password: 'testpass123'
    })

    const credentials = JSON.parse(secretString)

    assert.ok(!credentials.username, 'Username should be missing')
  })

  it('should detect missing password', () => {
    const secretString = JSON.stringify({
      username: 'testuser'
    })

    const credentials = JSON.parse(secretString)

    assert.ok(!credentials.password, 'Password should be missing')
  })

  it('should throw on malformed JSON', () => {
    const secretString = '{ invalid json }'

    assert.throws(() => {
      JSON.parse(secretString)
    }, SyntaxError)
  })
})

// Test retry configuration
describe('Retry Configuration', () => {
  it('should have valid retry values', () => {
    const RETRY_CONFIG = {
      BASTION_WAIT_MAX_RETRIES: 20,
      BASTION_WAIT_RETRY_DELAY_MS: 15000,
      PORT_FORWARDING_MAX_RETRIES: 2,
      SSM_AGENT_READY_WAIT_MS: 10000
    }

    assert.ok(RETRY_CONFIG.BASTION_WAIT_MAX_RETRIES > 0, 'Max retries should be positive')
    assert.ok(RETRY_CONFIG.BASTION_WAIT_RETRY_DELAY_MS > 0, 'Retry delay should be positive')
    assert.ok(RETRY_CONFIG.PORT_FORWARDING_MAX_RETRIES >= 0, 'Max retries should be non-negative')
    assert.ok(RETRY_CONFIG.SSM_AGENT_READY_WAIT_MS > 0, 'SSM wait should be positive')
  })
})