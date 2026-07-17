#!/usr/bin/env node
'use strict';

// Postinstall script for @navi-agent/navi.
//
// When a platform-specific optionalDependency is installed, this is a no-op.
// When installing from the wrapper alone (e.g., `npm pack` without platform
// packages), this script attempts to download the binary from GitHub Releases.

const https = require('node:https');
const fs = require('node:fs');
const path = require('node:path');
const { execSync } = require('node:child_process');

const platform = process.platform;
const arch = process.arch;
const platformArch = `${platform}-${arch}`;

// Check if the platform binary is already available (via optionalDependency)
function hasPlatformBinary() {
  try {
    require.resolve(`@navi-agent/navi-${platformArch}/package.json`);
    return true;
  } catch {
    return false;
  }
}

// Also check if a local binary exists next to the package root
function hasLocalBinary() {
  const binName = platform === 'win32' ? 'navi.exe' : 'navi';
  return fs.existsSync(path.join(__dirname, '..', binName));
}

if (hasPlatformBinary() || hasLocalBinary()) {
  // Binary already available — nothing to do
  process.exit(0);
}

// ── Fallback: download from GitHub Releases ──────────────────────────────────

const PACKAGE_VERSION = require('../package.json').version;
const REPO = 'navi-ai-org/navi';

function getArchiveName() {
  const ext = platform === 'win32' ? 'zip' : 'tar.gz';
  return `navi-${platformArch}.${ext}`;
}

function getDownloadUrl(version) {
  return `https://github.com/${REPO}/releases/download/v${version}/${getArchiveName()}`;
}

function download(url) {
  return new Promise((resolve, reject) => {
    const follow = (redirectUrl, depth = 0) => {
      if (depth > 5) return reject(new Error('Too many redirects'));
      https.get(redirectUrl, (res) => {
        if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
          follow(res.headers.location, depth + 1);
          return;
        }
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode}`));
          return;
        }
        const chunks = [];
        res.on('data', (chunk) => chunks.push(chunk));
        res.on('end', () => resolve(Buffer.concat(chunks)));
        res.on('error', reject);
      }).on('error', reject);
    };
    follow(url);
  });
}

async function main() {
  const version = PACKAGE_VERSION;
  const url = getDownloadUrl(version);

  console.log(`@navi-agent/navi: downloading navi v${version} for ${platformArch}...`);

  try {
    const archive = await download(url);
    const tmpDir = require('node:os').tmpdir();
    const archivePath = path.join(tmpDir, `${platformArch}-${version}-${getArchiveName()}`);

    fs.writeFileSync(archivePath, archive);

    // Extract
    if (platform === 'win32') {
      execSync(`powershell -Command "Expand-Archive -Path '${archivePath}' -DestinationPath '${path.join(__dirname, '..')}' -Force"`, { stdio: 'inherit' });
    } else {
      execSync(`tar -xzf "${archivePath}" -C "${path.join(__dirname, '..')}"`, { stdio: 'inherit' });
    }

    // Clean up
    try { fs.unlinkSync(archivePath); } catch {}

    console.log(`@navi-agent/navi: installed navi binary for ${platformArch}`);
  } catch (err) {
    console.warn(`@navi-agent/navi: could not download prebuilt binary: ${err.message}`);
    console.warn('');
    console.warn('The navi CLI may not be available. You can install it via:');
    console.warn('  cargo install navi-cli');
    console.warn(`  curl -fsSL https://github.com/navi-ai-org/navi/raw/refs/heads/main/scripts/install.sh | sh`);
    // Don't fail install — the user might install the binary separately
  }
}

main();
