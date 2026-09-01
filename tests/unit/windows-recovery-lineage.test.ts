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
  it('supports public 0.0.67 without a record and schema 3 from 0.0.69 onward', async () => {
    await expect(declaredLineage().then(verifyWindowsRecoveryLineage)).resolves.toEqual({
      baselineVersion: '0.0.67',
      schemaVersion: 3,
      publishedArtifacts: 2,
    });
  });

  it.each([1, 2])('proves no declared published artifact uses local schema %s', async (schema) => {
    const lineage = await declaredLineage();
    lineage.publishedArtifacts[1] = { version: '0.0.69', relaunchRecordSchema: schema };
    expect(() => verifyWindowsRecoveryLineage(lineage)).toThrow(/unpublished/u);
  });

  it('rejects replacing the released schema 3 contract', async () => {
    const changed = await declaredLineage();
    changed.currentRelaunchRecordSchema = 4;
    changed.publishedArtifacts[1] = { version: '0.0.69', relaunchRecordSchema: 4 };
    expect(() => verifyWindowsRecoveryLineage(changed)).toThrow(/invalid/u);
  });

  it('rejects invented predecessor releases and schema changes after 0.0.69', async () => {
    const invented = await declaredLineage();
    invented.publishedArtifacts.splice(1, 0, {
      version: '0.0.68',
      relaunchRecordSchema: 2,
    });
    expect(() => verifyWindowsRecoveryLineage(invented)).toThrow(/unpublished/u);

    const changed = await declaredLineage();
    changed.publishedArtifacts.push({ version: '0.0.70', relaunchRecordSchema: 4 });
    expect(() => verifyWindowsRecoveryLineage(changed)).toThrow(/schema 3 lineage/u);
  });
});
