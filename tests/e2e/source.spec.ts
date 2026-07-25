import { spawn } from 'node:child_process';
import { readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
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
import { rendererIsolation, rendererPages, resetProfile } from './helpers';

const electronModule: unknown = createRequire(resolve('package.json'))('electron');
if (typeof electronModule !== 'string') throw new Error('Electron executable is unavailable');
const electronExecutable = electronModule;

async function launch(profile: string, egressProof = false, environment: NodeJS.ProcessEnv = {}) {
  return electron.launch({
    args: [resolve('app'), `--talking-quill-user-data=${profile}`],
    env: {
      ...process.env,
      NODE_ENV: 'test',
      ...(egressProof ? { TALKING_QUILL_EGRESS_PROOF: '1' } : {}),
      ...environment,
    },
  });
}

async function readEgressCategories(profile: string): Promise<string[]> {
  const source = await readFile(resolve(profile, 'tmp', 'egress-proof.jsonl'), 'utf8');
  return source
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => (JSON.parse(line) as { readonly category: string }).category);
}

async function toggleCloseToTrayAndCloseImmediately(page: Page) {
  await page.evaluate(() => {
    const label = [...document.querySelectorAll('label')].find(
      (candidate) => candidate.textContent.trim() === 'Close to tray',
    );
    const toggle = label?.htmlFor === '' ? null : document.getElementById(label?.htmlFor ?? '');
    const close = document.querySelector<HTMLButtonElement>('button[aria-label="Close window"]');
    if (!(toggle instanceof HTMLInputElement) || close === null) {
      throw new Error('Close controls are unavailable');
    }
    toggle.click();
    close.click();
  });
}

async function closeWithDiagnostics(
  application: ElectronApplication,
  label: string,
  action: () => Promise<unknown>,
): Promise<void> {
  const closed = application.waitForEvent('close');
  await action();
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      closed,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new Error(`${label} did not close within 10 seconds`)),
          10_000,
        );
      }),
    ]);
  } catch (error: unknown) {
    const diagnostics = await application
      .evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows().map((window) => ({
          title: window.getTitle(),
          visible: window.isVisible(),
          destroyed: window.isDestroyed(),
          webContentsDestroyed: window.webContents.isDestroyed(),
        })),
      )
      .catch(() => 'application transport unavailable');
    await application.close().catch(() => undefined);
    throw new Error(`${String(error)}; window diagnostics: ${JSON.stringify(diagnostics)}`);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

async function expectAccessible(page: Page, name: string) {
  const results = await new AxeBuilder({ page })
    .setLegacyMode()
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    .analyze();
  expect(
    results.violations,
    `${name} accessibility violations:\n${JSON.stringify(results.violations, null, 2)}`,
  ).toEqual([]);
}

