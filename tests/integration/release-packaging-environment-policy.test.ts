import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import {
  createFreshEnvironment,
  PERSONAL_TARGETS,
  sanitizePersonalConsumerEnvironment,
} from '../../scripts/personal-use.mjs';
import { createPackagePlan, createProductionEnvironment } from '../../scripts/run-package.mjs';

const poisonedProducerEnvironment = {
  Path: '/discarded/path',
  PATH: '/reviewed/path',
  TALKING_QUILL_PACKAGE_MODE: 'update',
  TALKING_QUILL_PACKAGE_VARIANT: 'directory-test',
  TALKING_QUILL_PERSONAL_FRESH_INSTALL: '1',
  TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT: '1',
  TALKING_QUILL_NATIVE_FAULT_PHASE: 'terminalAcceptance',
  TALKING_QUILL_ACCEPTANCE_BUILD: '1',
  TALKING_QUILL_TASK6_TEST_HARNESS: '1',
  TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64: 'private',
  TALKING_QUILL_PREDECESSOR_VERSION: '0.0.68',
  TALKING_QUILL_PREDECESSOR_RELEASE_BUILD: 'a'.repeat(64),
  TALKING_QUILL_MACOS_PREDECESSOR_GATEWAY_SHA256: 'b'.repeat(64),
  talking_quill_predecessor_owner_sha256: 'c'.repeat(64),
  Talking_Quill_Native_Fault_Phase: 'commit',
  talking_quill_acceptance_request_private_key: 'private',
  Talking_Quill_Windows_Update_Signing_Key_Pkcs8_Base64: 'private',
};

function runEnvironmentChild(source: string) {
  const result = spawnSync(process.execPath, ['--input-type=module', '--eval', source], {
    cwd: process.cwd(),
    encoding: 'utf8',
    env: {
      ...process.env,
      NODE_ENV: 'test',
      talking_quill_personal_fresh_install: '1',
      Talking_Quill_Predecessor_Version: '0.0.68',
      talking_quill_native_fault_phase: 'terminalAcceptance',
      Talking_Quill_Acceptance_Build: '1',
      talking_quill_windows_update_signing_key_pkcs8_base64: 'private',
    },
  });
  expect(result.status, result.stderr).toBe(0);
  return JSON.parse(result.stdout) as Record<string, unknown>;
}

function expectNoCaseVariantDuplicates(environment: Record<string, unknown>) {
  const names = Object.keys(environment).map((name) => name.toUpperCase());
  expect(new Set(names).size).toBe(names.length);
}

describe('release packaging environment policy', () => {
  it('creates a coherent fresh producer environment and strips stale producer authority', () => {
    const environment = createFreshEnvironment(PERSONAL_TARGETS.win, poisonedProducerEnvironment);
    expect(environment).toMatchObject({
      PATH: '/reviewed/path',
      TALKING_QUILL_PACKAGE_MODE: 'fresh',
      TALKING_QUILL_PERSONAL_FRESH_INSTALL: '1',
    });
    for (const name of [
      'TALKING_QUILL_PACKAGE_VARIANT',
      'TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT',
      'TALKING_QUILL_NATIVE_FAULT_PHASE',
      'TALKING_QUILL_ACCEPTANCE_BUILD',
      'TALKING_QUILL_TASK6_TEST_HARNESS',
      'TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64',
      'TALKING_QUILL_PREDECESSOR_VERSION',
      'TALKING_QUILL_PREDECESSOR_RELEASE_BUILD',
      'TALKING_QUILL_MACOS_PREDECESSOR_GATEWAY_SHA256',
      'talking_quill_predecessor_owner_sha256',
      'Talking_Quill_Native_Fault_Phase',
      'talking_quill_acceptance_request_private_key',
      'Talking_Quill_Windows_Update_Signing_Key_Pkcs8_Base64',
    ]) {
      expect(environment).not.toHaveProperty(name);
    }
  });

  it('removes producer-only mode, predecessor, and marker values from personal consumers', () => {
    const environment = sanitizePersonalConsumerEnvironment(poisonedProducerEnvironment);
    expect(environment).toEqual({ PATH: '/reviewed/path' });
    expectNoCaseVariantDuplicates(environment);
  });

  it.each(['win-dir', 'win-arm64-dir'] as const)(
    'keeps %s fresh, directory-only, and outside the promotable target list',
    (target) => {
      const plan = createPackagePlan(target);
      const environment = createProductionEnvironment(plan, poisonedProducerEnvironment);
      expect(plan).toMatchObject({
        artifactRequirement: 'none',
        directoryTest: true,
        mode: 'fresh',
      });
      expect(environment).toMatchObject({
        TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: 'none',
        TALKING_QUILL_PACKAGE_VARIANT: 'directory-test',
        TALKING_QUILL_PACKAGE_MODE: 'fresh',
        TALKING_QUILL_PERSONAL_FRESH_INSTALL: '1',
      });
      expect(environment).not.toHaveProperty('TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT');
      expect(Object.keys(environment)).not.toContain('TALKING_QUILL_PREDECESSOR_VERSION');
      expectNoCaseVariantDuplicates(environment);
    },
  );

  it('sanitizes mixed-case personal environment keys across a real child process', () => {
    const environment = runEnvironmentChild(`
      import { sanitizePersonalConsumerEnvironment } from './scripts/personal-use.mjs';
      process.stdout.write(JSON.stringify(sanitizePersonalConsumerEnvironment()));
    `);
    expect(environment).not.toHaveProperty('TALKING_QUILL_PERSONAL_FRESH_INSTALL');
    expect(
      Object.keys(environment).some((name) =>
        /PREDECESSOR|FAULT|ACCEPTANCE|SIGNING_KEY/iu.test(name),
      ),
    ).toBe(false);
    expectNoCaseVariantDuplicates(environment);
  });

  it('sanitizes mixed-case package environment keys across a real child process', () => {
    const environment = runEnvironmentChild(`
      import { createPackagePlan, createProductionEnvironment } from './scripts/run-package.mjs';
      process.stdout.write(JSON.stringify(createProductionEnvironment(createPackagePlan('win'))));
    `);
    expect(environment).toMatchObject({
      TALKING_QUILL_PACKAGE_MODE: 'fresh',
      TALKING_QUILL_PERSONAL_FRESH_INSTALL: '1',
      TALKING_QUILL_PACKAGE_VARIANT: 'canonical',
    });
    expect(
      Object.keys(environment).some((name) =>
        /PREDECESSOR|FAULT|ACCEPTANCE|SIGNING_KEY/iu.test(name),
      ),
    ).toBe(false);
    expectNoCaseVariantDuplicates(environment);
  });

  it('binds Windows installer mode to validated artifact metadata, not caller environment', async () => {
    const inspector = await readFile('scripts/inspect-package.mjs', 'utf8');
    expect(inspector).toContain('packageMode: unpackedReleaseIdentity?.packageMode');
    expect(inspector).toContain(
      "throw new Error('Windows TQPKG2 package mode does not match unpacked release metadata')",
    );
    expect(inspector).not.toContain(
      'manifest.packageMode !== process.env.TALKING_QUILL_PACKAGE_MODE',
    );
  });
});
