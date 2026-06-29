/**
 * prepack.mjs
 *
 * Runs before `npm pack` / `npm publish`. Validates that the package is
 * ready for publishing:
 *
 * 1. index.js and index.d.ts exist
 * 2. At least one npm/ platform package has a binary (unless --allow-empty)
 * 3. package.json has required fields
 */

import { existsSync, readdirSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const packageDir = resolve(__dirname, '..');

const allowEmpty = process.argv.includes('--allow-empty');

let ok = true;

function check(condition, message) {
  if (!condition) {
    console.error(`prepack: FAIL — ${message}`);
    ok = false;
  }
}

// 1. Required files
check(existsSync(join(packageDir, 'index.js')), 'index.js not found');
check(existsSync(join(packageDir, 'index.d.ts')), 'index.d.ts not found');
check(existsSync(join(packageDir, 'README.md')), 'README.md not found');

// 2. At least one platform binary
if (!allowEmpty) {
  const npmDir = join(packageDir, 'npm');
  if (existsSync(npmDir)) {
    const platforms = readdirSync(npmDir).filter((d) =>
      existsSync(join(npmDir, d, `navi.${d}.node`)),
    );
    if (platforms.length === 0) {
      console.warn(
        'prepack: WARNING — no platform binaries found in npm/.\n' +
          'Run `npm run build` or build with --target for each platform.\n' +
          'Pass --allow-empty to skip this check.',
      );
    } else {
      console.log(`prepack: found binaries for: ${platforms.join(', ')}`);
    }
  }
}

// 3. package.json fields
const pkg = JSON.parse(
  (await import('node:fs')).readFileSync(join(packageDir, 'package.json'), 'utf-8'),
);

check(pkg.name, 'package.json missing "name"');
check(pkg.version, 'package.json missing "version"');
check(pkg.license, 'package.json missing "license"');
check(pkg.main, 'package.json missing "main"');
check(pkg.types, 'package.json missing "types"');

if (!ok) {
  console.error('\nprepack: package is not ready for publishing. Fix the issues above.');
  process.exit(1);
}

console.log('prepack: all checks passed.');
