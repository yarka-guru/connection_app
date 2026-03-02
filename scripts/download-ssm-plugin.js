#!/usr/bin/env node

import { execSync } from "node:child_process";
import {
  cpSync,
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

const __dirname = fileURLToPath(new URL(".", import.meta.url));
const PROJECT_ROOT = resolve(__dirname, "..");
const BINARIES_DIR = join(PROJECT_ROOT, "src-tauri", "binaries");
const BINARY_NAME = "session-manager-plugin";

// ---------------------------------------------------------------------------
// Target definitions
// ---------------------------------------------------------------------------

const TARGETS = [
  {
    triple: "aarch64-apple-darwin",
    url: "https://s3.amazonaws.com/session-manager-downloads/plugin/latest/mac_arm64/sessionmanager-bundle.zip",
    format: "mac-zip",
    nodeArch: "arm64",
    nodePlatform: "darwin",
  },
  {
    triple: "x86_64-apple-darwin",
    url: "https://s3.amazonaws.com/session-manager-downloads/plugin/latest/mac/sessionmanager-bundle.zip",
    format: "mac-zip",
    nodeArch: "x64",
    nodePlatform: "darwin",
  },
  {
    triple: "x86_64-unknown-linux-gnu",
    url: "https://s3.amazonaws.com/session-manager-downloads/plugin/latest/ubuntu_64bit/session-manager-plugin.deb",
    format: "deb",
    nodeArch: "x64",
    nodePlatform: "linux",
  },
  {
    triple: "aarch64-unknown-linux-gnu",
    url: "https://s3.amazonaws.com/session-manager-downloads/plugin/latest/ubuntu_arm64/session-manager-plugin.deb",
    format: "deb",
    nodeArch: "arm64",
    nodePlatform: "linux",
  },
  {
    triple: "x86_64-pc-windows-msvc",
    url: "https://s3.amazonaws.com/session-manager-downloads/plugin/latest/windows/SessionManagerPlugin.zip",
    format: "windows-zip",
    nodeArch: "x64",
    nodePlatform: "win32",
  },
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatBytes(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  const mb = kb / 1024;
  return `${mb.toFixed(1)} MB`;
}

/**
 * Download a URL to a local file path using fetch().
 */
async function downloadFile(url, destPath) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(
      `Failed to download ${url}: ${response.status} ${response.statusText}`,
    );
  }
  const buffer = Buffer.from(await response.arrayBuffer());
  writeFileSync(destPath, buffer);
  return buffer.length;
}

/**
 * Extract the session-manager-plugin binary from a macOS zip bundle.
 *
 * The zip contains: sessionmanager-bundle/bin/session-manager-plugin
 */
function extractMacZip(archivePath, tmpDir) {
  try {
    execSync(`unzip -o "${archivePath}" -d "${tmpDir}"`, { stdio: "pipe" });
  } catch {
    throw new Error(
      "Failed to extract macOS zip. Ensure `unzip` is installed.",
    );
  }
  const binaryPath = join(
    tmpDir,
    "sessionmanager-bundle",
    "bin",
    "session-manager-plugin",
  );
  if (!existsSync(binaryPath)) {
    throw new Error(
      `Expected binary not found at: ${binaryPath}. Archive contents: ${listDir(tmpDir)}`,
    );
  }
  return binaryPath;
}

/**
 * Extract the session-manager-plugin binary from a .deb package.
 *
 * .deb files are `ar` archives containing data.tar.gz (or data.tar.xz).
 * The binary is at: usr/local/sessionmanagerplugin/bin/session-manager-plugin
 */
