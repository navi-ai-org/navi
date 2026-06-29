#!/usr/bin/env node
'use strict';

// CLI wrapper — resolves the platform-specific navi binary and exec's it.
// This file is what `npm install -g @navi-agent/navi` puts in PATH as `navi`.

const { execFileSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const platform = process.platform;
const arch = process.arch;
const platformArch = `${platform}-${arch}`;
const isWindows = platform === 'win32';

// ---------------------------------------------------------------------------
// Binary resolution order:
//
// 1. NAVI_BINARY env var (explicit override).
// 2. Platform-specific optionalDependency (@navi-agent/navi-<platform>-<arch>).
// 3. Local binary next to this script.
// ---------------------------------------------------------------------------

function findBinary() {
  const candidates = [];

  // 1. Explicit env override
  if (process.env.NAVI_BINARY) {
    candidates.push(process.env.NAVI_BINARY);
  }

  // 2. Platform-specific optionalDependency
  try {
    const pkg = `@navi-agent/navi-${platformArch}`;
    const pkgDir = path.dirname(require.resolve(`${pkg}/package.json`));
    const binName = isWindows ? 'navi.exe' : 'navi';
    candidates.push(path.join(pkgDir, binName));
  } catch {
    // optionalDependency not installed — expected on unsupported platforms
  }

  // 3. Local binary next to this script
  const localBin = isWindows ? 'navi.exe' : 'navi';
  candidates.push(path.join(__dirname, '..', localBin));

  for (const candidate of candidates) {
    if (candidate && fs.existsSync(candidate)) {
      return candidate;
    }
  }

  return null;
}

const binary = findBinary();

if (!binary) {
  console.error(
    [
      'Unable to find the navi binary.',
      '',
      `Platform: ${platform} ${arch}`,
      '',
      'This usually means the prebuilt binary for your platform was not installed.',
      '',
      'To resolve this:',
      '  1. Try reinstalling: npm install -g @navi-agent/navi',
      '  2. Or install via cargo: cargo install navi-cli',
      '  3. Or use the shell installer: curl -fsSL https://raw.githubusercontent.com/navi-ai-org/navi/main/scripts/install.sh | sh',
      '',
      `Expected package: @navi-agent/navi-${platformArch}`,
    ].join('\n'),
  );
  process.exit(1);
}

// Forward all arguments to the real binary
const args = process.argv.slice(2);

try {
  const result = isWindows
    ? execFileSync(binary, args, { stdio: 'inherit' })
    : execFileSync(binary, args, { stdio: 'inherit' });
  process.exit(0);
} catch (err) {
  if (err.status !== undefined) {
    process.exit(err.status);
  }
  throw err;
}
