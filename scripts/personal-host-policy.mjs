import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve } from 'node:path';

import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

export function readMacSigningConfiguration(source = process.env) {
  const path =
    source.TALKING_QUILL_MACOS_LOCAL_CONFIG ??
    resolve(
      homedir(),
      'Library',
      'Application Support',
      'Talking Quill Local Build',
      'signing.json',
    );
  if (!existsSync(path)) throw new Error(`Run pnpm personal:mac:setup first; missing ${path}`);
  const value = JSON.parse(readFileSync(path, 'utf8'));
  const required = [
    'TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256',
    'TALKING_QUILL_MACOS_POLICY_CMS_IDENTITY',
    'TALKING_QUILL_MACOS_INSTALLATION_ID',
    'TALKING_QUILL_MACOS_LOCAL_IDENTITY',
    'TALKING_QUILL_MACOS_LOCAL_CERT_SHA256',
    'TALKING_QUILL_MACOS_LOCAL_CERT_SHA1',
  ];
  if (
    value === null ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some((name) => !required.includes(name))
  ) {
    throw new Error('macOS local configuration contains unknown fields');
  }
  for (const name of required)
    if (typeof value[name] !== 'string' || value[name] === '')
      throw new Error(`Invalid macOS local configuration: ${name}`);
  for (const name of [
    'TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256',
    'TALKING_QUILL_MACOS_INSTALLATION_ID',
    'TALKING_QUILL_MACOS_LOCAL_CERT_SHA256',
  ]) {
    if (!/^[0-9a-f]{64}$/u.test(value[name]))
      throw new Error(`Invalid macOS local digest: ${name}`);
  }
  if (!/^[0-9a-f]{40}$/u.test(value.TALKING_QUILL_MACOS_LOCAL_CERT_SHA1)) {
    throw new Error('Invalid macOS local SHA-1 certificate fingerprint');
  }
  const mode = source.TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE ?? 'self-signed';
  if (!['self-signed', 'adhoc'].includes(mode)) throw new Error('Invalid local signing mode');
  return Object.fromEntries([
    ...required.map((name) => [name, value[name]]),
    ['TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE', mode],
  ]);
}

export function requireNotRegisteredMacStatus(status, output) {
  if (status !== 0 || output.trim() !== '0') {
    throw new Error('Fresh installation requires authoritative SMAppService notRegistered status');
  }
}

export function requireFreshMacAbsence(serviceBridge, environment) {
  const service = 'com.talkingquill.app.keyboard-owner';
  for (const account of ['owner-ipc-v1', 'maintenance-latch-v1']) {
    const result = spawnSync(
      '/usr/bin/security',
      ['find-generic-password', '-s', service, '-a', account],
      { env: environment },
    );
    if (result.status === 0) {
      throw new Error(
        `Fresh installation refuses existing Keyboard Owner Keychain item: ${account}`,
      );
    }
    if (result.status !== 44) throw new Error('Could not prove Keyboard Owner Keychain absence');
  }
  const ownerRoot = resolve(
    homedir(),
    'Library',
    'Application Support',
    'Talking Quill',
    'KeyboardOwner',
  );
  if (existsSync(ownerRoot)) {
    throw new Error(`Fresh installation refuses pending owner state or cleanup: ${ownerRoot}`);
  }
  const serviceStatus = spawnSync(serviceBridge, ['status'], {
    encoding: 'utf8',
    env: environment,
  });
  requireNotRegisteredMacStatus(serviceStatus.status, serviceStatus.stdout);
  const processes = spawnSync('/bin/ps', ['-axo', 'command='], {
    encoding: 'utf8',
    env: environment,
  });
  if (
    processes.status !== 0 ||
    /talking-quill-(?:helper|keyboard-owner)|Talking Quill Keyboard Owner/u.test(processes.stdout)
  ) {
    throw new Error(
      'Fresh installation cannot prove that previous gateway/owner processes are absent',
    );
  }
}

export function waitForExactMacProcesses(paths, environment) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const result = spawnSync('/bin/ps', ['-axo', 'command='], {
      encoding: 'utf8',
      env: environment,
    });
    const commands = result.stdout.split('\n');
    if (
      result.status === 0 &&
      paths.every((path) =>
        commands.some((command) => command === path || command.startsWith(`${path} `)),
      )
    )
      return;
    spawnSync('/bin/sleep', ['1'], { env: environment });
  }
  throw new Error(`Timed out waiting for exact installed processes: ${paths.join(', ')}`);
}

export function detectPhysicalMacArchitecture({
  environment = sanitizedSubprocessEnvironment(),
  spawnProcess = spawnSync,
  sysctlCommand = { executable: '/usr/sbin/sysctl', arguments: [] },
  unameCommand = { executable: '/usr/bin/uname', arguments: [] },
} = {}) {
  const translated = spawnProcess(
    sysctlCommand.executable,
    [...sysctlCommand.arguments, '-in', 'sysctl.proc_translated'],
    { encoding: 'utf8', env: environment },
  );
  const machine = spawnProcess(unameCommand.executable, [...unameCommand.arguments, '-m'], {
    encoding: 'utf8',
    env: environment,
  });
  if (machine.status !== 0) throw new Error('Cannot determine physical Mac architecture');
  return translated.status === 0 && translated.stdout.trim() === '1'
    ? 'arm64'
    : machine.stdout.trim() === 'x86_64'
      ? 'x64'
      : machine.stdout.trim();
}

export const PERSONAL_TARGETS = Object.freeze({
  win: { packageTarget: 'win-unsigned', platform: 'win', architecture: 'x64' },
  'win-arm64': { packageTarget: 'win-arm64-unsigned', platform: 'win', architecture: 'arm64' },
  'mac-x64': { packageTarget: 'mac-owner-x64', platform: 'mac', architecture: 'x64' },
  'mac-arm64': { packageTarget: 'mac-owner-arm64', platform: 'mac', architecture: 'arm64' },
});

export function sanitizePersonalConsumerEnvironment(source = process.env) {
  return sanitizedSubprocessEnvironment(source);
}

export function createFreshEnvironment(configuration, source = process.env) {
  const environment = sanitizePersonalConsumerEnvironment(source);
  if (configuration.platform === 'mac') {
    Object.assign(environment, readMacSigningConfiguration(source));
  }
  environment.TALKING_QUILL_PACKAGE_MODE = 'fresh';
  environment.TALKING_QUILL_PERSONAL_FRESH_INSTALL = '1';
  return environment;
}

export function requireHost(configuration, requireNativeArchitecture) {
  const expected = configuration.platform === 'win' ? 'win32' : 'darwin';
  if (process.platform !== expected)
    throw new Error(
      `${configuration.platform} personal-use commands require a native ${expected} host`,
    );
  if (configuration.platform === 'win') {
    if (requireNativeArchitecture && process.arch !== configuration.architecture) {
      throw new Error(
        `Installed ${configuration.architecture} verification requires matching Windows hardware; this host is ${process.arch}`,
      );
    }
    return;
  }
  if (configuration.platform === 'mac') {
    const physical = detectPhysicalMacArchitecture();
    if (physical !== configuration.architecture) {
      throw new Error(
        `${configuration.architecture} personal-use command requires matching physical hardware; this Mac is ${physical}`,
      );
    }
  }
}
