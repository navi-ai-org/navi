/**
 * build-native.mjs
 *
 * Builds the navi-napi native addon and copies the resulting binary into the
 * package directory with a platform-specific filename.
 *
 * Usage:
 *   node scripts/build-native.mjs                    # debug build for host
 *   node scripts/build-native.mjs --release           # release build for host
 *   node scripts/build-native.mjs --target x86_64-unknown-linux-gnu
 *   node scripts/build-native.mjs --release --target aarch64-apple-darwin
 *
 * Environment variables:
 *   NODE_ENV=production    Same as --release
 *   CARGO_TARGET_DIR       Custom cargo target directory
 *   NAVI_NAPI_OUT_DIR      Override output directory (default: package dir)
 */

import { copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(__dirname, '..');
const workspaceRoot = resolve(packageDir, '..', '..');

// ── Parse CLI args ────────────────────────────────────────────────────────

const args = process.argv.slice(2);
const release = args.includes('--release') || process.env.NODE_ENV === 'production';

const targetIdx = args.indexOf('--target');
const targetTriple = targetIdx !== -1 ? args[targetIdx + 1] : null;

const strip = args.includes('--strip');

// ── Resolve platform binary name ──────────────────────────────────────────

function inferPlatform(triple) {
  if (!triple) return { platform: process.platform, arch: process.arch };

  if (triple.includes('linux')) return { platform: 'linux' };
  if (triple.includes('apple-darwin') || triple.includes('darwin'))
    return { platform: 'darwin' };
  if (triple.includes('windows') || triple.includes('win32'))
    return { platform: 'win32' };
  return { platform: process.platform };
}

function inferArch(triple) {
  if (!triple) return process.arch;

  if (triple.startsWith('x86_64') || triple.includes('x64')) return 'x64';
  if (triple.startsWith('aarch64') || triple.includes('arm64')) return 'arm64';
  return process.arch;
}

const { platform } = inferPlatform(targetTriple);
const arch = inferArch(targetTriple);
const profile = release ? 'release' : 'debug';

// ── Build with cargo ──────────────────────────────────────────────────────

const cargoArgs = ['build', '-p', 'navi-napi'];

if (release) {
  cargoArgs.push('--release');
}

if (targetTriple) {
  cargoArgs.push('--target', targetTriple);
}

console.log(`Building navi-napi (${profile}${targetTriple ? `, target=${targetTriple}` : ''})...`);

const result = spawnSync('cargo', cargoArgs, {
  cwd: workspaceRoot,
  stdio: 'inherit',
  env: process.env,
});

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

// ── Locate the built library ──────────────────────────────────────────────

function nativeLibraryName() {
  if (platform === 'win32') return 'navi_napi.dll';
  if (platform === 'darwin') return 'libnavi_napi.dylib';
  return 'libnavi_napi.so';
}

const targetRoot = process.env.CARGO_TARGET_DIR
  ? resolve(workspaceRoot, process.env.CARGO_TARGET_DIR)
  : join(workspaceRoot, 'target');

// When cross-compiling with --target, cargo puts the output in
// target/<triple>/<profile>/ instead of target/<profile>/
const source = targetTriple
  ? join(targetRoot, targetTriple, profile, nativeLibraryName())
  : join(targetRoot, profile, nativeLibraryName());

if (!existsSync(source)) {
  console.error(`Error: native library was not produced at ${source}`);
  process.exit(1);
}

// ── Strip debug symbols (optional, release only) ──────────────────────────

if (strip && release) {
  const stripResult = spawnSync('strip', [source], { stdio: 'inherit' });
  if (stripResult.status !== 0) {
    console.warn('Warning: strip failed (non-fatal, continuing)');
  }
}

// ── Copy to output directory ──────────────────────────────────────────────

const outDir = process.env.NAVI_NAPI_OUT_DIR || packageDir;
mkdirSync(outDir, { recursive: true });

const dest = join(outDir, `navi.${platform}-${arch}.node`);
copyFileSync(source, dest);
console.log(`Wrote ${dest}`);

// ── Also copy to the matching npm/ platform package if it exists ──────────

const npmPlatformDir = join(packageDir, 'npm', `${platform}-${arch}`);
if (existsSync(npmPlatformDir)) {
  const npmDest = join(npmPlatformDir, `navi.${platform}-${arch}.node`);
  copyFileSync(source, npmDest);
  console.log(`Wrote ${npmDest} (npm platform package)`);
}
