import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { test } from 'node:test';

const root = resolve(import.meta.dirname, '../..');
const manifest = resolve(root, 'helper', 'Cargo.toml');
const cargo = rustTool('cargo');
const rustup = rustTool('rustup');
const overallDeadline = performance.now() + 20 * 60_000;
const targets = [
  'x86_64-pc-windows-msvc',
  'aarch64-pc-windows-msvc',
  'x86_64-apple-darwin',
  'aarch64-apple-darwin',
];

if (!['win32', 'darwin', 'linux'].includes(process.platform)) {
  throw new Error(`Unsupported Rust workspace architecture-gate platform: ${process.platform}`);
}

function rustTool(name) {
  const executable = process.platform === 'win32' ? `${name}.exe` : name;
  const candidate = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin', executable);
  return existsSync(candidate) ? candidate : executable;
}

function run(command, arguments_, target) {
  const remaining = Math.floor(overallDeadline - performance.now());
  if (remaining <= 0) {
    throw new Error('Rust workspace architecture gate exceeded its 20-minute overall deadline');
  }
  const result = spawnSync(command, arguments_, {
    cwd: root,
    encoding: 'utf8',
    timeout: Math.min(5 * 60_000, remaining),
    maxBuffer: 4 * 1024 * 1024,
    env: {
      ...process.env,
      CARGO_TARGET_DIR: resolve(root, 'tmp', `rust-workspace-${target}`),
    },
  });
  if (result.error !== undefined) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${basename(command)} ${arguments_.join(' ')} failed with ${String(result.status)}\n${result.stdout}\n${result.stderr}`,
    );
  }
}

function check(target, selection) {
  run(
    cargo,
    [
      'check',
      '--manifest-path',
      manifest,
      '--locked',
      '--all-targets',
      '--target',
      target,
      ...selection,
    ],
    target,
  );
}

function checkLocalUnsignedOwner(target) {
  run(
    cargo,
    [
      'check',
      '--manifest-path',
      manifest,
      '--locked',
      '--release',
      '--target',
      target,
      '-p',
      'talking-quill-keyboard-owner',
      '--features',
      'local-unsigned-owner',
      '--lib',
      '--bins',
    ],
    target,
  );
}

test(
  'workspace roles cross-compile for Windows and macOS x64/ARM64 or fail',
  { timeout: 20 * 60_000 },
  () => {
    for (const target of targets) {
      run(rustup, ['target', 'add', target], target);
      const featureConfigurations = target.endsWith('windows-msvc')
        ? [
            'transactional-shortcuts-dev',
            'windows-native-test-input',
            'transactional-shortcuts-dev,windows-native-test-input',
          ]
        : ['transactional-shortcuts-dev'];
      check(target, ['--workspace']);
      check(target, ['-p', 'talking-quill-helper']);
      check(target, ['-p', 'talking-quill-windows-owner-ipc']);
      checkLocalUnsignedOwner(target);
      for (const features of featureConfigurations) {
        check(target, ['-p', 'talking-quill-keyboard-owner', '--features', features]);
      }
    }
  },
);
