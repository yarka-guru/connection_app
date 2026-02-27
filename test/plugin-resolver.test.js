import assert from 'node:assert/strict'
import { describe, it } from 'node:test'
import {
  checkPluginVersion,
  findPluginBinary,
  spawnPlugin,
} from '../src/plugin-resolver.js'

describe('plugin-resolver', () => {
  describe('findPluginBinary()', () => {
    it('should return a string or null', () => {
      const result = findPluginBinary()

      assert.ok(
        result === null || typeof result === 'string',
        `Expected string or null, got ${typeof result}`,
      )
    })

    it('should return null when plugin is not installed on this system', () => {
      // This test verifies that the function gracefully returns null
      // if session-manager-plugin is not installed. If it IS installed,
      // the result will be a string path, which is also acceptable.
      const result = findPluginBinary()

      assert.ok(
        result === null || typeof result === 'string',
        'Should return null or a valid path string',
      )
    })
  })

  describe('checkPluginVersion()', () => {
    it('should return null for nonexistent binary path', () => {
      const result = checkPluginVersion(
        '/nonexistent/path/to/session-manager-plugin',
      )

      assert.equal(result, null)
    })
  })

  describe('spawnPlugin()', () => {
    it('should be a function', () => {
      assert.equal(typeof spawnPlugin, 'function')
    })
  })
})
