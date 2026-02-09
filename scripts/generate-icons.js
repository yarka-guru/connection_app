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

  // Generate icons at different sizes
  const sizes = {
    '32x32.png': 32,
    '128x128.png': 128,
    '128x128@2x.png': 256,
  }

  for (const [filename, size] of Object.entries(sizes)) {
    const buffer = createIcon(size)
    fs.writeFileSync(path.join(iconsDir, filename), buffer)
  }

  // For .icns and .ico, we need the 256px version as base
  // Create a simple version for now
  const icon256 = createIcon(256)

  // Create ICNS (just use PNG for now - macOS accepts PNG in icns)
  const icnsHeader = Buffer.from([
    0x69,
    0x63,
    0x6e,
    0x73, // 'icns' magic
    0x00,
    0x00,
    0x00,
    0x00, // Total size (will be filled in)
  ])

  const ic08Type = Buffer.from([0x69, 0x63, 0x30, 0x38]) // 'ic08' (256x256 PNG)
  const ic08Size = Buffer.alloc(4)
  ic08Size.writeUInt32BE(8 + icon256.length, 0)

  const totalSize = 8 + 4 + 4 + icon256.length
  icnsHeader.writeUInt32BE(totalSize, 4)

  const icnsFile = Buffer.concat([icnsHeader, ic08Type, ic08Size, icon256])
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
} catch (_err) {
  // Create minimal placeholder PNGs
  const minimalPng = Buffer.from([
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d,
    0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00,
    0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
    0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
  ])

  const pngFiles = ['32x32.png', '128x128.png', '128x128@2x.png']
  for (const file of pngFiles) {
    fs.writeFileSync(path.join(iconsDir, file), minimalPng)
  }

  // Create minimal .icns and .ico
  const icnsHeader = Buffer.from([
    0x69, 0x63, 0x6e, 0x73, 0x00, 0x00, 0x00, 0x00,
  ])
  const icp4Type = Buffer.from([0x69, 0x63, 0x70, 0x34])
  const icp4Size = Buffer.alloc(4)
  icp4Size.writeUInt32BE(8 + minimalPng.length, 0)
  const totalSize = 8 + 4 + 4 + minimalPng.length
  icnsHeader.writeUInt32BE(totalSize, 4)
  const icnsFile = Buffer.concat([icnsHeader, icp4Type, icp4Size, minimalPng])
  fs.writeFileSync(path.join(iconsDir, 'icon.icns'), icnsFile)

  const icoHeader = Buffer.alloc(22)
  icoHeader.writeUInt16LE(0, 0)
  icoHeader.writeUInt16LE(1, 2)
  icoHeader.writeUInt16LE(1, 4)
  icoHeader.writeUInt8(1, 6)
  icoHeader.writeUInt8(1, 7)
  icoHeader.writeUInt32LE(minimalPng.length, 14)
  icoHeader.writeUInt32LE(22, 18)
  const icoFile = Buffer.concat([icoHeader, minimalPng])
  fs.writeFileSync(path.join(iconsDir, 'icon.ico'), icoFile)
}
