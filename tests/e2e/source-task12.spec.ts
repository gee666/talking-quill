import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import AxeBuilder from '@axe-core/playwright';
import {
  _electron as electron,
  expect,
  test,
  type ElectronApplication,
  type Page,
} from '@playwright/test';
import { rendererPages, resetFreshProfile } from './helpers';

const executable: unknown = createRequire(resolve('package.json'))('electron');
if (typeof executable !== 'string') throw new Error('Electron executable unavailable');
const electronExecutable = executable;

async function launch(profile: string) {
  return electron.launch({
    executablePath: electronExecutable,
    args: [resolve('app'), `--talking-quill-user-data=${profile}`, '--talking-quill-task6-test'],
    env: { ...process.env, NODE_ENV: 'test' },
  });
}
async function driver(
  application: ElectronApplication,
  method: string,
  args: readonly unknown[] = [],
): Promise<unknown> {
  const result: unknown = await application.evaluate(
    (_electron, input) => {
      const value: unknown = Reflect.get(globalThis, Symbol.for('talking-quill:task6-test-driver'));
      if (typeof value !== 'object' || value === null) throw new Error('Task 6 driver unavailable');
      const fn: unknown = Reflect.get(value, input.method);
      if (typeof fn !== 'function') throw new Error('Driver method unavailable');
      const invocation: unknown = Reflect.apply(fn, value, input.args);
      return invocation;
    },
    { method, args },
  );
  return result;
}
async function axe(page: Awaited<ReturnType<typeof rendererPages>>['main']) {
  const results = await new AxeBuilder({ page })
    .setLegacyMode()
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze();
  expect(results.violations).toEqual([]);
}
async function assertCompactTracker(page: Page, currentStep: number) {
  const items = page.locator('.welcome__progress li');
  await expect(items).toHaveCount(6);
  await expect(items.nth(currentStep - 1)).toHaveAttribute('data-state', 'current');
  const geometry = await items.evaluateAll((elements) =>
    elements.map((element) => {
      const box = element.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, height: box.height };
    }),
  );
  expect(
    Math.max(...geometry.map(({ top }) => top)) - Math.min(...geometry.map(({ top }) => top)),
  ).toBeLessThan(2);
  expect(Math.max(...geometry.map(({ height }) => height))).toBeLessThanOrEqual(56);
  for (let index = 1; index < geometry.length; index += 1) {
    expect(geometry[index]?.left).toBeGreaterThanOrEqual(geometry[index - 1]?.right ?? 0);
  }
}

