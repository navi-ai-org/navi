/**
 * postinstall.mjs
 *
 * Runs after `npm install` for consumers who install @navi-agent/napi without a
 * prebuilt binary for their platform. If cargo is available, this script
 * attempts a release build from source so the package works out of the box.
 *
 * When a prebuilt binary IS available (either as an optionalDependency or as a
 * local .node file), this script exits silently.
 */

import { existsSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(__dirname, '..');
const platform = process.platform;
const arch = process.arch;
const platformArch = `${platform}-${arch}`;

// ── Check if a binary already exists ──────────────────────────────────────

function nativeLibraryName() {
  if (platform === 'win32') return 'navi_napi.dll';
  if (platform === 'darwin') return 'libnavi_napi.dylib';
  return 'libnavi_napi.so';
}

function binaryExists() {
  // 1. NAVI_NAPI_BINARY env var
  if (process.env.NAVI_NAPI_BINARY && existsSync(process.env.NAVI_NAPI_BINARY)) {
    return true;
  }

  // 2. optionalDependency package
  try {
    const resolved = require.resolve(`@navi-agent/napi-${platformArch}`);
    if (existsSync(resolved)) return true;
  } catch {
    // not installed
  }

  // 3. Local prebuilt
  if (existsSync(join(packageDir, `navi.${platformArch}.node`))) return true;
  if (existsSync(join(packageDir, 'navi.node'))) return true;

  // 4. Workspace target (dev mode)
  const workspaceRoot = join(packageDir, '..', '..');
  if (existsSync(join(workspaceRoot, 'target', 'release', nativeLibraryName()))) return true;
  if (existsSync(join(workspaceRoot, 'target', 'debug', nativeLibraryName()))) return true;

  return false;
}

if (binaryExists()) {
  process.exit(0);
}

// ── No binary found — try building from source ────────────────────────────

const cargoCheck = spawnSync('cargo', ['--version'], { stdio: 'ignore' });
if (cargoCheck.status !== 0) {
  // No cargo — cannot build from source. The loader in index.js will throw
  // a helpful error at require() time.
  console.warn(
    [
      '@navi-agent/napi: no prebuilt binary available for ' + platformArch + '.',
      'Install Rust (https://rustup.rs) and re-install, or set NAVI_NAPI_BINARY.',
      'The package will not work until a native binary is available.',
    ].join('\n'),
  );
  process.exit(0);
}

console.warn(
  `@navi-agent/napi: no prebuilt binary for ${platformArch}. Building from source...`,
);

const workspaceRoot = join(packageDir, '..', '..');
const cwd = existsSync(join(workspaceRoot, 'Cargo.toml')) ? workspaceRoot : packageDir;

const result = spawnSync('cargo', ['build', '-p', 'navi-napi', '--release'], {
  cwd,
  stdio: 'inherit',
  env: process.env,
});

if (result.status !== 0) {
  console.warn(
    [
      '@navi-agent/napi: cargo build failed. The package will not work until you',
      'manually build the native binary or install a prebuilt one.',
      'Run `npm run build` in crates/navi-napi to retry.',
    ].join('\n'),
  );
  // Do not fail install — let index.js throw at require() time with a
  // detailed error message instead of breaking npm install for the project.
  process.exit(0);
}

// Copy the built binary into the package directory
import { copyFileSync, mkdirSync } from 'node:fs';

const targetRoot = process.env.CARGO_TARGET_DIR
  ? resolve(cwd, process.env.CARGO_TARGET_DIR)
  : join(cwd, 'target');

const source = join(targetRoot, 'release', nativeLibraryName());
if (existsSync(source)) {
  const dest = join(packageDir, `navi.${platformArch}.node`);
  mkdirSync(packageDir, { recursive: true });
  copyFileSync(source, dest);
  console.log(`@navi-agent/napi: built and installed binary → ${dest}`);
} else {
  console.warn(`@navi-agent/napi: build succeeded but binary not found at ${source}`);
}
