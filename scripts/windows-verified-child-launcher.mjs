import { spawn, spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

const HEX = /^[0-9a-f]{64}$/u;

export function verifiedChildArguments(bootstrap, child, timeoutMs) {
  if (
    !HEX.test(bootstrap?.sha256 ?? '') ||
    !Number.isSafeInteger(bootstrap?.bytes) ||
    bootstrap.bytes <= 0 ||
    !HEX.test(child?.sha256 ?? '') ||
    !Number.isSafeInteger(child?.bytes) ||
    child.bytes <= 0 ||
    !Number.isSafeInteger(timeoutMs) ||
    timeoutMs <= 0 ||
    timeoutMs > 80 * 60 * 1_000 ||
    !Array.isArray(child.arguments ?? []) ||
    (child.arguments ?? []).length > 32 ||
    (child.arguments ?? []).some(
      (argument) =>
        typeof argument !== 'string' || argument.length > 32_768 || argument.includes('\0'),
    )
  ) {
    throw new Error('Verified native child identity is invalid');
  }
  return [
    '--windows-installed-acceptance-verified-child-v1',
    bootstrap.sha256,
    String(bootstrap.bytes),
    resolve(child.path),
    child.sha256,
    String(child.bytes),
    String(timeoutMs),
    '0',
    '--',
    ...(child.arguments ?? []),
  ];
}

export function launchVerifiedChildSync({ bootstrap, child, timeoutMs, input, maxBuffer }) {
  return spawnSync(resolve(bootstrap.path), verifiedChildArguments(bootstrap, child, timeoutMs), {
    cwd: resolve('.'),
    env: sanitizedSubprocessEnvironment({
      SystemRoot: process.env.SystemRoot,
      WINDIR: process.env.WINDIR,
    }),
    input,
    encoding: 'utf8',
    windowsHide: true,
    timeout: timeoutMs,
    maxBuffer,
  });
}

export function launchVerifiedChild({ bootstrap, child, timeoutMs }) {
  return spawn(resolve(bootstrap.path), verifiedChildArguments(bootstrap, child, timeoutMs), {
    shell: false,
    windowsHide: true,
    stdio: ['pipe', 'pipe', 'pipe'],
    env: sanitizedSubprocessEnvironment(),
  });
}
