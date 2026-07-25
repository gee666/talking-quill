import { rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';

const appRequire = createRequire(resolve('app', 'package.json'));
const packageFile = appRequire.resolve('better-sqlite3/package.json');
const packageRoot = dirname(packageFile);
const packageRequire = createRequire(packageFile);

rmSync(join(packageRoot, 'build'), {
  recursive: true,
  force: true,
  maxRetries: 5,
  retryDelay: 100,
});

const prebuildInstall = packageRequire.resolve('prebuild-install/bin.js');
let result = spawnSync(process.execPath, [prebuildInstall], {
  cwd: packageRoot,
  encoding: 'utf8',
  stdio: 'inherit',
});
if (result.status !== 0) {
  const nodeGyp = packageRequire.resolve('node-gyp/bin/node-gyp.js');
  result = spawnSync(process.execPath, [nodeGyp, 'rebuild', '--release'], {
    cwd: packageRoot,
    encoding: 'utf8',
    stdio: 'inherit',
  });
}
if (result.status !== 0) throw new Error('Unable to restore the Node.js native module ABI');
console.log('Restored better-sqlite3 for the host Node.js ABI');