test('secure window roles, navigation, close lifecycle, and persistence', async () => {
  const profile = await resetProfile('source-profile');
  let application = await launch(profile);
  const { main, widget, capture } = await rendererPages(application);

  await expect(main.getByRole('heading', { name: 'Dictation needs local setup' })).toBeVisible();
  await expect(widget.getByText('Ready', { exact: true })).toBeAttached();
  await expectAccessible(main, 'Dashboard screen');
  await expectAccessible(widget, 'Widget shell');
  await expect
    .poll(() => capture.evaluate(() => document.documentElement.dataset.ready))
    .toBe('true');

  expect(
    await application.evaluate(({ BrowserWindow }) =>
      BrowserWindow.getAllWindows()
        .find((window) => window.getTitle() === 'Talking Quill Widget')
        ?.isFocusable(),
    ),
  ).toBe(false);

  expect(
    await application.evaluate(async ({ BrowserWindow }) => {
      const windows = BrowserWindow.getAllWindows();
      for (const window of windows) window.webContents.openDevTools({ mode: 'detach' });
      await new Promise((resolveWait) => setTimeout(resolveWait, 100));
      return windows.every((window) => !window.webContents.isDevToolsOpened());
    }),
  ).toBe(true);

  const isolatedRenderer = {
    requireType: 'undefined',
    processType: 'undefined',
    bufferType: 'undefined',
    moduleType: 'undefined',
    localStorage: {},
  };
  expect(await rendererIsolation(main)).toEqual(isolatedRenderer);
  expect(await rendererIsolation(widget)).toEqual(isolatedRenderer);
  expect(await rendererIsolation(capture)).toEqual(isolatedRenderer);
  expect(
    await main.evaluate(() => ({
      keys: Object.keys(window.talkingQuill),
      frozen: Object.isFrozen(window.talkingQuill),
      nestedFrozen: Object.values(window.talkingQuill).every(
        (value) => typeof value !== 'object' || value === null || Object.isFrozen(value),
      ),
      providerKeys: Object.keys(window.talkingQuill.providers),
      genericInvoke: Reflect.has(window.talkingQuill, 'invoke'),
      arbitraryChannel: Reflect.has(window.talkingQuill, 'ipc:unknown'),
      rawIpc: typeof Reflect.get(globalThis, 'ipcRenderer'),
      widgetNamespace: Reflect.has(window, 'talkingQuillWidget'),
      captureNamespace: Reflect.has(window, 'talkingQuillCapture'),
    })),
  ).toEqual({
    keys: [
      'welcome',
      'info',
      'activationTest',
      'app',
      'settings',
      'profiles',
      'data',
      'recording',
      'echo',
      'history',
      'commands',
      'vocabulary',
      'providers',
      'models',
      'windowControls',
    ],
    frozen: true,
    nestedFrozen: true,
    providerKeys: [
      'catalog',
      'piInstallationStatus',
      'savePiInstallation',
      'browsePiInstallation',
      'saveConfig',
      'setSecret',
      'secretStatus',
      'deleteSecret',
      'listModels',
      'testConnection',
      'destination',
      'cancel',
      'osaStatus',
      'setOnScreenAwareness',
      'verifyVision',
      'confirmVision',
    ],
    genericInvoke: false,
    arbitraryChannel: false,
    rawIpc: 'undefined',
    widgetNamespace: false,
    captureNamespace: false,
  });
  expect(
    await application.evaluate(() =>
      Reflect.has(globalThis, Symbol.for('talking-quill:task6-test-driver')),
    ),
  ).toBe(false);
  expect(
    await widget.evaluate(() => ({
      main: Reflect.has(window, 'talkingQuill'),
      capture: Reflect.has(window, 'talkingQuillCapture'),
      keys: Object.keys(window.talkingQuillWidget),
      frozen: Object.isFrozen(window.talkingQuillWidget),
      arbitraryChannel: Reflect.has(window.talkingQuillWidget, 'ipc:unknown'),
      rawIpc: typeof Reflect.get(globalThis, 'ipcRenderer'),
    })),
  ).toEqual({
    main: false,
    capture: false,
    keys: ['ready', 'stop', 'cancel', 'setInteractive', 'onSessionChanged'],
    frozen: true,
    arbitraryChannel: false,
    rawIpc: 'undefined',
  });
  expect(
    await capture.evaluate(() => ({
      main: Reflect.has(window, 'talkingQuill'),
      widget: Reflect.has(window, 'talkingQuillWidget'),
      keys: Object.keys(window.talkingQuillCapture),
      frozen: Object.isFrozen(window.talkingQuillCapture),
      arbitraryChannel: Reflect.has(window.talkingQuillCapture, 'ipc:unknown'),
      rawIpc: typeof Reflect.get(globalThis, 'ipcRenderer'),
    })),
  ).toEqual({
    main: false,
    widget: false,
    keys: ['ready'],
    frozen: true,
    arbitraryChannel: false,
    rawIpc: 'undefined',
  });

  expect(await main.evaluate(() => window.open('https://example.com'))).toBeNull();
  await expect(
    main.evaluate(() =>
      fetch('https://example.com')
        .then(() => 'allowed')
        .catch(() => 'blocked'),
    ),
  ).resolves.toBe('blocked');
  const csp = await main
    .locator('meta[http-equiv="Content-Security-Policy"]')
    .getAttribute('content');
  expect(csp).toContain("default-src 'none'");

  await expect(main.getByRole('heading', { name: 'Dictation needs local setup' })).toBeFocused();
  await main.keyboard.press('Shift+Tab');
  const info = main.getByRole('button', { name: 'Info' });
  await expect(info).toBeFocused();
  const focusAppearance = await info.evaluate((element) => {
    const style = getComputedStyle(element);
    return { boxShadow: style.boxShadow, outlineWidth: style.outlineWidth };
  });
  expect(focusAppearance.boxShadow === 'none' && focusAppearance.outlineWidth === '0px').toBe(
    false,
  );
  await main.keyboard.press('Shift+Tab');
  await expect(main.getByRole('button', { name: 'Settings' })).toBeFocused();
  await main.keyboard.press('Enter');
  await expect(main.getByRole('heading', { name: 'General settings' })).toBeFocused();
  await expectAccessible(main, 'Settings screen');
  await main.keyboard.press('Shift+Tab');
  await expect(main.getByRole('button', { name: 'Info' })).toBeFocused();
  await main.keyboard.press('Enter');
  await expect(main.getByRole('heading', { name: 'About Talking Quill' })).toBeFocused();
  await expectAccessible(main, 'Info screen');
  await main.emulateMedia({ reducedMotion: 'reduce' });
  const motionDurations = await main
    .locator('.nav-item')
    .first()
    .evaluate((element) => {
      const style = getComputedStyle(element);
      return [style.transitionDuration, style.animationDuration];
    });
  expect(
    motionDurations.every((durations) =>
      durations.split(',').every((duration) => parseFloat(duration) <= 0.000_01),
    ),
  ).toBe(true);

  await main.getByRole('button', { name: 'Close window' }).click();
  await expect
    .poll(() =>
      application.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()
          .find((window) => window.getTitle() === 'Talking Quill')
          ?.isVisible(),
      ),
    )
    .toBe(false);
  const secondInstance = spawn(
    electronExecutable,
    [resolve('app'), `--talking-quill-user-data=${profile}`],
    {
      stdio: 'ignore',
      windowsHide: true,
      env: { ...process.env, NODE_ENV: 'test' },
    },
  );
  await new Promise<void>((resolveExit, reject) => {
    secondInstance.once('exit', () => resolveExit());
    secondInstance.once('error', reject);
  });
  await expect
    .poll(() =>
      application.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()
          .find((window) => window.getTitle() === 'Talking Quill')
          ?.isVisible(),
      ),
    )
    .toBe(true);

  await main.getByRole('button', { name: 'Settings' }).click();
  await main.getByRole('checkbox', { name: 'Close to tray' }).click();
  await expect(main.getByRole('checkbox', { name: 'Close to tray' })).not.toBeChecked();
  await expect(main.getByText('Close behavior saved on this device.')).toBeVisible();
  const secret = 'e2e-secret-never-rendered';
  const vaultStatus = await main.evaluate(async (value) => {
    const state = await window.talkingQuill.providers.secretStatus('openai');
    return window.talkingQuill.providers.setSecret('openai', state.bindingToken, value);
  }, secret);
  expect(vaultStatus.configured).toBe(true);
  expect(await main.locator('body').textContent()).not.toContain(secret);
  expect(await main.evaluate(() => JSON.stringify(localStorage))).not.toContain(secret);

  const shutdownSecret = 'queued-vault-secret';
  const firstClose = application.waitForEvent('close');
  await main.evaluate(async (value) => {
    const state = await window.talkingQuill.providers.secretStatus('groq');
    // Start the write IPC before shutdown so transport draining owns it.
    void window.talkingQuill.providers.setSecret('groq', state.bindingToken, value);
    void window.talkingQuill.app.setEnabled(false);
    void window.talkingQuill.windowControls.close();
  }, shutdownSecret);
  await firstClose;
  const encryptedVault = await readFile(resolve(profile, 'credentials.enc'), 'utf8');
  expect(encryptedVault).not.toContain(secret);
  expect(encryptedVault).not.toContain(shutdownSecret);

  application = await launch(profile);
  const restarted = (await rendererPages(application)).main;
  await restarted.getByRole('button', { name: 'Settings' }).click();
  await expect(restarted.getByRole('checkbox', { name: 'Close to tray' })).not.toBeChecked();
  await expect(restarted.getByRole('checkbox', { name: 'Enable Talking Quill' })).not.toBeChecked();
  expect(
    await restarted.evaluate(() => window.talkingQuill.providers.secretStatus('groq')),
  ).toMatchObject({ providerId: 'groq', configured: true });
  const secondClose = application.waitForEvent('close');
  await application.evaluate(({ app }) => app.quit());
  await secondClose;
});