test('Welcome resumes, completes Raw-only setup, reopens, and reaches first dictation', async () => {
  test.setTimeout(90_000);
  const profile = await resetFreshProfile('task12-welcome');
  let application = await launch(profile);
  try {
    let main = (await rendererPages(application)).main;
    await expect(main.getByRole('heading', { name: 'Welcome' })).toBeFocused();
    await axe(main);
    await main.getByRole('button', { name: 'Continue' }).click();
    await expect(main.getByRole('heading', { name: 'Microphone' })).toBeFocused();
    await main.getByRole('button', { name: 'Continue' }).click();
    await expect(main.getByRole('alert')).toBeVisible();
    await expect(main.getByRole('heading', { name: 'Microphone' })).toBeVisible();
    const exit = application.waitForEvent('close');
    await application.evaluate(({ app }) => app.quit());
    await exit;

    application = await launch(profile);
    main = (await rendererPages(application)).main;
    await expect(main.getByRole('heading', { name: 'Microphone' })).toBeFocused();
    await driver(application, 'setWelcomePrerequisites', [true]);
    await main.getByRole('button', { name: 'Continue' }).click();
    await expect(main.getByRole('heading', { name: 'Local model' })).toBeFocused();
    await main.getByRole('button', { name: 'Continue' }).click();
    await expect(main.getByRole('heading', { name: 'Shortcut' })).toBeFocused();
    await main.getByRole('button', { name: 'Test activation shortcut' }).click();
    await driver(application, 'activationDown');
    await driver(application, 'activationUp');
    await expect(main.getByText(/gesture recognized/)).toBeVisible();
    await main.getByRole('button', { name: 'Continue' }).click();
    await expect(
      main.getByRole('heading', { name: 'Smart processing', exact: true }).first(),
    ).toBeFocused();
    await main.getByRole('button', { name: 'Skip Smart processing' }).click();
    await expect(main.getByRole('heading', { name: 'Ready', exact: true })).toBeFocused();
    await main.getByRole('button', { name: 'Start using Talking Quill' }).click();
    await expect(main.getByRole('heading', { name: 'Talking Quill is ready' })).toBeVisible();

    // Completion is a durable UI latch: temporary readiness loss and a real restart do not rewind.
    await driver(application, 'setWelcomePrerequisites', [false]);
    const completedExit = application.waitForEvent('close');
    await application.evaluate(({ app }) => app.quit());
    await completedExit;
    application = await launch(profile);
    main = (await rendererPages(application)).main;
    await expect(main.getByRole('heading', { name: 'Talking Quill is ready' })).toBeVisible();
    await expect(main.getByRole('heading', { name: 'Welcome' })).toHaveCount(0);
    await driver(application, 'setWelcomePrerequisites', [true]);

    await driver(application, 'activationDown');
    await driver(application, 'frames', [0.2, 15]);
    await driver(application, 'activationUp');
    await driver(application, 'key', ['enter']);
    await expect
      .poll(() =>
        driver(application, 'snapshot').then(
          (value: unknown) => (value as { session: { phase: string } }).session.phase,
        ),
      )
      .toBe('completed');
    await expect(main.getByText('deterministic transcript')).toBeVisible();

    await main.getByRole('button', { name: 'Info' }).click();
    await main.screenshot({ path: 'tmp/review-screenshots/info-large.png', fullPage: false });
    await main.getByRole('button', { name: 'Reopen Welcome' }).click();
    await expect(main.getByRole('button', { name: 'Exit Welcome' })).toBeVisible();
    await main.getByRole('button', { name: 'Exit Welcome' }).click();
    await expect(main.getByRole('heading', { name: 'About Talking Quill' })).toBeVisible();
  } finally {
    await application.evaluate(({ app }) => app.quit()).catch(() => undefined);
  }
});

test('Task 12 screens remain accessible without overflow at 960 by 600', async () => {
  const profile = await resetFreshProfile('task12-compact');
  const application = await launch(profile);
  try {
    const main = (await rendererPages(application)).main;
    await application.evaluate(({ BrowserWindow }) =>
      BrowserWindow.getAllWindows()
        .find((window) => window.getTitle() === 'Talking Quill')
        ?.setSize(960, 600),
    );
    await axe(main);
    await assertCompactTracker(main, 1);
    await main.screenshot({
      path: 'tmp/review-screenshots/welcome-step-1-960.png',
      fullPage: false,
    });
    await driver(application, 'setWelcomePrerequisites', [true]);
    expect(
      await main.evaluate(() => ({
        width: document.documentElement.scrollWidth,
        viewport: document.documentElement.clientWidth,
      })),
    ).toEqual({ width: 960, viewport: 960 });
    for (let step = 1; step < 6; step += 1) {
      if (step === 4) {
        await main.getByRole('button', { name: 'Test activation shortcut' }).click();
        await driver(application, 'activationDown');
        await driver(application, 'activationUp');
      }
      const button = main.getByRole('button', {
        name: step === 5 ? 'Skip Smart processing' : 'Continue',
      });
      await button.click();
      await axe(main);
      await assertCompactTracker(main, step + 1);
      await main.screenshot({
        path: `tmp/review-screenshots/welcome-step-${String(step + 1)}-960.png`,
        fullPage: false,
      });
      if (step === 4) {
        const scrollRegion = main.getByRole('region', { name: 'Smart processing setup controls' });
        await scrollRegion.focus();
        await scrollRegion.press('End');
        await expect.poll(() => scrollRegion.evaluate((node) => node.scrollTop)).toBeGreaterThan(0);
        const lastControl = scrollRegion.getByRole('button').last();
        await expect(lastControl).toBeInViewport();
        await main.screenshot({
          path: 'tmp/review-screenshots/smart-scrolled-960.png',
          fullPage: false,
        });
      }
    }
  } finally {
    await application.evaluate(({ app }) => app.quit()).catch(() => undefined);
  }
});
