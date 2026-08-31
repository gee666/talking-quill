import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const setup = resolve('installer/windows-setup/src/windows.rs');

describe('native Windows install transaction contract', () => {
  it('keeps write-ahead recovery in the elevated worker', async () => {
    const source = (await readFile(setup, 'utf8')).replace(/\s+/gu, ' ');
    const stagingRecord = source.indexOf('write_transaction(paths, "staging"');
    const extraction = source.indexOf('package::extract_file', stagingRecord);
    const preparedRecord = source.indexOf('write_transaction(paths, "prepared"', extraction);
    const predecessorRename = source.indexOf(
      'durable_rename(&paths.install, &paths.backup)',
      preparedRecord,
    );
    const publish = source.indexOf(
      'durable_rename(&paths.staging, &paths.install)',
      predecessorRename,
    );
    const commit = source.indexOf('write_transaction(paths, "committed"', publish);
    expect(stagingRecord).toBeGreaterThanOrEqual(0);
    expect(extraction).toBeGreaterThan(stagingRecord);
    expect(preparedRecord).toBeGreaterThan(extraction);
    expect(predecessorRename).toBeGreaterThan(preparedRecord);
    expect(publish).toBeGreaterThan(predecessorRename);
    expect(commit).toBeGreaterThan(publish);
  });

  it('recovers each durable phase without an interpreter', async () => {
    const source = await readFile(setup, 'utf8');
    for (const phase of ['staging', 'prepared', 'committed', 'uninstalling']) {
      expect(source).toContain(`"${phase}"`);
    }
    expect(source).toContain('FILE_ATTRIBUTE_REPARSE_POINT');
    expect(source).not.toMatch(/powershell|cmd\.exe|wscript|cscript/iu);
  });
});