function extractDeb(archivePath, tmpDir) {
  const extractDir = join(tmpDir, "deb-extract");
  mkdirSync(extractDir, { recursive: true });

  // Step 1: Use `ar` to extract the deb contents
  try {
    execSync(`ar x "${archivePath}" --output "${extractDir}"`, {
      stdio: "pipe",
    });
  } catch {
    throw new Error(
      "Failed to extract .deb archive. Ensure `ar` is installed (usually part of binutils).",
    );
  }

  // Step 2: Find and extract data.tar.gz or data.tar.xz
  const dataDir = join(tmpDir, "deb-data");
  mkdirSync(dataDir, { recursive: true });

  const files = readdirSync(extractDir);
  const dataTarGz = files.find((f) => f === "data.tar.gz");
  const dataTarXz = files.find((f) => f === "data.tar.xz");
  const dataTarZst = files.find((f) => f === "data.tar.zst");
  const dataTar = files.find((f) => f.startsWith("data.tar"));

  if (dataTarGz) {
    execSync(`tar xzf "${join(extractDir, dataTarGz)}" -C "${dataDir}"`, {
      stdio: "pipe",
    });
  } else if (dataTarXz) {
    execSync(`tar xJf "${join(extractDir, dataTarXz)}" -C "${dataDir}"`, {
      stdio: "pipe",
    });
  } else if (dataTarZst) {
    execSync(
      `tar --zstd -xf "${join(extractDir, dataTarZst)}" -C "${dataDir}"`,
      { stdio: "pipe" },
    );
  } else if (dataTar) {
    execSync(`tar xf "${join(extractDir, dataTar)}" -C "${dataDir}"`, {
      stdio: "pipe",
    });
  } else {
    throw new Error(
      `No data.tar.* found in .deb. Contents: ${files.join(", ")}`,
    );
  }

  const binaryPath = join(
    dataDir,
    "usr",
    "local",
    "sessionmanagerplugin",
    "bin",
    "session-manager-plugin",
  );
  if (!existsSync(binaryPath)) {
    throw new Error(
      `Expected binary not found at: ${binaryPath}. Extracted contents: ${listDir(dataDir)}`,
    );
  }
  return binaryPath;
}

/**
 * Extract the session-manager-plugin.exe from a Windows zip.
 *
 * The zip may contain the binary at bin/session-manager-plugin.exe or at root.
 */
function extractWindowsZip(archivePath, tmpDir) {
  const isWindows = process.platform === "win32";

  if (isWindows) {
    try {
      execSync(
        `powershell -Command "Expand-Archive -Force -Path '${archivePath}' -DestinationPath '${tmpDir}'"`,
        { stdio: "pipe" },
      );
    } catch {
      throw new Error(
        "Failed to extract Windows zip using PowerShell Expand-Archive.",
      );
    }
  } else {
    try {
      execSync(`unzip -o "${archivePath}" -d "${tmpDir}"`, { stdio: "pipe" });
    } catch {
      throw new Error(
        "Failed to extract Windows zip. Ensure `unzip` is installed.",
      );
    }
  }

  // Search for the exe — could be in bin/ subdirectory or at root
  const candidates = [
    join(tmpDir, "bin", "session-manager-plugin.exe"),
    join(tmpDir, "session-manager-plugin.exe"),
    join(tmpDir, "SessionManagerPlugin", "bin", "session-manager-plugin.exe"),
    join(tmpDir, "SessionManagerPlugin", "session-manager-plugin.exe"),
  ];

  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }

  // If none of the known paths worked, search recursively for it
  const found = findFileRecursive(tmpDir, "session-manager-plugin.exe");
  if (found) return found;

  throw new Error(
    `session-manager-plugin.exe not found in Windows zip. Contents: ${listDir(tmpDir)}`,
  );
}

/**
 * Recursively search for a file by name.
 */
function findFileRecursive(dir, filename) {
  const entries = readdirSync(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      const result = findFileRecursive(fullPath, filename);
      if (result) return result;
    } else if (entry.name.toLowerCase() === filename.toLowerCase()) {
      return fullPath;
    }
  }
  return null;
}

/**
 * List directory contents for debugging.
 */
function listDir(dir) {
  try {
    const entries = [];
    function walk(d, prefix) {
      for (const e of readdirSync(d, { withFileTypes: true })) {
        const rel = prefix ? `${prefix}/${e.name}` : e.name;
        entries.push(rel);
        if (e.isDirectory()) walk(join(d, e.name), rel);
      }
    }
    walk(dir, "");
    return entries.join(", ");
  } catch {
    return "(unable to list)";
  }
}

/**
 * Determine which targets to process based on the current platform.
 */
