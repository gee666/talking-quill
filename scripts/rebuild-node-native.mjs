import { rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';

const appRequire = createRequire(resolve('app', 'package.json'));
const packageFile = appRequire.resolve('better-sqlite3/package.json');
const packageRoot = dirname(packageFile);
const packageRequire = createRequire(packageFile);

try {
  rmSync(join(packageRoot, 'build'), {
    recursive: true,
    force: true,
    maxRetries: 20,
    retryDelay: 250,
  });
} catch (error) {
  if (error?.code === 'EPERM' || error?.code === 'EBUSY' || error?.code === 'EACCES') {
    throw new Error(
      'better-sqlite3 is still loaded by a process; source E2E Electron cleanup did not complete',
      { cause: error },
    );
  }
  throw error;
}

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
