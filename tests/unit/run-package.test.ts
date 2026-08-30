import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  CANONICAL_PACKAGE_TARGETS,
  createPackagePlan,
  createProductionEnvironment,
} from '../../scripts/run-package.mjs';

const expectedPlans = {
  win: ['package:win', 'nsis', 'win', 'x64'],
  'win-arm64': ['package:win:arm64', 'nsis', 'win', 'arm64'],
  'win-dir': ['package:win:dir', 'none', 'win', 'x64'],
  'win-arm64-dir': ['package:win:arm64:dir', 'none', 'win', 'arm64'],
  'win-unsigned': ['package:win', 'nsis', 'win', 'x64'],
  'win-arm64-unsigned': ['package:win:arm64:unsigned', 'nsis', 'win', 'arm64'],
  'mac-x64': ['package:mac:x64', 'dmg-zip', 'mac', 'x64'],
  'mac-arm64': ['package:mac:arm64', 'dmg-zip', 'mac', 'arm64'],
  'mac-x64-unsigned': ['package:mac:x64', 'dmg-zip', 'mac', 'x64'],
  'mac-arm64-unsigned': ['package:mac:arm64', 'dmg-zip', 'mac', 'arm64'],
  'mac-owner-x64': ['package:mac:owner:x64', 'dmg-zip', 'mac', 'x64'],
  'mac-owner-arm64': ['package:mac:owner:arm64', 'dmg-zip', 'mac', 'arm64'],
} as const;

