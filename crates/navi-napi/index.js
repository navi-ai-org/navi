'use strict';

const fs = require('node:fs');
const path = require('node:path');

const platform = process.platform;
const arch = process.arch;

const candidates = [
  process.env.NAVI_NAPI_BINARY,
  path.join(__dirname, `navi.${platform}-${arch}.node`),
  path.join(__dirname, 'navi.node'),
  path.join(__dirname, '..', '..', 'target', 'release', nativeLibraryName()),
  path.join(__dirname, '..', '..', 'target', 'debug', nativeLibraryName()),
].filter(Boolean);

let lastError;
for (const candidate of candidates) {
  if (!fs.existsSync(candidate)) {
    continue;
  }
  try {
    module.exports = require(candidate);
    return;
  } catch (error) {
    lastError = error;
  }
}

const searched = candidates.map((candidate) => `  - ${candidate}`).join('\n');
const message = [
  'Unable to load @navi/napi native binding.',
  'Run `npm run build` in crates/navi-napi, or set NAVI_NAPI_BINARY.',
  'Searched:',
  searched,
  lastError ? `Last error: ${lastError.message}` : null,
].filter(Boolean).join('\n');

throw new Error(message);

function nativeLibraryName() {
  if (platform === 'win32') {
    return 'navi_napi.dll';
  }
  if (platform === 'darwin') {
    return 'libnavi_napi.dylib';
  }
  return 'libnavi_napi.so';
}
