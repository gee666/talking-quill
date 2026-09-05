import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { access, lstat, readFile, readdir } from 'node:fs/promises';
import { relative, resolve, sep } from 'node:path';
import { describe, expect, it } from 'vitest';

interface CargoDependency {
  readonly name: string;
  readonly req: string;
  readonly path?: string | null;
  readonly source: string | null;
  readonly rename: string | null;
  readonly kind: string | null;
  readonly target: string | null;
  readonly features: readonly string[];
  readonly uses_default_features: boolean;
}

interface CargoTarget {
  readonly kind: readonly string[];
  readonly crate_types: readonly string[];
  readonly src_path: string;
  readonly 'required-features'?: readonly string[];
}

interface CargoPackage {
  readonly id: string;
  readonly name: string;
  readonly version: string;
  readonly edition: string;
  readonly rust_version: string | null;
  readonly manifest_path: string;
  readonly dependencies: readonly CargoDependency[];
  readonly features: Readonly<Record<string, readonly string[]>>;
  readonly targets: readonly CargoTarget[];
  readonly metadata: Readonly<Record<string, unknown>>;
}

interface CargoMetadata {
  readonly packages: readonly CargoPackage[];
  readonly workspace_members: readonly string[];
  readonly workspace_default_members: readonly string[];
}

interface DependencyPolicy {
  readonly name: string;
  readonly req: string;
  readonly source: string | null;
  readonly path: string | null;
  readonly rename: string | null;
  readonly kind: string | null;
  readonly target: string | null;
  readonly features: readonly string[];
  readonly usesDefaultFeatures: boolean;
}

const root = resolve(import.meta.dirname, '../..');
const cargoManifest = resolve(root, 'helper', 'Cargo.toml');
const cratesIo = 'registry+https://github.com/rust-lang/crates.io-index';
const macos = 'cfg(target_os = "macos")';
const windows = 'cfg(windows)';
const expectedPackages = new Map([
  ['talking-quill-helper', 'helper/Cargo.toml'],
  ['talking-quill-acceptance-signer', 'helper/acceptance-signer/Cargo.toml'],
  ['talking-quill-common-e2e', 'helper/common-e2e/Cargo.toml'],
  ['talking-quill-keyboard-core', 'helper/keyboard-core/Cargo.toml'],
  ['talking-quill-keyboard-owner', 'helper/keyboard-owner/Cargo.toml'],
  ['talking-quill-owner-protocol', 'helper/owner-protocol/Cargo.toml'],
  ['talking-quill-windows-owner-ipc', 'helper/windows-owner-ipc/Cargo.toml'],
]);

const ownerWindowsFeatures = [
  'Win32_Foundation',
  'Win32_Graphics_Gdi',
  'Win32_Media_Audio',
  'Win32_Security',
  'Win32_Security_Authentication_Identity',
  'Win32_Security_Authorization',
  'Win32_Security_Cryptography',
  'Win32_Security_Cryptography_Certificates',
  'Win32_Security_WinTrust',
  'Win32_Storage_FileSystem',
  'Win32_System_Com',
  'Win32_System_Console',
  'Win32_System_DataExchange',
  'Win32_System_IO',
  'Win32_System_LibraryLoader',
  'Win32_System_Memory',
  'Win32_System_Pipes',
  'Win32_System_Registry',
  'Win32_System_RemoteDesktop',
  'Win32_System_StationsAndDesktops',
  'Win32_System_SystemInformation',
  'Win32_System_Threading',
  'Win32_UI_Accessibility',
  'Win32_UI_HiDpi',
  'Win32_UI_Input_KeyboardAndMouse',
  'Win32_UI_WindowsAndMessaging',
].sort();

const windowsIpcFeatures = [
  'Win32_Foundation',
  'Win32_Security',
  'Win32_Security_Authorization',
  'Win32_Storage_FileSystem',
  'Win32_System_Com',
  'Win32_System_Pipes',
  'Win32_System_RemoteDesktop',
  'Win32_System_SystemInformation',
  'Win32_System_Threading',
  'Win32_UI_Shell',
].sort();

