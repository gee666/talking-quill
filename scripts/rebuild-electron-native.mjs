import { rebuild } from '@electron/rebuild';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const architecture = process.argv[2];
if (!['x64', 'arm64'].includes(architecture)) {
  throw new Error('usage: rebuild-electron-native.mjs x64|arm64');
}
const require = createRequire(import.meta.url);
const electronVersion = require('electron/package.json').version;
const appRoot = resolve(fileURLToPath(new URL('../app/', import.meta.url)));
await rebuild({
  buildPath: appRoot,
  electronVersion,
  arch: architecture,
  force: true,
  onlyModules: ['better-sqlite3'],
});
console.log(`Rebuilt better-sqlite3 for Electron ${electronVersion}/${architecture}`);
