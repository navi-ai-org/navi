/**
 * prepublish-check.mjs
 *
 * Final validation before `npm publish`. Ensures the package is complete.
 */

import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(__dirname, '..');

let ok = true;
const errors = [];
const warnings = [];

function error(msg) {
  errors.push(msg);
  ok = false;
}

function warn(msg) {
  warnings.push(msg);
}

// ── Check required files ──────────────────────────────────────────────────

for (const file of ['index.js', 'index.d.ts', 'README.md', 'docs/navi-napi-guide.md']) {
  if (!existsSync(join(packageDir, file))) {
    error(`Missing required file: ${file}`);
  }
}

// ── Check platform binaries ───────────────────────────────────────────────

const npmDir = join(packageDir, 'npm');
const expectedPlatforms = [
  'linux-x64',
  'linux-arm64',
  'darwin-x64',
  'darwin-arm64',
  'win32-x64',
];

const foundPlatforms = [];

for (const plat of expectedPlatforms) {
  const pkgPath = join(npmDir, plat, 'package.json');
  if (!existsSync(pkgPath)) {
    warn(`Missing platform package: npm/${plat}/package.json`);
    continue;
  }

  const binaryPath = join(npmDir, plat, `navi.${plat}.node`);
  if (existsSync(binaryPath)) {
    foundPlatforms.push(plat);
  } else {
    warn(`Platform package npm/${plat} exists but has no binary`);
  }
}

if (foundPlatforms.length === 0) {
  warn(
    'No platform binaries found. Consumers will need Rust toolchain to build from source.',
  );
} else {
  console.log(`Platform binaries: ${foundPlatforms.join(', ')}`);
}

// ── Check package.json consistency ────────────────────────────────────────

const pkg = JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf-8'));

const optionalDeps = pkg.optionalDependencies || {};
for (const plat of expectedPlatforms) {
  const depName = `@navi-agent/napi-${plat}`;
  if (!optionalDeps[depName]) {
    warn(`package.json missing optionalDependency: ${depName}`);
  }
}

if (pkg.license !== 'Apache-2.0') {
  error(`License should be Apache-2.0, got: ${pkg.license}`);
}

if (!pkg.publishConfig?.access) {
  warn('Missing publishConfig.access — scoped package may fail to publish');
}

// ── Summary ───────────────────────────────────────────────────────────────

for (const w of warnings) {
  console.warn(`  WARNING: ${w}`);
}

for (const e of errors) {
  console.error(`  ERROR: ${e}`);
}

if (!ok) {
  console.error('\nprepublish-check failed. Fix errors before publishing.');
  process.exit(1);
}

console.log('\nprepublish-check passed.');
