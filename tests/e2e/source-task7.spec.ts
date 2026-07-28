import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { _electron as electron, expect, test, type ElectronApplication } from '@playwright/test';
import { rendererPages, resetProfile } from './helpers';

const electronModule: unknown = createRequire(resolve('package.json'))('electron');
if (typeof electronModule !== 'string') throw new Error('Electron executable is unavailable');
const electronExecutable = electronModule;

async function driver<Result = void>(
  application: ElectronApplication,
  method: string,
  args: readonly unknown[] = [],
): Promise<Result> {
  return application.evaluate(
    (_electron, input) => {
      const target: unknown = Reflect.get(
        globalThis,
        Symbol.for('talking-quill:task6-test-driver'),
      );
      if (typeof target !== 'object' || target === null) throw new Error('Test driver unavailable');
      const operation = (target as Record<string, unknown>)[input.method];
      if (typeof operation !== 'function') throw new Error('Test operation unavailable');
      return (operation as (...values: readonly unknown[]) => unknown)(...input.args);
    },
    { method, args },
  ) as Promise<Result>;
}

async function waitForPhase(application: ElectronApplication, phase: string) {
  await expect
    .poll(() =>
      driver<{ readonly session: { readonly phase: string } }>(application, 'snapshot').then(
        (value) => value.session.phase,
      ),
    )
    .toBe(phase);
}

async function quick(application: ElectronApplication, text: string, cancel = false) {
  await driver(application, 'setTranscript', [text]);
  await driver(application, 'activationComplete', [100]);
  await expect
    .poll(() =>
      driver<{ readonly recording: { readonly active: boolean } }>(application, 'snapshot').then(
        (value) => value.recording.active,
      ),
    )
    .toBe(true);
  await waitForPhase(application, 'recordingQuick');
  await driver(application, 'frames', [0.2, 15]);
  await driver(application, 'key', [cancel ? 'escape' : 'enter']);
  await waitForPhase(application, cancel ? 'cancelled' : 'completed');
  await waitForPhase(application, 'idle');
}

async function launch(profile: string, now?: number) {
  return electron.launch({
    executablePath: electronExecutable,
    args: [
      resolve('app'),
      `--talking-quill-user-data=${profile}`,
      '--talking-quill-task6-test',
      ...(now === undefined ? [] : [`--talking-quill-test-now=${String(now)}`]),
    ],
    env: { ...process.env, NODE_ENV: 'test' },
  });
}

test('Task 7 records completion, excludes cancellation, and supports local history CRUD', async () => {
  test.setTimeout(90_000);
  const profile = await resetProfile('task7-history');
  const application = await launch(profile);
  try {
    const { main } = await rendererPages(application);
    await quick(application, 'first retained dictation');
    await main.getByRole('button', { name: 'Dictation history' }).click();
    await expect(main.getByText('first retained dictation')).toBeVisible();
    await quick(application, 'cancelled private dictation', true);
    await expect(main.getByText('cancelled private dictation')).toHaveCount(0);
    await expect(
      main.getByRole('list', { name: 'Dictation history' }).getByRole('listitem'),
    ).toHaveCount(1);

    await main.getByRole('button', { name: 'Copy' }).click();
    await expect(main.getByText('Copied to your clipboard.')).toBeVisible();
    expect(await application.evaluate(({ clipboard }) => clipboard.readText())).toBe(
      'first retained dictation',
    );
    await main
      .getByRole('listitem')
      .filter({ hasText: 'first retained dictation' })
      .getByRole('button', { name: 'Delete' })
      .click();
    await expect(main.getByText('first retained dictation')).toHaveCount(0);
    await expect(main.getByText('Nothing here yet')).toBeVisible();

    await quick(application, 'second retained dictation');
    await expect(main.getByText('second retained dictation')).toBeVisible();
    await main.getByRole('button', { name: 'Delete all history' }).click();
    await expect(main.getByRole('dialog', { name: 'Delete all history?' })).toBeVisible();
    await main.getByRole('button', { name: 'Delete all', exact: true }).click();
    await expect(main.getByText('Nothing here yet')).toBeVisible();

    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Privacy & data' }).click();
    const historyEnabled = main.getByRole('checkbox', {
      name: 'Keep a history of what you dictated',
    });
    await historyEnabled.click();
    await expect(main.getByText('History preference saved.')).toBeVisible();
    await expect(historyEnabled).not.toBeChecked();
    await main.getByRole('button', { name: 'Dashboard' }).click();
    await quick(application, 'history disabled dictation');
    await main.getByRole('button', { name: 'Dictation history' }).click();
    await expect(main.getByText('history disabled dictation')).toHaveCount(0);
    await expect(main.getByText('Nothing here yet')).toBeVisible();
  } finally {
    await application.close();
  }
});

test('Task 7 prunes expired history at startup using the configured retention', async () => {
  test.setTimeout(90_000);
  const profile = await resetProfile('task7-retention');
  const day = 24 * 60 * 60 * 1_000;
  const oldNow = 100 * day;
  const currentNow = 110 * day;

  let application = await launch(profile, oldNow);
  try {
    const main = (await rendererPages(application)).main;
    await quick(application, 'expired dictation');
    await main.getByRole('button', { name: 'Dictation history' }).click();
    await expect(main.getByText('expired dictation')).toBeVisible();
  } finally {
    await application.close();
  }

  application = await launch(profile, currentNow);
  try {
    const main = (await rendererPages(application)).main;
    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Privacy & data' }).click();
    await main.getByRole('combobox', { name: 'How long to keep history' }).selectOption('7');
    await expect(main.getByText('Retention preference saved.')).toBeVisible();
    await main.getByRole('button', { name: 'Dashboard' }).click();
    await quick(application, 'retained dictation');
  } finally {
    await application.close();
  }

  application = await launch(profile, currentNow);
  try {
    const main = (await rendererPages(application)).main;
    await main.getByRole('button', { name: 'Dictation history' }).click();
    await expect(main.getByText('expired dictation')).toHaveCount(0);
    await expect(main.getByText('retained dictation')).toBeVisible();
  } finally {
    await application.close();
  }
});
