import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import {
  type WindowsRecoveryLineage,
  verifyWindowsRecoveryLineage,
} from '../../scripts/windows-recovery-lineage.mjs';

async function declaredLineage(): Promise<WindowsRecoveryLineage> {
  const config = JSON.parse(await readFile('release.config.json', 'utf8')) as {
    windowsRecoveryLineage: WindowsRecoveryLineage;
  };
  return structuredClone(config.windowsRecoveryLineage);
}

describe('Windows recovery release lineage', () => {
  it('labels 0.0.67 local/non-public and starts public schema 3 at fresh root 0.0.69', async () => {
    await expect(declaredLineage().then(verifyWindowsRecoveryLineage)).resolves.toEqual({
      trustRootVersion: '0.0.69',
      localMigrationSourceVersion: '0.0.67',
      schemaVersion: 3,
      publishedArtifacts: 1,
    });
  });

  it.each([1, 2])('proves no declared public artifact uses local schema %s', async (schema) => {
    const lineage = await declaredLineage();
    lineage.publishedArtifacts[0] = {
      version: '0.0.69',
      predecessorVersion: null,
      relaunchRecordSchema: schema,
    };
    expect(() => verifyWindowsRecoveryLineage(lineage)).toThrow(/invalid/u);
  });

  it('rejects any claim that local 0.0.67 was public or an update predecessor', async () => {
    const publicClaim = await declaredLineage();
    publicClaim.publishedArtifacts.unshift({
      version: '0.0.67',
      predecessorVersion: null,
      relaunchRecordSchema: 3,
    });
    expect(() => verifyWindowsRecoveryLineage(publicClaim)).toThrow(
      /never be claimed as a public/u,
    );

    const predecessorClaim = await declaredLineage();
    const root = predecessorClaim.publishedArtifacts[0];
    if (root === undefined) throw new Error('Missing trust root fixture');
    root.predecessorVersion = '0.0.67';
    expect(() => verifyWindowsRecoveryLineage(predecessorClaim)).toThrow(
      /fresh public trust-lineage root/u,
    );
  });

  it('requires future updates to start from exact public 0.0.69', async () => {
    const valid = await declaredLineage();
    valid.publishedArtifacts.push({
      version: '0.0.70',
      predecessorVersion: '0.0.69',
      relaunchRecordSchema: 3,
    });
    expect(verifyWindowsRecoveryLineage(valid).publishedArtifacts).toBe(2);

    const update = valid.publishedArtifacts[1];
    if (update === undefined) throw new Error('Missing update fixture');
    update.predecessorVersion = '0.0.67';
    expect(() => verifyWindowsRecoveryLineage(valid)).toThrow(/prior public release/u);
  });

  it('requires the one exact local uninstall-preserve-fresh migration', async () => {
    const changed = await declaredLineage();
    const migration = changed.localMigrations[0];
    if (migration === undefined) throw new Error('Missing migration fixture');
    migration.provenance = 'public';
    expect(() => verifyWindowsRecoveryLineage(changed)).toThrow(/local migration policy/u);
  });
});
