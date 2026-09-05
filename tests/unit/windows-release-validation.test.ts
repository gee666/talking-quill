import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { windowsReleaseValidation } from '../../scripts/windows-release-validation.mjs';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));

describe('automated release validation inventory', () => {
  it('binds both architectures without claiming manual acceptance and rejects altered evidence', async () => {
    const directory = await createTestDirectory('windows-release-validation');
    roots.push(directory);
    await expect(windowsReleaseValidation(directory)).rejects.toThrow();
    for (const arch of ['x64', 'arm64']) {
      for (const prefix of [
        'windows-installer-ui-smoke',
        'windows-installer-success-fresh',
        'windows-local-migration',
        'windows-terminal-fault-candidate',
      ]) {
        await writeFile(join(directory, `${prefix}-${arch}.json`), JSON.stringify({ arch }));
      }
    }
    const report = await windowsReleaseValidation(directory);
    expect(report.evidence).toHaveLength(8);
    expect(report.realReboot).toBe('not-collected');
    expect(report.protectedInstalledAcceptance).toBe('not-collected');
    await expect(windowsReleaseValidation(directory, true)).resolves.toEqual(report);
    await writeFile(join(directory, 'windows-installer-ui-smoke-arm64.json'), '{}');
    await expect(windowsReleaseValidation(directory, true)).rejects.toThrow(/inventory mismatch/u);
  });
});
