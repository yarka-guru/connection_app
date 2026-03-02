import assert from 'node:assert/strict'
import { mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, before, describe, it } from 'node:test'

describe('credential-resolver', () => {
  let testDir
  let originalHome
  let originalAwsVault
  let originalAwsProfile

  before(async () => {
    testDir = join(tmpdir(), `test-cred-resolver-${Date.now()}`)
    await mkdir(join(testDir, '.aws'), { recursive: true })
    originalHome = process.env.HOME
    originalAwsVault = process.env.AWS_VAULT
    originalAwsProfile = process.env.AWS_PROFILE
  })

  after(async () => {
    process.env.HOME = originalHome
    if (originalAwsVault !== undefined) {
      process.env.AWS_VAULT = originalAwsVault
    } else {
      delete process.env.AWS_VAULT
    }
    if (originalAwsProfile !== undefined) {
      process.env.AWS_PROFILE = originalAwsProfile
    } else {
      delete process.env.AWS_PROFILE
    }
    await rm(testDir, { recursive: true, force: true })
  })

  describe('parseAwsConfig()', () => {
    it('should parse config with default, dev, and prod profiles', async () => {
      const configContent = `[default]
region = us-east-1
output = json

[profile dev]
region = us-east-2
role_arn = arn:aws:iam::123456789012:role/dev

[profile prod]
region = us-west-2
role_arn = arn:aws:iam::123456789012:role/prod
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)
      process.env.HOME = testDir

      // Re-import to pick up new HOME
      const { parseAwsConfig } = await import(
        `../src/credential-resolver.js?t=parse1_${Date.now()}`
      )
      const profiles = await parseAwsConfig()

      assert.ok(profiles.default, 'Should have default profile')
      assert.equal(profiles.default.region, 'us-east-1')
      assert.equal(profiles.default.output, 'json')

      assert.ok(profiles.dev, 'Should have dev profile')
      assert.equal(profiles.dev.region, 'us-east-2')
      assert.equal(profiles.dev.role_arn, 'arn:aws:iam::123456789012:role/dev')

      assert.ok(profiles.prod, 'Should have prod profile')
      assert.equal(profiles.prod.region, 'us-west-2')
    })

    it('should return empty object when config file does not exist', async () => {
      const nonexistentDir = join(tmpdir(), `nonexistent-home-${Date.now()}`)
      process.env.HOME = nonexistentDir

      const { parseAwsConfig } = await import(
        `../src/credential-resolver.js?t=parse2_${Date.now()}`
      )
      const profiles = await parseAwsConfig()

      assert.deepEqual(profiles, {})

      // Restore HOME for subsequent tests
      process.env.HOME = testDir
    })

    it('should handle comment lines (# and ;)', async () => {
      const configContent = `# This is a comment
; This is also a comment
[profile myprofile]
# inline comment above key
region = us-west-1
; another comment
output = json
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)
      process.env.HOME = testDir

      const { parseAwsConfig } = await import(
        `../src/credential-resolver.js?t=parse3_${Date.now()}`
      )
      const profiles = await parseAwsConfig()

      assert.ok(profiles.myprofile, 'Should have myprofile')
      assert.equal(profiles.myprofile.region, 'us-west-1')
      assert.equal(profiles.myprofile.output, 'json')
      // Comments should not appear as keys
      assert.equal(Object.keys(profiles.myprofile).length, 2)
    })

    it('should handle key=value pairs with spaces around =', async () => {
      const configContent = `[profile spaced]
region   =   us-east-1
output=text
role_arn =arn:aws:iam::111:role/test
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)
      process.env.HOME = testDir

      const { parseAwsConfig } = await import(
        `../src/credential-resolver.js?t=parse4_${Date.now()}`
      )
      const profiles = await parseAwsConfig()

      assert.ok(profiles.spaced, 'Should have spaced profile')
      assert.equal(profiles.spaced.region, 'us-east-1')
      assert.equal(profiles.spaced.output, 'text')
      assert.equal(profiles.spaced.role_arn, 'arn:aws:iam::111:role/test')
    })
  })

  describe('detectAuthType()', () => {
    before(async () => {
      const configContent = `[profile sso-profile]
sso_start_url = https://my-sso.awsapps.com/start
sso_account_id = 123456789012
sso_role_name = AdminRole
region = us-east-1

[profile assume-role-mfa-profile]
role_arn = arn:aws:iam::123456789012:role/admin
mfa_serial = arn:aws:iam::123456789012:mfa/user
source_profile = default

[profile assume-role-profile]
role_arn = arn:aws:iam::123456789012:role/readonly
source_profile = default

[profile process-profile]
credential_process = /usr/bin/get-creds

[profile static-profile]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[profile empty-profile]
region = us-east-1
`
      await writeFile(join(testDir, '.aws', 'config'), configContent)
      process.env.HOME = testDir
    })

    it('should detect SSO profile', async () => {
      const { detectAuthType } = await import(
        `../src/credential-resolver.js?t=auth1_${Date.now()}`
      )
      const result = await detectAuthType('sso-profile')

      assert.equal(result.type, 'sso')
      assert.equal(
        result.details.sso_start_url,
        'https://my-sso.awsapps.com/start',
      )
      assert.equal(result.details.sso_account_id, '123456789012')
      assert.equal(result.details.sso_role_name, 'AdminRole')
    })

    it('should detect assume-role-mfa profile', async () => {
      const { detectAuthType } = await import(
        `../src/credential-resolver.js?t=auth2_${Date.now()}`
      )
      const result = await detectAuthType('assume-role-mfa-profile')

      assert.equal(result.type, 'assume-role-mfa')
      assert.equal(
        result.details.role_arn,
        'arn:aws:iam::123456789012:role/admin',
      )
      assert.equal(
        result.details.mfa_serial,
        'arn:aws:iam::123456789012:mfa/user',
      )
      assert.equal(result.details.source_profile, 'default')
    })

    it('should detect assume-role profile (no MFA)', async () => {
      const { detectAuthType } = await import(
        `../src/credential-resolver.js?t=auth3_${Date.now()}`
      )
      const result = await detectAuthType('assume-role-profile')

      assert.equal(result.type, 'assume-role')
      assert.equal(
        result.details.role_arn,
        'arn:aws:iam::123456789012:role/readonly',
      )
      assert.equal(result.details.source_profile, 'default')
    })

    it('should detect process credentials', async () => {
      const { detectAuthType } = await import(
        `../src/credential-resolver.js?t=auth4_${Date.now()}`
      )
      const result = await detectAuthType('process-profile')

      assert.equal(result.type, 'process')
      assert.equal(result.details.credential_process, '/usr/bin/get-creds')
    })

    it('should detect static credentials', async () => {
      const { detectAuthType } = await import(
        `../src/credential-resolver.js?t=auth5_${Date.now()}`
      )
      const result = await detectAuthType('static-profile')

      assert.equal(result.type, 'static')
    })

    it('should return unknown for profile with no recognized keys', async () => {
      const { detectAuthType } = await import(
        `../src/credential-resolver.js?t=auth6_${Date.now()}`
      )
      const result = await detectAuthType('empty-profile')

      assert.equal(result.type, 'unknown')
    })

    it('should return unknown for nonexistent profile', async () => {
      const { detectAuthType } = await import(
        `../src/credential-resolver.js?t=auth7_${Date.now()}`
      )
      const result = await detectAuthType('does-not-exist')

      assert.equal(result.type, 'unknown')
    })
  })

  describe('isRunningUnderAwsVault()', () => {
    it('should return true when AWS_VAULT is set', async () => {
      process.env.AWS_VAULT = 'my-profile'

      const { isRunningUnderAwsVault } = await import(
        `../src/credential-resolver.js?t=vault1_${Date.now()}`
      )
      assert.equal(isRunningUnderAwsVault(), true)
    })

    it('should return false when AWS_VAULT is not set', async () => {
      delete process.env.AWS_VAULT

      const { isRunningUnderAwsVault } = await import(
        `../src/credential-resolver.js?t=vault2_${Date.now()}`
      )
      assert.equal(isRunningUnderAwsVault(), false)
    })
  })

  describe('resolveCredentials()', () => {
    it('should return a function', async () => {
      delete process.env.AWS_VAULT
      delete process.env.AWS_PROFILE

      const { resolveCredentials } = await import(
        `../src/credential-resolver.js?t=resolve1_${Date.now()}`
      )
      const provider = resolveCredentials('some-profile')

      assert.equal(typeof provider, 'function')
    })

    it('should return a function when AWS_VAULT is set', async () => {
      process.env.AWS_VAULT = 'my-vault-profile'

      const { resolveCredentials } = await import(
        `../src/credential-resolver.js?t=resolve2_${Date.now()}`
      )
      const provider = resolveCredentials('some-profile')

      assert.equal(typeof provider, 'function')

      delete process.env.AWS_VAULT
    })

    it('should return a function when AWS_PROFILE is set and no profile given', async () => {
      delete process.env.AWS_VAULT
      process.env.AWS_PROFILE = 'env-profile'

      const { resolveCredentials } = await import(
        `../src/credential-resolver.js?t=resolve3_${Date.now()}`
      )
      const provider = resolveCredentials(null)

      assert.equal(typeof provider, 'function')

      delete process.env.AWS_PROFILE
    })
  })
})