test('applies the runtime reduced-motion fallback', async () => {
  const profile = await resetProfile('source-reduced-motion');
  const application = await launch(profile);
  try {
    const { main } = await rendererPages(application);
    await main.emulateMedia({ reducedMotion: 'reduce' });
    expect(
      await main.evaluate(() => {
        const style = getComputedStyle(document.body);
        const milliseconds = (duration: string) =>
          duration.endsWith('ms')
            ? Number.parseFloat(duration)
            : Number.parseFloat(duration) * 1_000;
        return {
          matches: matchMedia('(prefers-reduced-motion: reduce)').matches,
          animationMilliseconds: milliseconds(style.animationDuration),
          transitionMilliseconds: milliseconds(style.transitionDuration),
        };
      }),
    ).toEqual({ matches: true, animationMilliseconds: 0.01, transitionMilliseconds: 0.01 });
  } finally {
    await application.close();
  }
});

test('authentic opt-in npm Pi is auto-discovered and shown through the source Settings path', async () => {
  test.skip(
    process.env.TALKING_QUILL_REAL_NPM_PI_PREFIX === undefined,
    'requires isolated npm evidence',
  );
  test.setTimeout(60_000);
  const prefix = process.env.TALKING_QUILL_REAL_NPM_PI_PREFIX;
  if (prefix === undefined) return;
  const profile = await resetProfile(`pi-authentic-profile-${Date.now().toString(36)}`);
  const application = await launch(profile, false, {
    PATH: `${prefix};${process.env.PATH ?? ''}`,
  });
  try {
    const { main } = await rendererPages(application);
    await expect
      .poll(() => main.evaluate(() => window.talkingQuill.providers.piInstallationStatus()))
      .toMatchObject({ state: 'ready', version: '0.81.1' });
    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Smart processing' }).click();
    await main.getByRole('button', { name: /Ollama.*Run LLMs locally/i }).click();
    await main.getByRole('searchbox', { name: 'Search providers' }).fill('Pi');
    await main.locator('#pi[role="option"]').click();
    await expect(main.getByText(/Pi 0\.81\.1 — ready/i)).toBeVisible({ timeout: 30_000 });
    await main.getByRole('textbox', { name: 'Pi installation path' }).fill(prefix);
    await main.getByRole('button', { name: 'Save path' }).click();
    await expect
      .poll(() => main.evaluate(() => window.talkingQuill.providers.piInstallationStatus()), {
        timeout: 30_000,
      })
      .toMatchObject({ state: 'ready', mode: 'configured', version: '0.81.1' });
    await main.getByRole('button', { name: 'Auto-detect' }).click();
    await expect(main.getByText(/Pi 0\.81\.1 — ready/i)).toBeVisible({ timeout: 30_000 });
  } finally {
    await application.close().catch(() => undefined);
  }
});

