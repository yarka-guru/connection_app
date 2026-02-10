#!/usr/bin/env node

/**
 * Generate latest.json for Tauri updater
 *
 * This script generates the update manifest that needs to be uploaded
 * to GitHub releases for auto-updates to work.
 *
 * Usage: node scripts/generate-update-json.js <version> <release-url>
 * Example: node scripts/generate-update-json.js 1.7.0 https://github.com/yarka-guru/connection_app/releases/download/v1.7.0
 */

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

const version = process.argv[2]
const releaseUrl = process.argv[3]

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
const productName = tauriConf.productName.replace(/\s+/g, '_')

const updateManifest = {
  version: version,
  notes: `Release v${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    'darwin-aarch64': {
      url: `${releaseUrl}/${productName}_${version}_aarch64.app.tar.gz`,
      signature: '',
    },
    'darwin-x86_64': {
      url: `${releaseUrl}/${productName}_${version}_x64.app.tar.gz`,
      signature: '',
    },
    'linux-x86_64': {
      url: `${releaseUrl}/${productName}_${version}_amd64.AppImage.tar.gz`,
      signature: '',
    },
    'linux-aarch64': {
      url: `${releaseUrl}/${productName}_${version}_aarch64.AppImage.tar.gz`,
      signature: '',
    },
    'windows-x86_64': {
      url: `${releaseUrl}/${productName}_${version}_x64-setup.nsis.zip`,
      signature: '',
    },
  },
}

const outputPath = path.join(__dirname, '../latest.json')
fs.writeFileSync(outputPath, JSON.stringify(updateManifest, null, 2))
