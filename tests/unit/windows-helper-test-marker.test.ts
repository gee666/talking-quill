import { mkdir, rm, writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';

import {
  GATEWAY_CANNOT_SUPPRESS_MARKER,
  LEGACY_NATIVE_MARKERS,
  OWNER_SAFE_DISABLED_MARKER,
  OWNER_TEST_SEAMS_MARKER,
  RETIRED_WINDOWS_UPDATE_BRIDGE_KEY_MARKER,
  WINDOWS_TEST_PHYSICAL_MARKER,
  WINDOWS_UPDATE_PRIMARY_KEY_MARKER,
  verifyHelperBuildContract,
} from '../../scripts/helper-build-contract.mjs';

const root = resolve(import.meta.dirname, '../..');
const fixtureRoot = resolve(root, 'tmp', 'helper-build-contract-test');
const WINDOWS_UPDATE_KEY = Buffer.from(`04${'11'.repeat(64)}`, 'ascii');

function source(path: string): string {
  return readFileSync(resolve(root, path), 'utf8');
}

async function fixture(name: string, chunks: (Buffer | string)[]): Promise<string> {
  await mkdir(fixtureRoot, { recursive: true });
  const path = resolve(fixtureRoot, name);
  await writeFile(path, Buffer.concat(chunks.map((chunk) => Buffer.from(chunk))));
  return path;
}

afterEach(async () => {
  await rm(fixtureRoot, { recursive: true, force: true });
});

describe('structural gateway build-marker boundary', () => {
  it('keeps native test features owner-scoped and rejects optimized core seams', () => {
    const rootCargo = source('helper/Cargo.toml');
    const coreCargo = source('helper/keyboard-core/Cargo.toml');
    const coreBuild = source('helper/keyboard-core/build.rs');
    const ownerCargo = source('helper/keyboard-owner/Cargo.toml');
    const injection = source('helper/keyboard-owner/src/platform/windows/injection.rs');
    expect(rootCargo).not.toContain('windows-native-test-input = []');
    expect(coreCargo).toContain('native-test-input = []');
    expect(ownerCargo).toContain(
      'windows-native-test-input = ["talking-quill-keyboard-core/native-test-input"]',
    );
    expect(coreBuild).toContain(
      'native-test-input cannot be compiled into an optimized keyboard core',
    );
    expect(injection).toContain(
      '#[cfg(all(feature = "windows-native-test-input", not(debug_assertions)))]',
    );
    expect(injection).toContain(
      'compile_error!("windows-native-test-input cannot be included in a release helper")',
    );
  });

  it('accepts only the permanent non-suppressing gateway marker', async () => {
    const path = await fixture('gateway.exe', [
      Buffer.from([0, 1]),
      GATEWAY_CANNOT_SUPPRESS_MARKER,
      WINDOWS_UPDATE_PRIMARY_KEY_MARKER,
      WINDOWS_UPDATE_KEY,
    ]);
    await expect(verifyHelperBuildContract(path, { windows: true })).resolves.toBeUndefined();
  });

  it('rejects missing, owner, legacy native, and test-input markers', async () => {
    const missing = await fixture('missing.exe', ['ordinary executable']);
    await expect(verifyHelperBuildContract(missing, { windows: true })).rejects.toThrow(
      'gateway-cannot-suppress marker',
    );

    for (const [index, marker] of [
      OWNER_SAFE_DISABLED_MARKER,
      OWNER_TEST_SEAMS_MARKER,
      ...LEGACY_NATIVE_MARKERS,
    ].entries()) {
      const path = await fixture(`forbidden-${String(index)}.exe`, [
        GATEWAY_CANNOT_SUPPRESS_MARKER,
        marker,
      ]);
      await expect(verifyHelperBuildContract(path, { windows: false })).rejects.toThrow(
        'forbidden or mixed role/build marker',
      );
    }

    const testMarker = Buffer.alloc(8);
    testMarker.writeBigUInt64LE(WINDOWS_TEST_PHYSICAL_MARKER);
    const testBuild = await fixture('test-build.exe', [
      GATEWAY_CANNOT_SUPPRESS_MARKER,
      WINDOWS_UPDATE_PRIMARY_KEY_MARKER,
      WINDOWS_UPDATE_KEY,
      testMarker,
    ]);
    await expect(verifyHelperBuildContract(testBuild, { windows: true })).rejects.toThrow(
      'test-only physical input marker',
    );

    const retiredBridge = await fixture('retired-bridge.exe', [
      GATEWAY_CANNOT_SUPPRESS_MARKER,
      WINDOWS_UPDATE_PRIMARY_KEY_MARKER,
      WINDOWS_UPDATE_KEY,
      RETIRED_WINDOWS_UPDATE_BRIDGE_KEY_MARKER,
      WINDOWS_UPDATE_KEY,
    ]);
    await expect(verifyHelperBuildContract(retiredBridge, { windows: true })).rejects.toThrow(
      'retired updater bridge key',
    );
  });

  it('uses the shared positive role verifiers for staging and package inspection', () => {
    expect(source('scripts/build-helper.mjs')).toContain('verifyStagedNativeRoleSet');
    const inspector = source('scripts/inspect-package.mjs');
    expect(inspector).toContain('verifyHelperBuildContract');
    expect(inspector).toContain('verifyCompleteNativeRoleInventory');
  });
});