describe('package orchestration', () => {
  it('selects both Windows architectures as canonical release targets', () => {
    expect(CANONICAL_PACKAGE_TARGETS).toEqual(['win', 'win-arm64']);
    for (const target of CANONICAL_PACKAGE_TARGETS) {
      const plan = createPackagePlan(target);
      expect(plan.artifactRequirement).not.toBe('none');
    }
  });

  it('keeps each target command and security identity in one descriptor', () => {
    for (const [target, [command, artifactRequirement, platform, architecture]] of Object.entries(
      expectedPlans,
    )) {
      expect(createPackagePlan(target)).toEqual({
        command,
        artifactRequirement,
        platform,
        architecture,
        mode: target.endsWith('-dir') ? 'directory-test' : 'update',
        pnpmArguments: ['--filter', '@talking-quill/app', command],
      });
    }
    for (const invalid of [
      undefined,
      'unknown',
      'mac-owner',
      'mac-owner-universal',
      '__proto__',
      'constructor',
      'toString',
    ]) {
      expect(() => createPackagePlan(invalid)).toThrow(
        'mac-owner-x64, mac-owner-arm64, or an unsigned variant',
      );
    }
  });

  it('keeps acceptance packages noncanonical and strips authorization from canonical builds', () => {
    const acceptance = createPackagePlan('win-installed-acceptance');
    expect(acceptance).toMatchObject({ architecture: 'x64', acceptance: true });
    expect(CANONICAL_PACKAGE_TARGETS).not.toContain('win-installed-acceptance');
    expect(
      createProductionEnvironment(acceptance, {
        TALKING_QUILL_ACCEPTANCE_MANIFEST_PRIVATE_KEY_PEM: 'private',
      }),
    ).toMatchObject({
      TALKING_QUILL_ACCEPTANCE_BUILD: '1',
      TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD: '1',
      TALKING_QUILL_ACCEPTANCE_MANIFEST_PRIVATE_KEY_PEM: 'private',
    });
    expect(
      createProductionEnvironment(createPackagePlan('win'), {
        TALKING_QUILL_ACCEPTANCE_BUILD: '1',
        TALKING_QUILL_ACCEPTANCE_MANIFEST_PRIVATE_KEY_PEM: 'private',
      }),
    ).not.toHaveProperty('TALKING_QUILL_ACCEPTANCE_BUILD');
  });

  it.each(['mac-owner-x64', 'mac-owner-arm64'] as const)(
    'creates a strict sanitized production environment for %s',
    (target) => {
      const plan = createPackagePlan(target);
      const environment = createProductionEnvironment(plan, {
        PATH: '/reviewed/path',
        TALKING_QUILL_MACOS_LOCAL_IDENTITY: 'retained-signing-input',
        TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE: 'must-not-cross-production-boundary',
        TALKING_QUILL_TASK6_TEST_HARNESS: 'must-not-cross-production-boundary',
        TALKING_QUILL_UNRELATED_HARNESS_FLAG: 'must-not-cross-production-boundary',
      });
      expect(environment).toMatchObject({
        PATH: '/reviewed/path',
        CSC_IDENTITY_AUTO_DISCOVERY: 'false',
        TALKING_QUILL_PACKAGE_INSPECTION_STRICT: '1',
        TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: 'dmg-zip',
        TALKING_QUILL_PACKAGE_TARGET: 'mac',
        TALKING_QUILL_PACKAGE_ARCH: target.endsWith('x64') ? 'x64' : 'arm64',
        TALKING_QUILL_MACOS_LOCAL_IDENTITY: 'retained-signing-input',
      });
      expect(environment).not.toHaveProperty('TALKING_QUILL_MACOS_TEST_SUCCESSOR_NONCE');
      expect(environment).not.toHaveProperty('TALKING_QUILL_TASK6_TEST_HARNESS');
      expect(environment).not.toHaveProperty('TALKING_QUILL_UNRELATED_HARNESS_FLAG');
    },
  );

  it('delegates prepackage and inspection exactly once to every app package command', async () => {
    const [appManifest, rootManifest] = await Promise.all([
      readFile(resolve('app/package.json'), 'utf8').then(
        (source) => JSON.parse(source) as { scripts: Record<string, string> },
      ),
      readFile(resolve('package.json'), 'utf8').then(
        (source) => JSON.parse(source) as { scripts: Record<string, string> },
      ),
    ]);
    const [orchestrator, prepackage] = await Promise.all([
      readFile(resolve('scripts/run-package.mjs'), 'utf8'),
      readFile(resolve('scripts/prepackage-check.mjs'), 'utf8'),
    ]);
    expect(orchestrator).not.toContain('prepackage-check.mjs');
    expect(orchestrator).not.toContain('package:inspect');
    for (const [command] of Object.values(expectedPlans)) {
      expect(appManifest.scripts[command]).toContain('scripts/prepackage-check.mjs');
      expect(appManifest.scripts[command]).toContain('scripts/inspect-package.mjs');
    }
    for (const command of ['package:win']) {
      expect(appManifest.scripts[command]).toContain('electron-builder.unsigned.yml');
    }
    for (const command of ['package:mac:x64', 'package:mac:arm64']) {
      expect(appManifest.scripts[command]).toContain('--include-macos-owner');
      expect(appManifest.scripts[command]).toContain('electron-builder.macos-owner.yml');
      expect(appManifest.scripts[command]).toContain('--macos-owner');
    }
    for (const script of Object.values(appManifest.scripts)) {
      expect(script).not.toContain('-c.compression=store');
    }
    expect(rootManifest.scripts['package:mac:owner:x64']).toBe(
      'node scripts/run-package.mjs mac-owner-x64',
    );
    expect(rootManifest.scripts['package:mac:owner:arm64']).toBe(
      'node scripts/run-package.mjs mac-owner-arm64',
    );
    for (const command of ['package:mac:owner:x64', 'package:mac:owner:arm64']) {
      expect(prepackage).toContain(`case '${command}':`);
      expect(appManifest.scripts[command]).toContain('scripts/prepackage-check.mjs');
      expect(appManifest.scripts[command]).toContain('scripts/inspect-package.mjs');
      expect(appManifest.scripts[command]).toContain('--macos-owner');
    }
  });
});
