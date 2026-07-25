import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { existsSync, readFileSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import { resolve, join } from 'node:path';

const root = resolve('tmp/tests/pi-npm');
const prefix = join(root, 'prefix');
const packageRoot = join(prefix, 'node_modules/@earendil-works/pi-coding-agent');
const receiptPath = join(root, 'receipt.json');
const action = process.argv[2] ?? 'verify';

if (action === 'cleanup') {
  rmSync(root, { recursive: true, force: true });
  console.log(`Removed ${root}`);
  process.exit(0);
}
if (action === 'install') {
  rmSync(root, { recursive: true, force: true });
  mkdirSync(root, { recursive: true });
  const npmArgs = [
    'install',
    '-g',
    '--prefix',
    prefix,
    '--ignore-scripts',
    '--no-audit',
    '--no-fund',
    '@earendil-works/pi-coding-agent@0.81.1',
  ];
  const result =
    process.platform === 'win32'
      ? spawnSync(
          resolve(process.env.SystemRoot ?? 'C:\\Windows', 'System32/cmd.exe'),
          ['/d', '/s', '/c', `npm ${npmArgs.map(quoteCmdArgument).join(' ')}`],
          { cwd: root, encoding: 'utf8', windowsVerbatimArguments: true, timeout: 180_000 },
        )
      : spawnSync('npm', npmArgs, { cwd: root, encoding: 'utf8', timeout: 180_000 });
  if (result.status !== 0)
    throw new Error(`Owned Pi fixture install failed: ${result.stderr || result.stdout}`);
  writeReceipt();
}
const receipt = verifyReceipt();
if (action === 'env') {
  console.log(`TALKING_QUILL_REAL_NPM_PI_PREFIX=${receipt.prefix}`);
} else {
  console.log(JSON.stringify(receipt));
}

function writeReceipt() {
  const manifestPath = join(packageRoot, 'package.json');
  const cliPath = join(packageRoot, 'dist/cli.js');
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  if (manifest.name !== '@earendil-works/pi-coding-agent' || manifest.version !== '0.81.1')
    throw new Error('Owned Pi fixture package identity mismatch');
  const cli = readFileSync(cliPath);
  const shim = process.platform === 'win32' ? join(prefix, 'pi.cmd') : join(prefix, 'bin/pi');
  if (!existsSync(shim)) throw new Error(`Owned Pi fixture shim missing: ${shim}`);
  writeFileSync(
    receiptPath,
    JSON.stringify(
      {
        schemaVersion: 1,
        token: randomUUID(),
        packageName: manifest.name,
        version: manifest.version,
        prefix,
        packageRoot,
        shim,
        cliSha256: createHash('sha256').update(cli).digest('hex'),
      },
      null,
      2,
    ),
  );
}

function verifyReceipt() {
  const receipt = JSON.parse(readFileSync(receiptPath, 'utf8'));
  const manifest = JSON.parse(readFileSync(join(receipt.packageRoot, 'package.json'), 'utf8'));
  const cli = readFileSync(join(receipt.packageRoot, 'dist/cli.js'));
  if (
    receipt.schemaVersion !== 1 ||
    receipt.packageName !== '@earendil-works/pi-coding-agent' ||
    receipt.version !== '0.81.1' ||
    manifest.name !== receipt.packageName ||
    manifest.version !== receipt.version ||
    createHash('sha256').update(cli).digest('hex') !== receipt.cliSha256 ||
    !existsSync(receipt.shim)
  )
    throw new Error('Owned Pi fixture receipt verification failed');
  return Object.freeze(receipt);
}

function quoteCmdArgument(value) {
  if (/^[A-Za-z0-9@./:_-]+$/u.test(value)) return value;
  if (/["%!?&|<>^\r\n]/u.test(value)) throw new Error('Unsafe owned fixture argument');
  return `"${value}"`;
}