test('Pi settings expose not-found recovery and explicit retry discovery', async () => {
  const profile = await resetProfile(`pi-refresh-profile-${Date.now().toString(36)}`);
  const application = await launch(profile, false, { TALKING_QUILL_PI_TEST_UNAVAILABLE: '1' });
  try {
    const { main } = await rendererPages(application);
    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Smart processing' }).click();
    await main.getByRole('button', { name: /Ollama.*Run LLMs locally/i }).click();
    await main.getByRole('searchbox', { name: 'Search providers' }).fill('Pi');
    await main.locator('#pi[role="option"]').click();
    await expect(main.getByText(/Pi is unavailable.*installation path and retry/i)).toBeVisible();
    await main.getByRole('button', { name: 'Retry discovery' }).click();
    await expect(main.getByText(/Pi is unavailable.*installation path and retry/i)).toBeVisible();
  } finally {
    await Promise.race([
      application.close(),
      new Promise<void>((resolveClose) => setTimeout(resolveClose, 5_000)),
    ]);
  }
});

test('provider UI and a fresh-renderer canary prove secrets stay outside renderer stores and responses', async () => {
  test.setTimeout(90_000);
  const profile = await resetProfile('provider-canary-profile');
  const secret = 'fresh-renderer-hostile-echo-canary';
  let receivedCredential = false;
  const server = createServer((request, response) => {
    receivedCredential ||= request.headers.authorization === `Bearer ${secret}`;
    response.setHeader('content-type', 'application/json');
    if (request.method === 'GET' && request.url === '/v1/models') {
      response.end(
        JSON.stringify({
          data: [
            { id: 'manual-safe-model', name: 'Manual safe model' },
            { id: secret, name: `hostile-${secret}` },
          ],
        }),
      );
      return;
    }
    if (request.method === 'POST' && request.url === '/v1/chat/completions') {
      request.resume();
      response.end(JSON.stringify({ choices: [{ message: { content: 'safe completion' } }] }));
      return;
    }
    response.statusCode = 404;
    response.end(JSON.stringify({ error: 'not found' }));
  });
  await new Promise<void>((resolveListen, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolveListen);
  });
  const address = server.address();
  if (typeof address !== 'object' || address === null) throw new Error('Mock server unavailable');
  let oldAuthorizationSentToEndpointB = false;
  const endpointB = createServer((request, response) => {
    oldAuthorizationSentToEndpointB ||= request.headers.authorization === `Bearer ${secret}`;
    response.setHeader('content-type', 'application/json');
    if (request.method === 'GET' && request.url === '/v1/models') {
      response.end(JSON.stringify({ data: [{ id: 'manual-safe-model' }] }));
      return;
    }
    response.end(JSON.stringify({ choices: [{ message: { content: 'safe completion' } }] }));
  });
  await new Promise<void>((resolveListen, reject) => {
    endpointB.once('error', reject);
    endpointB.listen(0, '127.0.0.1', resolveListen);
  });
  const addressB = endpointB.address();
  if (typeof addressB !== 'object' || addressB === null) throw new Error('Mock server unavailable');

  let application = await launch(profile);
  try {
    const main = (await rendererPages(application)).main;
    await main.evaluate(() => {
      const events: string[] = [];
      window.talkingQuill.settings.onChanged((next) => events.push(JSON.stringify(next)));
      Reflect.set(globalThis, '__providerSettingsEvents', events);
    });
    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Smart processing' }).click();
    await main.getByRole('button', { name: /Ollama.*Run LLMs locally/i }).click();
    const providerLogos = main
      .getByRole('listbox', { name: 'Smart processing providers' })
      .locator('img');
    await expect(providerLogos).toHaveCount(38);
    await expect
      .poll(() =>
        providerLogos.evaluateAll((images) =>
          images.every(
            (image) =>
              image instanceof HTMLImageElement && image.complete && image.naturalWidth > 0,
          ),
        ),
      )
      .toBe(true);
    await main.getByRole('searchbox', { name: 'Search providers' }).fill('Generic OpenAI');
    await main.getByRole('option', { name: /^Generic OpenAI.*Connect/ }).click();
    await main.getByLabel('Endpoint URL').fill(`http://127.0.0.1:${String(address.port)}/v1`);
    await main.getByRole('textbox', { name: 'Model', exact: true }).fill('manual-safe-model');
    await main.getByRole('button', { name: 'Save configuration' }).click();
    await expect(main.getByText('Draft saved')).toBeVisible();

    const password = main.locator('input[type="password"]');
    await password.fill(secret);
    await main.getByRole('button', { name: 'Store API key' }).click();
    await expect(password).toHaveValue('');
    await expect(main.getByText('Configured', { exact: true })).toBeVisible();
    await main.getByRole('button', { name: 'Discover models' }).click();
    await expect(main.getByText('1 models found')).toBeVisible();
    await main.getByRole('button', { name: 'Test connection' }).click();
    await expect(main.getByText(/Connection verified/)).toBeVisible();
    await expect(main.getByText(/Local destination — verified/)).toBeVisible();

    const rendererEvidence = await main.evaluate(async () => {
      const bootstrap = await window.talkingQuill.app.getBootstrap();
      const models = await window.talkingQuill.providers.listModels(
        'generic-openai',
        'e2e-canary-models',
      );
      const testResult = await window.talkingQuill.providers.testConnection(
        'generic-openai',
        'e2e-canary-test',
      );
      const events: unknown = Reflect.get(globalThis, '__providerSettingsEvents');
      return JSON.stringify({
        bootstrap,
        models,
        testResult,
        events,
        body: document.body.textContent,
        localStorage: JSON.stringify(localStorage),
        sessionStorage: JSON.stringify(sessionStorage),
      });
    });
    expect(rendererEvidence).not.toContain(secret);
    expect(rendererEvidence).toContain('manual-safe-model');
    expect(receivedCredential).toBe(true);

    await main.getByLabel('Endpoint URL').fill(`http://127.0.0.1:${String(addressB.port)}/v1`);
    await expect(
      main.getByText(/Provider destination changed.*re-enter credentials/i),
    ).toBeVisible();
    await main.getByRole('button', { name: 'Save configuration' }).click();
    await expect(main.getByText('Not configured')).toBeVisible();
    await main.getByRole('button', { name: 'Discover models' }).click();
    await expect(main.getByText('1 models found')).toBeVisible();
    expect(oldAuthorizationSentToEndpointB).toBe(false);

    const firstExit = application.waitForEvent('close');
    await application.evaluate(({ app }) => app.quit());
    await firstExit;
    const settingsFile = await readFile(resolve(profile, 'settings.json'), 'utf8');
    const vaultFile = await readFile(resolve(profile, 'credentials.enc'), 'utf8');
    expect(settingsFile).not.toContain(secret);
    expect(vaultFile).not.toContain(secret);
    for (const userDataFile of await filesBelow(profile)) {
      expect(await readFile(userDataFile, 'utf8')).not.toContain(secret);
    }

    application = await launch(profile);
    const freshRenderer = (await rendererPages(application)).main;
    const freshEvidence = await freshRenderer.evaluate(async () =>
      JSON.stringify({
        bootstrap: await window.talkingQuill.app.getBootstrap(),
        status: await window.talkingQuill.providers.secretStatus('generic-openai'),
        body: document.body.textContent,
        localStorage: JSON.stringify(localStorage),
        sessionStorage: JSON.stringify(sessionStorage),
      }),
    );
    expect(freshEvidence).not.toContain(secret);
    expect(freshEvidence).toContain('"configured":false');
  } finally {
    const exit = application.waitForEvent('close');
    await application.evaluate(({ app }) => app.quit()).catch(() => undefined);
    await exit.catch(() => undefined);
    await Promise.all([
      new Promise<void>((resolveClose) => server.close(() => resolveClose())),
      new Promise<void>((resolveClose) => endpointB.close(() => resolveClose())),
    ]);
  }
});

