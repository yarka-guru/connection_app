import assert from 'node:assert/strict'
import { mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { after, before, describe, it } from 'node:test'
import {
  deleteProjectConfig,
  loadProjectConfigs,
  saveProjectConfig,
  saveProjectConfigs,
  validateProjectConfig,
} from '../configLoader.js'

describe('configLoader', () => {
  let testDir
  let configPath

  before(async () => {
    testDir = join(tmpdir(), `test-config-loader-${Date.now()}`)
    await mkdir(testDir, { recursive: true })
    configPath = join(testDir, 'projects.json')
  })

  after(async () => {
    await rm(testDir, { recursive: true, force: true })
  })

  describe('loadProjectConfigs', () => {
    it('should return empty object when file is missing', async () => {
      const missingPath = join(testDir, 'nonexistent.json')
      const result = await loadProjectConfigs(missingPath)
      assert.deepEqual(result, {})
    })

    it('should load valid JSON', async () => {
      const data = { myProject: { name: 'My Project', region: 'us-east-1' } }
      await writeFile(configPath, JSON.stringify(data))
      const result = await loadProjectConfigs(configPath)
      assert.deepEqual(result, data)
    })

    it('should throw on malformed JSON', async () => {
      const badPath = join(testDir, 'bad.json')
      await writeFile(badPath, '{ not valid json }')
      await assert.rejects(() => loadProjectConfigs(badPath), SyntaxError)
    })
  })

  describe('saveProjectConfigs', () => {
    it('should create directory and file', async () => {
      const nestedPath = join(testDir, 'nested', 'dir', 'projects.json')
      const data = { test: { name: 'Test' } }
      await saveProjectConfigs(data, nestedPath)
      const result = await loadProjectConfigs(nestedPath)
      assert.deepEqual(result, data)
    })
  })

  describe('saveProjectConfig', () => {
    it('should add a project to existing configs', async () => {
      const savePath = join(testDir, 'save-test.json')
      await saveProjectConfigs({ existing: { name: 'Existing' } }, savePath)
      await saveProjectConfig('new-project', { name: 'New' }, savePath)
      const result = await loadProjectConfigs(savePath)
      assert.equal(Object.keys(result).length, 2)
      assert.equal(result['new-project'].name, 'New')
      assert.equal(result.existing.name, 'Existing')
    })

    it('should overwrite an existing project', async () => {
      const savePath = join(testDir, 'overwrite-test.json')
      await saveProjectConfigs({ proj: { name: 'Old' } }, savePath)
      await saveProjectConfig('proj', { name: 'Updated' }, savePath)
      const result = await loadProjectConfigs(savePath)
      assert.equal(result.proj.name, 'Updated')
    })
  })

  describe('deleteProjectConfig', () => {
    it('should remove a project', async () => {
      const delPath = join(testDir, 'delete-test.json')
      await saveProjectConfigs(
        { a: { name: 'A' }, b: { name: 'B' } },
        delPath,
      )
      await deleteProjectConfig('a', delPath)
      const result = await loadProjectConfigs(delPath)
      assert.equal(Object.keys(result).length, 1)
      assert.equal(result.b.name, 'B')
      assert.equal(result.a, undefined)
    })
  })

  describe('validateProjectConfig', () => {
    const validConfig = {
      name: 'Test Project',
      region: 'us-east-1',
      database: 'mydb',
      secretPrefix: 'rds!cluster',
      rdsType: 'cluster',
      rdsPattern: '-rds-aurora',
      profileFilter: null,
      envPortMapping: { dev: '5433', staging: '5434' },
      defaultPort: '5432',
    }

    it('should accept a valid config', () => {
      const result = validateProjectConfig(validConfig)
      assert.equal(result.valid, true)
      assert.equal(result.errors.length, 0)
    })

    it('should reject missing required fields', () => {
      const result = validateProjectConfig({ name: 'Only Name' })
      assert.equal(result.valid, false)
      assert.ok(result.errors.length > 0)
      assert.ok(result.errors.some((e) => e.includes('region')))
    })

    it('should reject invalid rdsType', () => {
      const result = validateProjectConfig({ ...validConfig, rdsType: 'aurora' })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('rdsType')))
    })

    it('should reject invalid region format', () => {
      const result = validateProjectConfig({ ...validConfig, region: 'invalid' })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('region')))
    })

    it('should reject non-numeric defaultPort', () => {
      const result = validateProjectConfig({
        ...validConfig,
        defaultPort: 'abc',
      })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('defaultPort')))
    })

    it('should reject non-numeric port in envPortMapping', () => {
      const result = validateProjectConfig({
        ...validConfig,
        envPortMapping: { dev: 'abc' },
      })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('Port for "dev"')))
    })

    // Engine validation tests
    it('should accept config with engine=postgres', () => {
      const result = validateProjectConfig({ ...validConfig, engine: 'postgres' })
      assert.equal(result.valid, true)
    })

    it('should accept config with engine=mysql', () => {
      const result = validateProjectConfig({ ...validConfig, engine: 'mysql' })
      assert.equal(result.valid, true)
    })

    it('should accept config without engine field (backward compat)', () => {
      const { engine: _, ...configWithoutEngine } = validConfig
      const result = validateProjectConfig(configWithoutEngine)
      assert.equal(result.valid, true)
    })

    it('should reject invalid engine value', () => {
      const result = validateProjectConfig({ ...validConfig, engine: 'sqlite' })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('engine')))
    })

    // Shell-safe validation tests
    it('should reject secretPrefix with shell metacharacters', () => {
      const result = validateProjectConfig({ ...validConfig, secretPrefix: 'rds;rm -rf /' })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('secretPrefix')))
    })

    it('should reject rdsPattern with shell metacharacters', () => {
      const result = validateProjectConfig({ ...validConfig, rdsPattern: '$(whoami)' })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('rdsPattern')))
    })

    it('should reject database with shell metacharacters', () => {
      const result = validateProjectConfig({ ...validConfig, database: 'db&echo' })
      assert.equal(result.valid, false)
      assert.ok(result.errors.some((e) => e.includes('database')))
    })

    it('should accept safe special characters in fields', () => {
      const result = validateProjectConfig({
        ...validConfig,
        secretPrefix: 'rds!cluster-my/prefix',
        rdsPattern: 'my-rds_pattern.v2',
        database: 'my_db.prod',
      })
      assert.equal(result.valid, true)
    })
  })
})
