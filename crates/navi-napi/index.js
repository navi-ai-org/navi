'use strict';

const fs = require('node:fs');
const path = require('node:path');

const platform = process.platform;
const arch = process.arch;
const platformArch = `${platform}-${arch}`;

// ---------------------------------------------------------------------------
// Binary resolution order:
//
// 1. NAVI_NAPI_BINARY env var (explicit override, for development or CI).
// 2. Platform-specific optionalDependency package
//    (@navi-agent/napi-<platform>-<arch>), installed automatically by npm when
//    the prebuilt binary exists for the current platform.
// 3. Local prebuilt binary: nav.<platform>-<arch>.node next to index.js.
// 4. Generic local binary: nav.node next to index.js.
// 5. Workspace target/ directory (development, debug then release).
// ---------------------------------------------------------------------------

/** @type {string[]} */
const candidates = [];

// 1. Explicit env override
if (process.env.NAVI_NAPI_BINARY) {
  candidates.push(process.env.NAVI_NAPI_BINARY);
}

// 2. Platform-specific optionalDependency package
try {
  const pkg = `@navi-agent/napi-${platformArch}`;
  const resolved = require.resolve(pkg);
  candidates.push(resolved);
} catch {
  // optionalDependency not installed — expected on unsupported platforms
}

// 3-4. Local prebuilt binaries
candidates.push(
  path.join(__dirname, `navi.${platformArch}.node`),
  path.join(__dirname, 'navi.node'),
);

// 5. Workspace target/ (development only)
candidates.push(
  path.join(__dirname, '..', '..', 'target', 'release', nativeLibraryName()),
  path.join(__dirname, '..', '..', 'target', 'debug', nativeLibraryName()),
);

// ---------------------------------------------------------------------------
// Try each candidate
// ---------------------------------------------------------------------------

let lastError;
for (const candidate of candidates) {
  if (!candidate || !fs.existsSync(candidate)) {
    continue;
  }
  try {
    module.exports = require(candidate);
    return;
  } catch (error) {
    lastError = error;
  }
}

// ---------------------------------------------------------------------------
// All candidates exhausted — build from source or provide helpful error
// ---------------------------------------------------------------------------

const searched = candidates.map((c) => `  - ${c}`).join('\n');

// Check if cargo is available for a build-from-source fallback
const { spawnSync } = require('node:child_process');
const cargoCheck = spawnSync('cargo', ['--version'], { stdio: 'ignore' });

if (cargoCheck.status === 0) {
  // Attempt automatic build from source
  console.warn(
    `@navi-agent/napi: no prebuilt binary found for ${platformArch}. ` +
      'Building from source (this may take a while)...',
  );

  const workspaceRoot = path.join(__dirname, '..', '..');
  const buildResult = spawnSync('cargo', ['build', '-p', 'navi-napi', '--release'], {
    cwd: fs.existsSync(workspaceRoot) ? workspaceRoot : __dirname,
    stdio: 'inherit',
    env: process.env,
  });

  if (buildResult.status === 0) {
    // Try to load the freshly built binary
    const source = path.join(
      fs.existsSync(workspaceRoot) ? workspaceRoot : __dirname,
      'target',
      'release',
      nativeLibraryName(),
    );
    if (fs.existsSync(source)) {
      try {
        module.exports = require(source);
        return;
      } catch (error) {
        lastError = error;
      }
    }
  }
}

// ---------------------------------------------------------------------------

const lines = [
  'Unable to load @navi-agent/napi native binding.',
  '',
  'No prebuilt binary is available for your platform:',
  `  platform: ${platform}`,
  `  arch:     ${arch}`,
  '',
  'Searched:',
  searched,
];

if (lastError) {
  lines.push('', `Last error: ${lastError.message}`);
}

lines.push(
  '',
  'To resolve this:',
  '  1. Install a prebuilt binary package for your platform, or',
  '  2. Install Rust (https://rustup.rs) and run `npm run build` in crates/navi-napi, or',
  '  3. Set NAVI_NAPI_BINARY to the absolute path of your compiled .node file.',
);

throw new Error(lines.join('\n'));

// ---------------------------------------------------------------------------

function nativeLibraryName() {
  if (platform === 'win32') {
    return 'navi_napi.dll';
  }
  if (platform === 'darwin') {
    return 'libnavi_napi.dylib';
  }
  return 'libnavi_napi.so';
}