for (const attempt of [0, 1, 2]) {
  test(`immediate close-to-tray lifecycle is deterministic (isolated case ${String(attempt + 1)})`, async () => {
    test.setTimeout(45_000);
    const profile = await resetProfile(`close-race-${String(attempt)}`);
    let application: ElectronApplication | null = await launch(profile);
    try {
      let main = (await rendererPages(application)).main;
      await main.getByRole('button', { name: 'Settings' }).click();
      await expect(main.getByRole('checkbox', { name: 'Close to tray' })).toBeVisible();

      await closeWithDiagnostics(application, `case ${String(attempt + 1)} immediate quit`, () =>
        toggleCloseToTrayAndCloseImmediately(main),
      );

      application = await launch(profile);
      main = (await rendererPages(application)).main;
      await main.getByRole('button', { name: 'Settings' }).click();
      await expect(main.getByRole('checkbox', { name: 'Close to tray' })).toBeVisible();
      await expect(main.getByRole('checkbox', { name: 'Close to tray' })).not.toBeChecked();

      await toggleCloseToTrayAndCloseImmediately(main);
      await expect
        .poll(() =>
          application?.evaluate(({ BrowserWindow }) =>
            BrowserWindow.getAllWindows()
              .find((window) => window.getTitle() === 'Talking Quill')
              ?.isVisible(),
          ),
        )
        .toBe(false);
      const persisted = JSON.parse(await readFile(resolve(profile, 'settings.json'), 'utf8')) as {
        readonly app: { readonly closeToTray: boolean };
      };
      expect(persisted.app.closeToTray).toBe(true);

      await closeWithDiagnostics(
        application,
        `case ${String(attempt + 1)} final quit`,
        () => application?.evaluate(({ app }) => app.quit()) ?? Promise.resolve(),
      );
      application = null;
    } finally {
      await application?.close().catch(() => undefined);
    }
  });
}