let cachedMetadata: CargoMetadata | undefined;

function portablePath(path: string): string {
  return relative(root, path).split(sep).join('/');
}

function metadata(): CargoMetadata {
  if (cachedMetadata !== undefined) return cachedMetadata;
  try {
    cachedMetadata = JSON.parse(
      execFileSync(
        'cargo',
        [
          'metadata',
          '--manifest-path',
          cargoManifest,
          '--locked',
          '--no-deps',
          '--format-version',
          '1',
        ],
        { cwd: root, encoding: 'utf8', timeout: 30_000, maxBuffer: 4 * 1024 * 1024 },
      ),
    ) as CargoMetadata;
  } catch (cause) {
    throw new Error('Cargo metadata failed or exceeded its 30-second boundary', { cause });
  }
  return cachedMetadata;
}

function workspacePackage(name: string): CargoPackage {
  const result = metadata().packages.find((candidate) => candidate.name === name);
  if (result === undefined) throw new Error(`Missing Cargo workspace package: ${name}`);
  return result;
}

function boundaryMetadata(name: string): Record<string, unknown> {
  const value = workspacePackage(name).metadata['talking-quill-boundary'];
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`Missing boundary metadata: ${name}`);
  }
  return value as Record<string, unknown>;
}

function policy(
  name: string,
  req: string,
  options: Partial<Omit<DependencyPolicy, 'name' | 'req'>> = {},
): DependencyPolicy {
  return {
    name,
    req,
    source: cratesIo,
    path: null,
    rename: null,
    kind: null,
    target: null,
    features: [],
    usesDefaultFeatures: true,
    ...options,
  };
}

function normalizedDependencies(name: string): DependencyPolicy[] {
  return workspacePackage(name)
    .dependencies.map((record) => ({
      name: record.name,
      req: record.req,
      source: record.source,
      path: record.path === undefined || record.path === null ? null : portablePath(record.path),
      rename: record.rename,
      kind: record.kind,
      target: record.target,
      features: [...record.features].sort(),
      usesDefaultFeatures: record.uses_default_features,
    }))
    .sort((left, right) => left.name.localeCompare(right.name));
}

function expectDependencies(name: string, expected: readonly DependencyPolicy[]): void {
  expect(normalizedDependencies(name), name).toEqual(
    [...expected].sort((left, right) => left.name.localeCompare(right.name)),
  );
}

async function rustSources(directory: string): Promise<string> {
  const absolute = resolve(root, directory);
  const entries = await readdir(absolute, { recursive: true, withFileTypes: true });
  const chunks: string[] = [];
  for (const entry of entries) {
    const path = resolve(entry.parentPath, entry.name);
    expect((await lstat(path)).isSymbolicLink(), portablePath(path)).toBe(false);
    if (entry.isFile() && entry.name.endsWith('.rs')) chunks.push(await readFile(path, 'utf8'));
  }
  return chunks.join('\n');
}

