#!/usr/bin/env node

/**
 * Package the sidecar for all supported platforms.
 * This script:
 * 1. Bundles the ES modules using esbuild
 * 2. Packages the bundle with pkg for each platform
 */

import { execSync } from 'node:child_process'
import fs from 'node:fs'
import { build } from 'esbuild'

const TARGETS = [
  { triple: 'aarch64-apple-darwin', pkg: 'node22-macos-arm64' },
  { triple: 'x86_64-apple-darwin', pkg: 'node22-macos-x64' },
  { triple: 'x86_64-unknown-linux-gnu', pkg: 'node22-linux-x64' },
  { triple: 'x86_64-pc-windows-msvc', pkg: 'node22-win-x64' },
]

async function main() {
  const bundlePath = 'src-tauri/binaries/gui-adapter-bundle.cjs'

  // Ensure binaries directory exists
  fs.mkdirSync('src-tauri/binaries', { recursive: true })
  await build({
    entryPoints: ['gui-adapter.js'],
    bundle: true,
    platform: 'node',
    target: 'node22',
    outfile: bundlePath,
    format: 'cjs',
    external: ['inquirer'],
  })

  // Step 2: Package with pkg for each target
  for (const { triple, pkg } of TARGETS) {
    const outputPath = `src-tauri/binaries/gui-adapter-${triple}`

    const command = `npx @yao-pkg/pkg ${bundlePath} --target ${pkg} -o ${outputPath}`

    try {
      execSync(command, { stdio: 'inherit' })
    } catch (_error) {
      process.exit(1)
    }
  }

  // Clean up bundle
  fs.unlinkSync(bundlePath)
}

main().catch((_err) => {
  process.exit(1)
})