test('categorized source egress proof exposes only explicit user-triggered choke points', async () => {
  const profile = await resetProfile('source-egress-proof');
  const application = await launch(profile, true);
  try {
    const { main } = await rendererPages(application);
    await expect(main.getByRole('heading', { name: 'Dictation needs local setup' })).toBeVisible();
    expect(await readEgressCategories(profile)).toEqual([]);

    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Smart processing' }).click();
    const providerForm = main.locator('.provider-form');
    await providerForm.getByLabel('Model').fill('proof-model');
    await providerForm.getByRole('button', { name: 'Save configuration' }).click();
    await expect(providerForm.getByText('Draft saved')).toBeVisible();
    await main.getByRole('button', { name: 'Test connection' }).click();
    await expect.poll(() => readEgressCategories(profile)).toEqual(['provider']);

    await main.getByRole('button', { name: 'Info' }).click();
    await main.getByRole('button', { name: 'Check for updates' }).click();
    await expect.poll(() => readEgressCategories(profile)).toEqual(['provider', 'update']);

    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Transcription model' }).click();
    await main.getByRole('button', { name: 'Download model' }).click();
    await expect
      .poll(() => readEgressCategories(profile))
      .toEqual(['provider', 'update', 'model-download']);
  } finally {
    await application.close();
  }
});

async function filesBelow(root: string): Promise<readonly string[]> {
  const files: string[] = [];
  const walk = async (directory: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true }).catch(() => []);
    for (const entry of entries) {
      const path = resolve(directory, entry.name);
      if (entry.isDirectory()) await walk(path);
      else if (entry.isFile()) files.push(path);
    }
  };
  await walk(root);
  return files;
}
