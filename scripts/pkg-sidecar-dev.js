#!/usr/bin/env node

/**
 * Helper script to package the sidecar for the current platform during development.
 * This script:
 * 1. Bundles the ES modules using esbuild
 * 2. Packages the bundle with pkg
 */

import { execSync } from 'node:child_process'
import fs from 'node:fs'
import process from 'node:process'
import { build } from 'esbuild'

// Map Node.js platform/arch to Tauri target triple
function getTargetTriple() {
  const platform = process.platform
  const arch = process.arch

  if (platform === 'darwin') {
    return arch === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin'
  } else if (platform === 'linux') {
    return arch === 'arm64'
      ? 'aarch64-unknown-linux-gnu'
      : 'x86_64-unknown-linux-gnu'
  } else if (platform === 'win32') {
    return 'x86_64-pc-windows-msvc'
  }

  throw new Error(`Unsupported platform: ${platform}-${arch}`)
}

// Map to pkg target
function getPkgTarget() {
  const platform = process.platform
  const arch = process.arch

  if (platform === 'darwin') {
    return arch === 'arm64' ? 'node22-macos-arm64' : 'node22-macos-x64'
  } else if (platform === 'linux') {
    return arch === 'arm64' ? 'node22-linux-arm64' : 'node22-linux-x64'
  } else if (platform === 'win32') {
    return 'node22-win-x64'
  }

  throw new Error(`Unsupported platform: ${platform}-${arch}`)
}

async function main() {
  const targetTriple = getTargetTriple()
  const pkgTarget = getPkgTarget()
  const bundlePath = 'src-tauri/binaries/gui-adapter-bundle.cjs'
  const outputPath = `src-tauri/binaries/gui-adapter-${targetTriple}`

  // Ensure binaries directory exists
  fs.mkdirSync('src-tauri/binaries', { recursive: true })
  await build({
    entryPoints: ['gui-adapter.js'],
    bundle: true,
    platform: 'node',
    target: 'node22',
    outfile: bundlePath,
    format: 'cjs',
    // Externalize CLI-only dependencies and Node.js built-ins
    external: [
      'inquirer', // CLI-only, not needed for GUI adapter
    ],
  })
  const command = `npx @yao-pkg/pkg ${bundlePath} --target ${pkgTarget} -o ${outputPath}`

  try {
    execSync(command, { stdio: 'inherit' })

    // Clean up bundle
    fs.unlinkSync(bundlePath)
  } catch (_error) {
    process.exit(1)
  }
}

main().catch((_err) => {
  process.exit(1)
})
