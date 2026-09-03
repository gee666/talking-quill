import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HEX_SIGNATURE = /^[0-9a-f]{128}$/u;
const HEX_SEC1 = /^04[0-9a-f]{128}$/u;

export function signAcceptancePayload({
  signerPath,
  privateKeyPath,
  payloadBytes,
  signerSha256,
  spawnProcess = spawnSync,
}) {
  if (
    !Buffer.isBuffer(payloadBytes) ||
    payloadBytes.length === 0 ||
    payloadBytes.length > 64 * 1024
  ) {
    throw new Error('Acceptance signing payload is invalid');
  }
  const executable = resolve(signerPath);
  const metadata = lstatSync(executable);
  const executableBytes = readFileSync(executable);
  if (
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.nlink !== 1 ||
    !/^[0-9a-f]{64}$/u.test(signerSha256 ?? '') ||
    createHash('sha256').update(executableBytes).digest('hex') !== signerSha256
  ) {
    throw new Error('Native acceptance signer identity is invalid');
  }
  mkdirSync(resolve('tmp'), { recursive: true, mode: 0o700 });
  const snapshotRoot = mkdtempSync(resolve('tmp/acceptance-signer-'));
  const snapshotPath = resolve(snapshotRoot, 'acceptance-signer.exe');
  let result;
  try {
    writeFileSync(snapshotPath, executableBytes, { flag: 'wx', mode: 0o700 });
    if (createHash('sha256').update(readFileSync(snapshotPath)).digest('hex') !== signerSha256) {
      throw new Error('Native acceptance signer snapshot is invalid');
    }
    result = spawnProcess(snapshotPath, ['--private-key', resolve(privateKeyPath)], {
      cwd: resolve('.'),
      env: Object.fromEntries(
        Object.entries({ SystemRoot: process.env.SystemRoot, WINDIR: process.env.WINDIR }).filter(
          ([, value]) => value !== undefined,
        ),
      ),
      input: payloadBytes,
      encoding: 'utf8',
      windowsHide: true,
      timeout: 10_000,
      maxBuffer: 4 * 1024,
    });
  } finally {
    rmSync(snapshotRoot, { recursive: true, force: true });
  }
  if (result?.status !== 0 || result.stderr !== '') {
    throw new Error('Narrow native acceptance signer failed');
  }
  const lines = result.stdout.split('\n');
  if (
    lines.length !== 3 ||
    !HEX_SIGNATURE.test(lines[0]) ||
    !HEX_SEC1.test(lines[1]) ||
    lines[2] !== ''
  ) {
    throw new Error('Narrow native acceptance signer output is invalid');
  }
  const sec1 = Buffer.from(lines[1], 'hex');
  const spki = Buffer.concat([
    Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
    sec1,
  ]);
  return Object.freeze({
    signatureBase64url: Buffer.from(lines[0], 'hex').toString('base64url'),
    publicKeySpkiBase64url: spki.toString('base64url'),
  });
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  throw new Error('Use the native RFC6979 signer; this module is only its process adapter');
}
