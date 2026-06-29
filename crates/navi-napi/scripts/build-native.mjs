import { copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const workspaceRoot = resolve(packageDir, '..', '..');
const release = process.argv.includes('--release') || process.env.NODE_ENV === 'production';
const profile = release ? 'release' : 'debug';

const cargoArgs = ['build', '-p', 'navi-napi'];
if (release) {
  cargoArgs.push('--release');
}

const result = spawnSync('cargo', cargoArgs, {
  cwd: workspaceRoot,
  stdio: 'inherit',
  env: process.env,
});

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

const targetRoot = process.env.CARGO_TARGET_DIR
  ? resolve(workspaceRoot, process.env.CARGO_TARGET_DIR)
  : join(workspaceRoot, 'target');
const source = join(targetRoot, profile, nativeLibraryName());
if (!existsSync(source)) {
  throw new Error(`Native library was not produced: ${source}`);
}

mkdirSync(packageDir, { recursive: true });
const platformBinary = join(packageDir, `navi.${process.platform}-${process.arch}.node`);
copyFileSync(source, platformBinary);
console.log(`Wrote ${platformBinary}`);

function nativeLibraryName() {
  if (process.platform === 'win32') {
    return 'navi_napi.dll';
  }
  if (process.platform === 'darwin') {
    return 'libnavi_napi.dylib';
  }
  return 'libnavi_napi.so';
}
