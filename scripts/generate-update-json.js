#!/usr/bin/env node

/**
 * Generate latest.json for Tauri updater
 *
 * This script generates the update manifest that needs to be uploaded
 * to GitHub releases for auto-updates to work.
 *
 * Usage: node scripts/generate-update-json.js <version> <release-url> [sigs-dir]
 * Example: node scripts/generate-update-json.js 1.7.0 https://github.com/yarka-guru/connection_app/releases/download/v1.7.0 sigs
 */

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

const version = process.argv[2]
const releaseUrl = process.argv[3]
const sigsDir = process.argv[4] // Optional: directory containing .sig files downloaded from release

if (!version || !releaseUrl) {
  process.exit(1)
}

// Read the current tauri.conf.json to get the product name
const tauriConf = JSON.parse(
  fs.readFileSync(
    path.join(__dirname, '../src-tauri/tauri.conf.json'),
    'utf-8',
  ),
)
// Tauri uses dots for spaces in bundle filenames
const productName = tauriConf.productName.replace(/\s+/g, '.')

// Rust target → Tauri updater platform mapping
const TARGET_TO_PLATFORM = {
  'aarch64-apple-darwin': 'darwin-aarch64',
  'x86_64-apple-darwin': 'darwin-x86_64',
  'x86_64-unknown-linux-gnu': 'linux-x86_64',
  'aarch64-unknown-linux-gnu': 'linux-aarch64',
  'x86_64-pc-windows-msvc': 'windows-x86_64',
}

// Read signatures from CI artifacts (named by rust target: <target>.sig)
function loadSignatures() {
  const sigs = {}
  if (!sigsDir) return sigs

  for (const [target, platform] of Object.entries(TARGET_TO_PLATFORM)) {
    try {
      sigs[platform] = fs
        .readFileSync(path.join(sigsDir, `${target}.sig`), 'utf-8')
        .trim()
    } catch {
      sigs[platform] = ''
    }
  }
  return sigs
}

const signatures = loadSignatures()

const macAarch64 = `${productName}_aarch64.app.tar.gz`
const macX64 = `${productName}_x64.app.tar.gz`
const linuxAmd64 = `${productName}_${version}_amd64.AppImage`
const linuxAarch64 = `${productName}_${version}_aarch64.AppImage`
const windowsX64 = `${productName}_${version}_x64-setup.exe`

const updateManifest = {
  version: version,
  notes: `Release v${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    'darwin-aarch64': {
      url: `${releaseUrl}/${macAarch64}`,
      signature: signatures['darwin-aarch64'] || '',
    },
    'darwin-x86_64': {
      url: `${releaseUrl}/${macX64}`,
      signature: signatures['darwin-x86_64'] || '',
    },
    'linux-x86_64': {
      url: `${releaseUrl}/${linuxAmd64}`,
      signature: signatures['linux-x86_64'] || '',
    },
    'linux-aarch64': {
      url: `${releaseUrl}/${linuxAarch64}`,
      signature: signatures['linux-aarch64'] || '',
    },
    'windows-x86_64': {
      url: `${releaseUrl}/${windowsX64}`,
      signature: signatures['windows-x86_64'] || '',
    },
  },
}

const outputPath = path.join(__dirname, '../latest.json')
fs.writeFileSync(outputPath, JSON.stringify(updateManifest, null, 2))

// Warn if latest.json is not gitignored (it should never be committed)
const gitignorePath = path.join(__dirname, '../.gitignore')
try {
  const gitignore = fs.readFileSync(gitignorePath, 'utf-8')
  if (!gitignore.includes('latest.json')) {
    console.warn('WARNING: latest.json is not in .gitignore — add it to avoid accidental commits')
  }
} catch {}

