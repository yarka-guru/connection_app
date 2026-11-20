#!/usr/bin/env node

/**
 * Generate app icons for Tauri.
 * Creates PNG icons with a database connection symbol.
 */

import fs from 'node:fs'
import path from 'node:path'
import { createCanvas } from 'canvas'

const iconsDir = 'src-tauri/icons'

// Create icons directory if it doesn't exist
if (!fs.existsSync(iconsDir)) {
  fs.mkdirSync(iconsDir, { recursive: true })
}

function createIcon(size) {
  const canvas = createCanvas(size, size)
  const ctx = canvas.getContext('2d')

  // Background - rounded square
  const radius = size * 0.2
  ctx.fillStyle = '#1a1a2e'
  ctx.beginPath()
  ctx.roundRect(0, 0, size, size, radius)
  ctx.fill()

  // Inner gradient background
  const gradient = ctx.createLinearGradient(0, 0, size, size)
  gradient.addColorStop(0, '#16213e')
  gradient.addColorStop(1, '#0f3460')
  ctx.fillStyle = gradient
  ctx.beginPath()
  ctx.roundRect(
    size * 0.08,
    size * 0.08,
    size * 0.84,
    size * 0.84,
    radius * 0.8,
  )
  ctx.fill()

  // Database cylinder
  const dbWidth = size * 0.5
  const dbHeight = size * 0.4
  const dbX = size * 0.25
  const dbY = size * 0.3
  const ellipseHeight = size * 0.08

  // Database body
  ctx.fillStyle = '#e94560'
  ctx.beginPath()
  ctx.ellipse(
    dbX + dbWidth / 2,
    dbY,
    dbWidth / 2,
    ellipseHeight,
    0,
    0,
    Math.PI * 2,
  )
  ctx.fill()

  ctx.fillStyle = '#c73e54'
  ctx.fillRect(dbX, dbY, dbWidth, dbHeight)

  ctx.fillStyle = '#e94560'
  ctx.beginPath()
  ctx.ellipse(
    dbX + dbWidth / 2,
    dbY + dbHeight,
    dbWidth / 2,
    ellipseHeight,
    0,
    0,
    Math.PI,
  )
  ctx.fill()

  // Middle ellipse line
  ctx.strokeStyle = '#a03347'
  ctx.lineWidth = size * 0.015
  ctx.beginPath()
  ctx.ellipse(
    dbX + dbWidth / 2,
    dbY + dbHeight * 0.35,
    dbWidth / 2,
    ellipseHeight * 0.8,
    0,
    0,
    Math.PI,
  )
  ctx.stroke()

  ctx.beginPath()
  ctx.ellipse(
    dbX + dbWidth / 2,
    dbY + dbHeight * 0.65,
    dbWidth / 2,
    ellipseHeight * 0.8,
    0,
    0,
    Math.PI,
  )
  ctx.stroke()

  // Connection arrow/lightning bolt
  ctx.fillStyle = '#4fc3f7'
  const boltX = size * 0.6
  const boltY = size * 0.55
  const boltSize = size * 0.25

  ctx.beginPath()
  ctx.moveTo(boltX, boltY)
  ctx.lineTo(boltX + boltSize * 0.4, boltY)
  ctx.lineTo(boltX + boltSize * 0.2, boltY + boltSize * 0.5)
  ctx.lineTo(boltX + boltSize * 0.5, boltY + boltSize * 0.5)
  ctx.lineTo(boltX - boltSize * 0.1, boltY + boltSize)
  ctx.lineTo(boltX + boltSize * 0.1, boltY + boltSize * 0.55)
  ctx.lineTo(boltX - boltSize * 0.1, boltY + boltSize * 0.55)
  ctx.closePath()
  ctx.fill()

  return canvas.toBuffer('image/png')
}

