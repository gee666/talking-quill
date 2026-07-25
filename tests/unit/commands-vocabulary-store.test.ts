import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { VoiceCommandStore } from '../../app/src/main/commands/voice-command-store';
import { VocabularyStore } from '../../app/src/main/vocabulary/vocabulary-store';
import { SETTINGS_MIGRATIONS } from '../../app/src/main/persistence/settings-migrations';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const directories: string[] = [];
afterEach(async () => Promise.all(directories.splice(0).map(removeTestDirectory)));

async function stores(name: string) {
  const directory = await createTestDirectory(name);
  directories.push(directory);
  const path = join(directory, 'settings.json');
  const settings = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
  await settings.initialize();
  return {
    path,
    settings,
    commands: new VoiceCommandStore(settings),
    vocabulary: new VocabularyStore(settings),
  };
}

describe('commands and vocabulary persistence', () => {
  it('persists command CRUD, normalizes snippets, and rejects near duplicates', async () => {
    const { settings, commands } = await stores('commands');
    const created = await commands.create({
      trigger: 'Launch dashboard',
      snippet: 'line 1\r\nline 2',
    });
    expect(created.snippet).toBe('line 1\nline 2');
    await expect(
      commands.create({ trigger: 'launch dashboart', snippet: 'other' }),
    ).rejects.toThrow('too similar');
    const updated = await commands.update(created.id, { snippet: 'new text' });
    expect(commands.match('LAUNCH DASHBOARD!')?.command.snippet).toBe('new text');
    await settings.flush();
    expect(settings.get().voiceCommands).toEqual([updated]);
    expect(await commands.delete(created.id)).toBe(true);
    expect(await commands.delete(created.id)).toBe(false);
  });

  it.each(['...', '— --', '😀'])('rejects nonmeaningful normalized trigger %s', async (trigger) => {
    const { commands } = await stores('commands-empty-normalized');
    await expect(commands.create({ trigger, snippet: 'text' })).rejects.toThrow(
      'letter or number after normalization',
    );
    expect(commands.list()).toEqual([]);
  });

  it('rejects multibyte command byte overflow and aggregate overflow without writes', async () => {
    const { commands } = await stores('commands-byte-limits');
    await expect(
      commands.create({ trigger: 'oversized snippet', snippet: '界'.repeat(70_000) }),
    ).rejects.toThrow('UTF-8');
    expect(commands.list()).toEqual([]);

    const snippet = '界'.repeat(60_000);
    await commands.create({ trigger: 'first large snippet', snippet });
    await commands.create({ trigger: 'second large snippet', snippet });
    await expect(commands.create({ trigger: 'third large snippet', snippet })).rejects.toThrow(
      'total UTF-8 size limit',
    );
    expect(commands.list()).toHaveLength(2);
  });

  it('serializes concurrent conflict checks', async () => {
    const { commands } = await stores('commands-concurrent');
    const results = await Promise.allSettled([
      commands.create({ trigger: 'send weekly report', snippet: 'one' }),
      commands.create({ trigger: 'send weekly reports', snippet: 'two' }),
    ]);
    expect(results.filter((result) => result.status === 'fulfilled')).toHaveLength(1);
    expect(commands.list()).toHaveLength(1);
  });

  it('rejects multibyte vocabulary byte and aggregate overflow atomically', async () => {
    const { vocabulary } = await stores('vocabulary-byte-limits');
    await expect(vocabulary.create('界'.repeat(150))).rejects.toThrow('UTF-8');
    expect(vocabulary.list()).toEqual([]);

    const oversizedAggregate = Array.from(
      { length: 700 },
      (_, index) => `${'界'.repeat(129)}${String(index)}`,
    );
    await expect(vocabulary.import(oversizedAggregate)).rejects.toThrow('total UTF-8 size limit');
    expect(vocabulary.list()).toEqual([]);
  });

  it('persists vocabulary CRUD and imports atomically', async () => {
    const { path, settings, vocabulary } = await stores('vocabulary');
    const first = await vocabulary.create('GraphQL');
    await expect(vocabulary.create('graphql')).rejects.toThrow('already');
    const imported = await vocabulary.import(['José', 'AnythingLLM']);
    expect(imported).toHaveLength(2);
    await expect(vocabulary.import(['valid', 'graphql'])).rejects.toThrow('already');
    expect(vocabulary.list().map((entry) => entry.value)).toEqual([
      'GraphQL',
      'José',
      'AnythingLLM',
    ]);
    await vocabulary.update(first.id, 'GraphQL API');
    await settings.flush();
    const reopened = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
    await reopened.initialize();
    expect(reopened.get().customVocabulary.map((entry) => entry.value)).toEqual([
      'GraphQL API',
      'José',
      'AnythingLLM',
    ]);
    expect(await vocabulary.delete(first.id)).toBe(true);
  });
});