describe('Rust A3 package and compile-forbidden boundaries', () => {
  it('materializes seven packages with separate runtime and narrow signer executables', () => {
    expect(
      new Map(
        metadata().packages.map((record) => [record.name, portablePath(record.manifest_path)]),
      ),
    ).toEqual(expectedPackages);
    expect(metadata().workspace_default_members).toEqual([
      workspacePackage('talking-quill-helper').id,
      workspacePackage('talking-quill-keyboard-owner').id,
    ]);
    expect([...metadata().workspace_members].sort()).toEqual(
      metadata()
        .packages.map((record) => record.id)
        .sort(),
    );
    const sourceVersion = (
      JSON.parse(readFileSync(resolve('package.json'), 'utf8')) as { version: string }
    ).version;
    for (const record of metadata().packages) {
      expect(
        { version: record.version, edition: record.edition, rustVersion: record.rust_version },
        record.name,
      ).toEqual({ version: sourceVersion, edition: '2024', rustVersion: '1.97.1' });
    }
    expect(boundaryMetadata('talking-quill-acceptance-signer')).toEqual({
      role: 'acceptance-signer',
      runtime: false,
      executable: true,
      'private-key-reader': true,
    });
    expect(boundaryMetadata('talking-quill-helper')).toEqual({
      role: 'gateway',
      stage: 'a3-structural-gateway',
      runtime: true,
      'electron-protocol': 10,
      'owner-client': true,
      'can-suppress': false,
    });
    expect(boundaryMetadata('talking-quill-keyboard-core')).toEqual({
      role: 'keyboard-core',
      stage: 'a3-materialized',
      runtime: false,
    });
    expect(boundaryMetadata('talking-quill-keyboard-owner')).toEqual({
      role: 'keyboard-owner',
      stage: 'r4-production-runtime-host',
      runtime: true,
      executable: true,
      'owner-protocol-server': true,
    });
    expect(boundaryMetadata('talking-quill-windows-owner-ipc')).toEqual({
      role: 'windows-owner-ipc',
      runtime: false,
      executable: false,
      privileged: false,
    });
  });

  it('has the exact one-way package graph and owner-only native test features', () => {
    const internal = (name: string): string[] =>
      [
        ...new Set(
          workspacePackage(name)
            .dependencies.filter((dependency) => dependency.source === null)
            .map((dependency) => dependency.name),
        ),
      ].sort();
    expect(internal('talking-quill-acceptance-signer')).toEqual([
      'talking-quill-windows-owner-ipc',
    ]);
    expect(internal('talking-quill-helper')).toEqual([
      'talking-quill-keyboard-core',
      'talking-quill-owner-protocol',
      'talking-quill-windows-owner-ipc',
    ]);
    expect(internal('talking-quill-common-e2e')).toEqual([
      'talking-quill-helper',
      'talking-quill-keyboard-owner',
      'talking-quill-owner-protocol',
    ]);
    expect(internal('talking-quill-keyboard-core')).toEqual([]);
    expect(internal('talking-quill-keyboard-owner')).toEqual([
      'talking-quill-keyboard-core',
      'talking-quill-owner-protocol',
      'talking-quill-windows-owner-ipc',
    ]);
    expect(internal('talking-quill-owner-protocol')).toEqual([]);
    expect(internal('talking-quill-windows-owner-ipc')).toEqual([]);
    expect(workspacePackage('talking-quill-helper').features).toEqual({
      default: [],
      'machine-lock-test-namespace': [],
      'windows-installed-acceptance': [],
      'windows-update-recovery-launcher': [],
    });
    expect(workspacePackage('talking-quill-keyboard-core').features).toEqual({
      default: [],
      'native-test-input': [],
    });
    expect(workspacePackage('talking-quill-keyboard-owner').features).toEqual({
      default: [],
      'local-unsigned-owner': [],
      'macos-native-lifecycle-fixture': [],
      'transactional-shortcuts-dev': ['talking-quill-keyboard-core/native-test-input'],
      'windows-native-test-input': ['talking-quill-keyboard-core/native-test-input'],
    });
    expect(workspacePackage('talking-quill-owner-protocol').features).toEqual({
      default: [],
      'test-transport': [],
    });
    expect(workspacePackage('talking-quill-windows-owner-ipc').features).toEqual({
      default: [],
    });
    const ownerTargets = workspacePackage('talking-quill-keyboard-owner').targets;
    expect(ownerTargets.filter((target) => target.kind.includes('bin'))).toHaveLength(2);
    expect(
      ownerTargets.find(
        (target) => target.src_path === resolve(root, 'helper/keyboard-owner/src/main.rs'),
      )?.['required-features'] ?? [],
    ).toEqual([]);
    expect(
      ownerTargets.find(
        (target) =>
          target.src_path ===
          resolve(root, 'helper/keyboard-owner/src/bin/macos-keychain-fixture.rs'),
      )?.['required-features'],
    ).toEqual(['macos-native-lifecycle-fixture']);
  });

  it('compile-gates the macOS lifecycle fixture and rejects it from production packages', async () => {
    const [manifest, buildGate, helperBuild, inspector] = await Promise.all([
      readFile(resolve(root, 'helper/keyboard-owner/Cargo.toml'), 'utf8'),
      readFile(resolve(root, 'helper/keyboard-owner/build.rs'), 'utf8'),
      readFile(resolve(root, 'scripts/build-helper.mjs'), 'utf8'),
      readFile(resolve(root, 'scripts/inspect-package.mjs'), 'utf8'),
    ]);
    expect(manifest).toContain('required-features = ["macos-native-lifecycle-fixture"]');
    expect(buildGate).toContain('CARGO_FEATURE_MACOS_NATIVE_LIFECYCLE_FIXTURE');
    expect(buildGate).toContain('permissioned-ci-v1');
    expect(buildGate).toContain('drag-to-trash-fixture-');
    expect(helperBuild).toContain("macosOwnerFeatures.push('macos-native-lifecycle-fixture')");
    expect(inspector).toContain('Packaged owner contains lifecycle test hook');
    expect(inspector).toContain('TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE');
  });

  it('pins exact dependency source, path, kind, target, and feature policies', () => {
    expectDependencies('talking-quill-acceptance-signer', [
      policy('getrandom', '=0.4.3'),
      policy('p256', '=0.14.0', {
        features: ['ecdh', 'ecdsa', 'pkcs8', 'std'],
        usesDefaultFeatures: false,
      }),
      policy('serde', '=1.0.229', { features: ['derive'] }),
      policy('serde_json', '=1.0.150', { features: ['raw_value'] }),
      policy('sha2', '=0.11.0'),
      policy('talking-quill-windows-owner-ipc', '*', {
        source: null,
        path: 'helper/windows-owner-ipc',
        target: windows,
      }),
      policy('zeroize', '=1.9.0', { features: ['derive'], usesDefaultFeatures: false }),
      policy('windows-sys', '=0.61.2', {
        target: windows,
        features: [
          'Win32_Foundation',
          'Win32_Security',
          'Win32_Security_Authorization',
          'Win32_Storage_FileSystem',
          'Win32_System_Diagnostics_ToolHelp',
          'Win32_System_IO',
          'Win32_System_JobObjects',
          'Win32_System_Pipes',
          'Win32_System_RemoteDesktop',
          'Win32_System_SystemServices',
          'Win32_System_Threading',
          'Win32_UI_Input_KeyboardAndMouse',
        ],
      }),
    ]);
    expectDependencies('talking-quill-helper', [
      policy('core-foundation-sys', '=0.8.7', { target: macos }),
      policy('crossbeam-channel', '=0.5.16'),
      policy('getrandom', '=0.4.3'),
      policy('hmac', '=0.13.0', { features: ['zeroize'] }),
      policy('libc', '=0.2.186'),
      policy('p256', '=0.14.0', {
        features: ['ecdh', 'ecdsa', 'std'],
        usesDefaultFeatures: false,
      }),
      policy('serde', '=1.0.229', { features: ['derive'] }),
      policy('serde_json', '=1.0.150', { features: ['raw_value'] }),
      policy('security-framework-sys', '=2.17.0', {
        target: macos,
        features: ['macos-12'],
      }),
      policy('sha2', '=0.11.0'),
      policy('thiserror', '=2.0.19'),
      policy('zeroize', '=1.9.0', { features: ['derive'], usesDefaultFeatures: false }),
      policy('talking-quill-keyboard-core', '*', {
        source: null,
        path: 'helper/keyboard-core',
      }),
      policy('talking-quill-owner-protocol', '*', {
        source: null,
        path: 'helper/owner-protocol',
      }),
      policy('talking-quill-windows-owner-ipc', '*', {
        source: null,
        path: 'helper/windows-owner-ipc',
        target: windows,
      }),
      policy('talking-quill-owner-protocol', '*', {
        source: null,
        path: 'helper/owner-protocol',
        kind: 'dev',
        features: ['test-transport'],
      }),
      policy('proptest', '=1.11.0', { kind: 'dev' }),
      policy('libc', '=0.2.186', { target: macos }),
      policy('windows-sys', '=0.61.2', {
        target: windows,
        features: [
          'Win32_Foundation',
          'Win32_Security',
          'Win32_Security_Authorization',
          'Win32_Storage_FileSystem',
          'Win32_System_Com',
          'Win32_System_Console',
          'Win32_System_Diagnostics_ToolHelp',
          'Win32_System_IO',
          'Win32_System_JobObjects',
          'Win32_System_Pipes',
          'Win32_System_Registry',
          'Win32_System_RemoteDesktop',
          'Win32_System_SystemInformation',
          'Win32_System_Threading',
          'Win32_UI_Shell',
          'Win32_UI_WindowsAndMessaging',
        ],
      }),
    ]);
    expectDependencies('talking-quill-keyboard-core', [
      policy('serde', '=1.0.229', { features: ['derive'] }),
      policy('thiserror', '=2.0.19'),
      policy('proptest', '=1.11.0', { kind: 'dev' }),
      policy('serde_json', '=1.0.150', { kind: 'dev', features: ['raw_value'] }),
    ]);
    expectDependencies('talking-quill-keyboard-owner', [
      policy('core-foundation-sys', '=0.8.7', { target: macos }),
      policy('crossbeam-channel', '=0.5.16'),
      policy('getrandom', '=0.4.3', { kind: 'dev' }),
      policy('getrandom', '=0.4.3', { target: macos }),
      policy('hmac', '=0.13.0', { features: ['zeroize'] }),
      policy('serde', '=1.0.229', { features: ['derive'] }),
      policy('serde_json', '=1.0.150', { features: ['raw_value'] }),
      policy('sha1', '=0.10.6', { target: macos }),
      policy('sha2', '=0.11.0'),
      policy('thiserror', '=2.0.19'),
      policy('zeroize', '=1.9.0', { features: ['derive'], usesDefaultFeatures: false }),
      policy('talking-quill-keyboard-core', '*', {
        source: null,
        path: 'helper/keyboard-core',
      }),
      policy('talking-quill-owner-protocol', '*', {
        source: null,
        path: 'helper/owner-protocol',
      }),
      policy('talking-quill-owner-protocol', '*', {
        source: null,
        path: 'helper/owner-protocol',
        kind: 'dev',
        features: ['test-transport'],
      }),
      policy('talking-quill-windows-owner-ipc', '*', {
        source: null,
        path: 'helper/windows-owner-ipc',
        target: windows,
      }),
      policy('proptest', '=1.11.0', { kind: 'dev' }),
      policy('libc', '=0.2.186', { target: macos }),
      policy('security-framework-sys', '=2.17.0', {
        target: macos,
        features: ['macos-12'],
      }),
      policy('windows-sys', '=0.61.2', { target: windows, features: ownerWindowsFeatures }),
    ]);
    const protocol = normalizedDependencies('talking-quill-owner-protocol');
    expect(protocol.map(({ name }) => name)).toEqual(
      [
        'getrandom',
        'hkdf',
        'hmac',
        'p256',
        'serde',
        'serde_json',
        'sha2',
        'subtle',
        'thiserror',
        'zeroize',
      ].sort(),
    );
    expectDependencies('talking-quill-windows-owner-ipc', [
      policy('getrandom', '=0.4.3'),
      policy('serde', '=1.0.229', { features: ['derive'] }),
      policy('serde_json', '=1.0.150', { features: ['raw_value'] }),
      policy('sha2', '=0.11.0'),
      policy('subtle', '=2.6.1', { usesDefaultFeatures: false, features: ['std'] }),
      policy('thiserror', '=2.0.19'),
      policy('zeroize', '=1.9.0', { features: ['derive'], usesDefaultFeatures: false }),
      policy('windows-sys', '=0.61.2', { target: windows, features: windowsIpcFeatures }),
    ]);
  });

  it.each([
    ['talking-quill-helper', 'transactional-shortcuts-dev'],
    ['talking-quill-helper', 'windows-native-test-input'],
    ['talking-quill-windows-owner-ipc', 'transactional-shortcuts-dev'],
    ['talking-quill-windows-owner-ipc', 'windows-native-test-input'],
  ])(
    'compile-forbids %s from selecting owner feature %s',
    (packageName, feature) => {
      const result = spawnSync(
        'cargo',
        [
          'check',
          '--manifest-path',
          cargoManifest,
          '--locked',
          '-p',
          packageName,
          '--features',
          feature,
        ],
        { cwd: root, encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024 },
      );
      expect(result.error).toBeUndefined();
      expect(result.status).not.toBe(0);
      expect(`${result.stdout}${result.stderr}`).toMatch(
        new RegExp(`does not (?:have|contain).*feature[^\\n]*${feature}`, 'u'),
      );
    },
    35_000,
  );

  it.each(['transactional-shortcuts-dev', 'windows-native-test-input'])(
    'forbids local unsigned artifacts from including the %s seam',
    (testFeature) => {
      const result = spawnSync(
        'cargo',
        [
          'check',
          '--manifest-path',
          cargoManifest,
          '--locked',
          '-p',
          'talking-quill-keyboard-owner',
          '--features',
          `local-unsigned-owner,${testFeature}`,
        ],
        { cwd: root, encoding: 'utf8', timeout: 30_000, maxBuffer: 1024 * 1024 },
      );
      expect(result.error).toBeUndefined();
      expect(result.status).not.toBe(0);
      expect(`${result.stdout}${result.stderr}`).toContain(
        'the local unsigned owner mode cannot contain native test seams',
      );
    },
    35_000,
  );

  it('keeps source modules inside their package and forbids reverse/native gateway imports', async () => {
    const [gateway, core, owner, ipc] = await Promise.all([
      rustSources('helper/src'),
      rustSources('helper/keyboard-core/src'),
      rustSources('helper/keyboard-owner/src'),
      rustSources('helper/windows-owner-ipc/src'),
    ]);
    for (const [name, source] of [
      ['gateway', gateway],
      ['core', core],
      ['owner', owner],
      ['ipc', ipc],
    ] as const) {
      expect(source, name).not.toMatch(/#\s*\[\s*path\s*=|\binclude(?:_bytes|_str)?!\s*\(/u);
    }
    const codeOnly = (source: string): string =>
      source.replace(/\/\/.*$/gmu, '').replace(/\/\*[\s\S]*?\*\//gu, '');
    expect(codeOnly(gateway)).not.toMatch(
      /NativePlatform|SetWindowsHookEx|SendInput|CGEventTapCreate|CGEventPost|talking_quill_keyboard_owner|windows-native-test-input|transactional-shortcuts-dev/u,
    );
    expect(codeOnly(core)).not.toMatch(
      /\bunsafe\b|windows_sys|security_framework|crossbeam|jsonrpc|talking_quill_(?:helper|keyboard_owner|owner_protocol)/u,
    );
    expect(codeOnly(owner)).not.toMatch(
      /talking_quill_helper|crate::protocol|CriticalDelivery|jsonrpc|RpcResponse|RequestId/u,
    );
    expect(owner).toContain('pub enum NativeEvent');
    expect(owner).toContain('NativeEvent::Keyboard');
    expect(codeOnly(ipc)).not.toMatch(
      /NativePlatform|SetWindowsHookEx|SendInput|CGEventTapCreate|paste\.inject|configure\.activation/u,
    );
    await expect(access(resolve(root, 'helper/gateway'))).rejects.toThrow();
    await expect(access(resolve(root, 'helper/src/platform'))).rejects.toThrow();
    await expect(access(resolve(root, 'helper/src/keyboard'))).rejects.toThrow();
  });

  it('keeps permanent markers truthful and always stages the enabled local owner separately', async () => {
    const [rootLib, ownerLib, build, inspect, packageManifest] = await Promise.all([
      readFile(resolve(root, 'helper/src/gateway.rs'), 'utf8'),
      readFile(resolve(root, 'helper/keyboard-owner/src/build_mode.rs'), 'utf8'),
      readFile(resolve(root, 'scripts/build-helper.mjs'), 'utf8'),
      readFile(resolve(root, 'scripts/inspect-package.mjs'), 'utf8'),
      readFile(resolve(root, 'package.json'), 'utf8'),
    ]);
    expect(rootLib).toContain('TALKING_QUILL_KEYBOARD_GATEWAY=PROTOCOL_V1_GATEWAY_CANNOT_SUPPRESS');
    expect(ownerLib).toContain(
      'TALKING_QUILL_KEYBOARD_OWNER=SAFE_DISABLED_ALL_PHYSICAL_EVENTS_PASS',
    );
    expect(build).toContain("'talking-quill-helper'");
    expect(build).not.toContain('transactional-shortcuts-dev');
    expect(build).toContain("argument === '--include-macos-owner'");
    expect(build).toContain('R9 always stages the enabled owner');
    expect(build).toContain("'local-unsigned-owner'");
    expect(build).toContain('verifyStagedNativeRoleSet');
    expect(inspect).toContain('verifyOwnerBuildContract');
    expect(inspect).toContain('verifyCompleteNativeRoleInventory');
    expect(inspect).toContain("process.argv.includes('--macos-owner')");
    const scripts = (JSON.parse(packageManifest) as { scripts: Record<string, string> }).scripts;
    expect(scripts['test:helper']).toContain('--workspace');
    expect(scripts['test:helper:transactional-dev']).toContain('-p talking-quill-keyboard-owner');
    expect(scripts['test:helper:transactional-dev']).not.toContain('-p talking-quill-helper');
    expect(scripts['test:helper:arm64-compile']).toBe(
      'node scripts/check-helper-architectures.mjs',
    );
  });

  it('keeps raw physical keyboard diagnostics out of production and scripts', async () => {
    const windowsModule = await readFile(
      resolve(root, 'helper/keyboard-owner/src/platform/windows.rs'),
      'utf8',
    );
    const hook = await readFile(
      resolve(root, 'helper/keyboard-owner/src/platform/windows/hook.rs'),
      'utf8',
    );
    expect(windowsModule).not.toContain('physical_diagnostic');
    expect(hook).not.toMatch(/PhysicalDiagnostic|TalkingQuillPhysicalHotkeyDiagnostic/u);
    await expect(
      access(resolve(root, 'scripts/arm-windows-physical-hotkey-diagnostic.ps1')),
    ).rejects.toThrow();
    await expect(
      access(resolve(root, 'scripts/query-windows-physical-hotkey-diagnostic.ps1')),
    ).rejects.toThrow();
  });

  it('removes obsolete native subprocess harnesses from the workspace', async () => {
    const owner = workspacePackage('talking-quill-keyboard-owner');
    expect(
      owner.targets.some(({ src_path }) => src_path.includes(`${sep}tests${sep}legacy${sep}`)),
    ).toBe(false);
    await expect(
      access(resolve(root, 'helper/keyboard-owner/tests/legacy/windows_native_harness.rs')),
    ).rejects.toThrow();
    await expect(
      access(resolve(root, 'helper/keyboard-owner/tests/legacy/macos_native_harness.rs')),
    ).rejects.toThrow();
  });
});
