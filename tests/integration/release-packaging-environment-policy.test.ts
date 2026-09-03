import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import {
  createFreshEnvironment,
  PERSONAL_TARGETS,
  sanitizePersonalConsumerEnvironment,
} from '../../scripts/personal-use.mjs';
import { createPackagePlan, createProductionEnvironment } from '../../scripts/run-package.mjs';

const poisonedProducerEnvironment = {
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
};

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
    ]) {
      expect(environment).not.toHaveProperty(name);
    }
  });

  it('removes producer-only mode, predecessor, and marker values from personal consumers', () => {
    const environment = sanitizePersonalConsumerEnvironment(poisonedProducerEnvironment);
    expect(environment).toEqual({ PATH: '/reviewed/path' });
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
    },
  );

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