function getTargetsForCurrentPlatform() {
  const arch = process.arch;
  const platform = process.platform;

  return TARGETS.filter(
    (t) => t.nodeArch === arch && t.nodePlatform === platform,
  );
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function processTarget(target) {
  const { triple, url, format } = target;
  const isWindows = triple.includes("windows");
  const outputFilename = `${BINARY_NAME}-${triple}${isWindows ? ".exe" : ""}`;
  const outputPath = join(BINARIES_DIR, outputFilename);
  const relativePath = outputPath.replace(PROJECT_ROOT + "/", "");

  console.log(`Downloading ${BINARY_NAME} for ${triple}...`);

  // Create a temp directory for this target
  const tmpDir = mkdtempSync(join(tmpdir(), `ssm-plugin-${triple}-`));

  try {
    // Determine archive filename from URL
    const urlParts = url.split("/");
    const archiveFilename = urlParts[urlParts.length - 1];
    const archivePath = join(tmpDir, archiveFilename);

    // Download
    const size = await downloadFile(url, archivePath);
    console.log(`  Downloaded: ${formatBytes(size)}`);

    // Extract
    let binaryPath;
    switch (format) {
      case "mac-zip":
        binaryPath = extractMacZip(archivePath, tmpDir);
        break;
      case "deb":
        binaryPath = extractDeb(archivePath, tmpDir);
        break;
      case "windows-zip":
        binaryPath = extractWindowsZip(archivePath, tmpDir);
        break;
      default:
        throw new Error(`Unknown format: ${format}`);
    }

    // Copy binary to destination
    cpSync(binaryPath, outputPath);

    // Set executable permission on Unix
    if (!isWindows) {
      chmodSync(outputPath, 0o755);
    }

    // Verify the file exists and has content
    const outputStat = statSync(outputPath);
    if (outputStat.size === 0) {
      throw new Error("Extracted binary is empty (0 bytes)");
    }

    console.log(`  Extracted to: ${relativePath}`);
    return true;
  } finally {
    // Clean up temp directory
    rmSync(tmpDir, { recursive: true, force: true });
  }
}

async function main() {
  const args = process.argv.slice(2);
  const currentPlatformOnly = args.includes("--current-platform-only");
  const tripleIndex = args.indexOf("--triple");
  const tripleArg = tripleIndex !== -1 ? args[tripleIndex + 1] : null;

  // Ensure output directory exists
  mkdirSync(BINARIES_DIR, { recursive: true });

  let targets;
  if (tripleArg) {
    targets = TARGETS.filter((t) => t.triple === tripleArg);
    if (targets.length === 0) {
      console.error(`Error: Unknown triple "${tripleArg}".`);
      console.error("Available triples:");
      for (const t of TARGETS) {
        console.error(`  - ${t.triple}`);
      }
      process.exit(1);
    }
    console.log(`Downloading for triple: ${tripleArg}\n`);
  } else if (currentPlatformOnly) {
    targets = getTargetsForCurrentPlatform();
    if (targets.length === 0) {
      console.warn(
        `Warning: No matching target for current platform (${process.platform}/${process.arch}).`,
      );
      console.warn("Available targets:");
      for (const t of TARGETS) {
        console.warn(`  - ${t.triple} (${t.nodePlatform}/${t.nodeArch})`);
      }
      process.exit(1);
    }
    console.log(
      `Downloading for current platform only: ${targets.map((t) => t.triple).join(", ")}\n`,
    );
  } else {
    targets = TARGETS;
    console.log(`Downloading ${BINARY_NAME} for all ${targets.length} platforms...\n`);
  }

  let succeeded = 0;
  let failed = 0;

  for (const target of targets) {
    try {
      const ok = await processTarget(target);
      if (ok) succeeded++;
    } catch (err) {
      failed++;
      console.error(
        `  WARNING: Failed for ${target.triple}: ${err.message}`,
      );
    }
    console.log();
  }

  console.log(
    `Done: ${succeeded}/${targets.length} platforms${failed > 0 ? ` (${failed} failed)` : ""}`,
  );

  if (failed > 0) {
    process.exit(1);
  }
}

main();