// Check if canvas is available
try {
  // Test if canvas works
  const testCanvas = createCanvas(1, 1)
  testCanvas.getContext('2d')

  // Generate icons at different sizes.
  // icon-1024.png is not bundled with the app — it is uploaded to App Store
  // Connect, which rejects submissions without a 1024x1024 icon.
  const sizes = {
    '32x32.png': 32,
    '128x128.png': 128,
    '128x128@2x.png': 256,
    'icon-1024.png': 1024,
  }

  for (const [filename, size] of Object.entries(sizes)) {
    const buffer = createIcon(size)
    fs.writeFileSync(path.join(iconsDir, filename), buffer)
  }

  const icon256 = createIcon(256)

  // Build the ICNS with every representation macOS asks for.
  //
  // This file used to hold ic08 alone (256x256). The Mac App Store derives a
  // macOS app's Store icon from the .icns inside the uploaded bundle — there is
  // no separate upload field as there is for iOS — and it requires the 1024x1024
  // representation, ic10. Without it the submission is rejected.
  //
  // ICNS is a flat sequence of typed chunks: 4-byte type, 4-byte big-endian
  // length that counts its own 8-byte header, then the payload. macOS accepts
  // PNG payloads for all of these types.
  const icnsEntries = [
    ['ic07', 128], // 128x128
    ['ic08', 256], // 256x256
    ['ic09', 512], // 512x512
    ['ic10', 1024], // 1024x1024 — 512@2x, required by the Mac App Store
    ['ic11', 32], // 16x16@2x
    ['ic12', 64], // 32x32@2x
    ['ic13', 256], // 128x128@2x
    ['ic14', 512], // 256x256@2x
  ]

  const icnsChunks = []
  for (const [type, size] of icnsEntries) {
    const png = size === 256 ? icon256 : createIcon(size)
    const header = Buffer.alloc(8)
    header.write(type, 0, 4, 'ascii')
    header.writeUInt32BE(8 + png.length, 4)
    icnsChunks.push(header, png)
  }

  const icnsBody = Buffer.concat(icnsChunks)
  const icnsHeader = Buffer.alloc(8)
  icnsHeader.write('icns', 0, 4, 'ascii')
  icnsHeader.writeUInt32BE(8 + icnsBody.length, 4)

  const icnsFile = Buffer.concat([icnsHeader, icnsBody])
  fs.writeFileSync(path.join(iconsDir, 'icon.icns'), icnsFile)

  // Create a simple ICO file
  const icon32 = createIcon(32)
  const _icon16 = createIcon(16)

  // ICO header
  const icoHeader = Buffer.alloc(6)
  icoHeader.writeUInt16LE(0, 0) // Reserved
  icoHeader.writeUInt16LE(1, 2) // Type: ICO
  icoHeader.writeUInt16LE(1, 4) // Number of images

  // Directory entry (for 32x32 PNG)
  const dirEntry = Buffer.alloc(16)
  dirEntry.writeUInt8(32, 0) // Width
  dirEntry.writeUInt8(32, 1) // Height
  dirEntry.writeUInt8(0, 2) // Color palette
  dirEntry.writeUInt8(0, 3) // Reserved
  dirEntry.writeUInt16LE(1, 4) // Color planes
  dirEntry.writeUInt16LE(32, 6) // Bits per pixel
  dirEntry.writeUInt32LE(icon32.length, 8) // Image size
  dirEntry.writeUInt32LE(22, 12) // Offset to image data

  const icoFile = Buffer.concat([icoHeader, dirEntry, icon32])
  fs.writeFileSync(path.join(iconsDir, 'icon.ico'), icoFile)
} catch (err) {
  // This used to overwrite every icon with a 1x1 transparent PNG so the script
  // could "succeed" without canvas. That is worse than failing: a developer
  // running it on a machine where the native canvas build is broken would
  // silently destroy the real icons and only find out when a build shipped
  // blank. Fail loudly instead and leave the existing files alone.
  console.error('Icon generation failed — existing icons left untouched.')
  console.error(`Reason: ${err.message}`)
  console.error('')
  console.error('The `canvas` package needs a working native build.')
  console.error('Try: npm install --save-dev canvas')
  process.exit(1)
}
