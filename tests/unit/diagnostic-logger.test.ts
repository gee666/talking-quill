import { readdir, readFile, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { DiagnosticLogger } from '../../app/src/main/security/diagnostic-logger';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const owned: string[] = [];

afterEach(async () => {
  await Promise.all(owned.splice(0).map((path) => removeTestDirectory(path)));
});

async function setup(enabled: boolean, options: { readonly maxBytes?: number } = {}) {
  const root = await createTestDirectory('diagnostic-logger');
  owned.push(root);
  const settings = new SettingsStore(join(root, 'settings.json'));
  await settings.initialize();
  if (enabled) await settings.update({ privacy: { diagnosticLoggingEnabled: true } });
  let now = 1;
  const logs = join(root, 'logs');
  const logger = new DiagnosticLogger(settings, logs, {
    ...options,
    now: () => now++,
  });
  await logger.initialize();
  return { root, logs, settings, logger };
}

describe('DiagnosticLogger', () => {
  it('writes nothing while disabled and follows the opt-in setting immediately', async () => {
    const { logs, settings, logger } = await setup(false);
    await logger.record('application.started', { component: 'application', outcome: 'ready' });
    await expect(readdir(logs)).rejects.toMatchObject({ code: 'ENOENT' });

    await settings.update({ privacy: { diagnosticLoggingEnabled: true } });
    await logger.record('application.started', { component: 'application', outcome: 'ready' });
    const source = await readFile(join(logs, 'diagnostic.jsonl'), 'utf8');
    expect(source).toContain('application.started');
    expect(source).not.toMatch(/transcript|authorization|request|response|body|audio|screenshot/i);

    await settings.update({ privacy: { diagnosticLoggingEnabled: false } });
    const before = await stat(join(logs, 'diagnostic.jsonl'));
    await logger.record('application.stopping', {
      component: 'application',
      outcome: 'requested',
    });
    const after = await stat(join(logs, 'diagnostic.jsonl'));
    expect(after.size).toBe(before.size);
    await logger.dispose();
  });

  it('rejects non-allowlisted metadata instead of attempting to redact user content', async () => {
    const { logs, logger } = await setup(true);
    expect(() =>
      logger.record('application.started', {
        component: 'application',
        transcript: 'private words',
      } as never),
    ).toThrow();
    await expect(readdir(logs)).resolves.toEqual([]);
    await logger.dispose();
  });

  it('rotates bounded files and keeps restrictive file permissions where supported', async () => {
    const { logs, logger } = await setup(true, { maxBytes: 1_024 });
    for (let index = 0; index < 40; index += 1) {
      await logger.record('application.started', {
        component: 'application',
        outcome: 'ready',
        code: `READY_${String(index)}`,
      });
    }
    await logger.dispose();
    const files = (await readdir(logs)).sort();
    expect(files).toEqual([
      'diagnostic.jsonl',
      'diagnostic.jsonl.1',
      'diagnostic.jsonl.2',
      'diagnostic.jsonl.3',
    ]);
    for (const file of files) {
      const metadata = await stat(join(logs, file));
      expect(metadata.size).toBeLessThanOrEqual(1_024);
      if (process.platform !== 'win32') expect(metadata.mode & 0o777).toBe(0o600);
    }
  });
});
