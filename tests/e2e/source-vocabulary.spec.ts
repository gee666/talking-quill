import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { _electron as electron, expect, test } from '@playwright/test';
import { rendererPages, resetProfile } from './helpers';

const electronModule: unknown = createRequire(resolve('package.json'))('electron');
if (typeof electronModule !== 'string') throw new Error('Electron executable is unavailable');

const EXPORTED_FILENAME = 'vocabulary-ui-roundtrip.txt';
const VALUES = ['GraphQL', 'José', 'co-op'] as const;

test('Settings UI exports and imports vocabulary through typed IPC and dialog file handling', async () => {
  const profile = await resetProfile('vocabulary-ui-roundtrip');
  const application = await electron.launch({
    executablePath: electronModule,
    args: [
      resolve('app'),
      `--talking-quill-user-data=${profile}`,
      '--talking-quill-vocabulary-test',
    ],
    env: { ...process.env, NODE_ENV: 'test' },
  });
  try {
    const { main } = await rendererPages(application);
    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Custom Vocabulary' }).click();
    const vocabulary = main.getByRole('list', { name: 'Custom vocabulary' });
    const input = main.getByLabel(/Word or phrase/);

    for (const value of VALUES) {
      await input.fill(value);
      await main.getByRole('button', { name: 'Add to vocabulary' }).click();
      await expect(vocabulary.getByText(value, { exact: true })).toBeVisible();
    }

    await main.getByRole('button', { name: 'Export plain text' }).click();
    await expect(
      main.getByText(`Exported ${String(VALUES.length)} vocabulary entries.`, { exact: true }),
    ).toBeVisible();
    expect(await readFile(resolve(profile, EXPORTED_FILENAME), 'utf8')).toBe(
      `${VALUES.join('\n')}\n`,
    );

    for (const value of VALUES) {
      await main.getByRole('button', { name: `Delete ${value}` }).click();
      await expect(main.getByText(value, { exact: true })).toHaveCount(0);
    }
    await expect(main.getByText('Custom vocabulary is empty')).toBeVisible();

    await main.getByRole('button', { name: 'Import plain text' }).click();
    await expect(
      main.getByText(`Imported ${String(VALUES.length)} vocabulary entries.`, { exact: true }),
    ).toBeVisible();
    const imported = main.getByRole('list', { name: 'Custom vocabulary' });
    for (const value of VALUES) {
      await expect(imported.getByText(value, { exact: true })).toBeVisible();
    }
  } finally {
    await application.close();
  }
});
