import { spawn, type ChildProcess } from 'node:child_process';
import { cp, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { delimiter, dirname, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { chromium, expect, test, type Browser, type Page } from '@playwright/test';
import { HistoryStore } from '../../app/src/main/persistence/history-store';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';

const packageRoot = resolve(process.env.TALKING_QUILL_PACKAGE_ROOT ?? 'release/win-unpacked');
const executable = resolve(
  packageRoot,
  process.env.TALKING_QUILL_PACKAGE_EXECUTABLE ??
    (process.platform === 'darwin'
      ? 'Talking Quill.app/Contents/MacOS/Talking Quill'
      : 'Talking Quill.exe'),
);
const fakeAudioFile = resolve('tests/fixtures/fake-microphone.wav');
const retainedThumbnailFixture = resolve('app/assets/provider-logos/mistral.jpeg');

interface PackagedApplication {
  readonly child: ChildProcess;
  readonly browser: Browser;
  readonly main: Page;
  readonly widget: Page;
  readonly capture: Page;
  readonly diagnostics: () => string;
}

async function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (typeof address === 'object' && address !== null)
        server.close(() => resolvePort(address.port));
      else reject(new Error('No debugging port'));
    });
  });
}

async function seedRetainedScreenshotHistory(profile: string): Promise<void> {
  const onboarded = structuredClone(DEFAULT_SETTINGS);
  onboarded.welcome = {
    completedAt: 1,
    lastStep: 5,
    microphoneTested: true,
    activationTested: true,
  };
  await writeFile(resolve(profile, 'settings.json'), `${JSON.stringify(onboarded)}\n`, 'utf8');
  const screenshots = resolve(profile, 'screenshots');
  await mkdir(screenshots, { recursive: true });
  const validJpeg = await readFile(retainedThumbnailFixture);
  await Promise.all([
    writeFile(resolve(screenshots, 'packaged-valid.jpg'), validJpeg),
    writeFile(resolve(screenshots, 'packaged-valid.thumb.jpg'), validJpeg),
    writeFile(resolve(screenshots, 'packaged-malformed.jpg'), Buffer.from('not-a-jpeg')),
    writeFile(resolve(screenshots, 'packaged-malformed.thumb.jpg'), Buffer.from('<svg></svg>')),
  ]);
  const store = new HistoryStore(resolve(profile, 'history.db'));
  try {
    const common = {
      dictationMode: 'quick' as const,
      processingMode: 'smart' as const,
      outcome: 'smart-completed' as const,
      rawText: 'raw packaged thumbnail fixture',
      providerId: 'ollama',
      modelId: 'fixture-vision',
      fellBack: false,
      errorCategory: null,
      voiceTrigger: null,
      voiceSnippet: null,
    };
    store.create({
      ...common,
      createdAt: Date.now() - 1,
      processedText: 'Packaged valid thumbnail',
      screenshotFilename: 'packaged-valid.jpg',
    });
    store.create({
      ...common,
      createdAt: Date.now(),
      processedText: 'Packaged malformed thumbnail',
      screenshotFilename: 'packaged-malformed.jpg',
    });
  } finally {
    store.close();
  }
}

async function launchPackaged(
  profile: string,
  interactivePiAppData?: string,
  egressProof = true,
  mediaHarness = false,
): Promise<PackagedApplication> {
  const port = await freePort();
  let stage = 'native-spawn';
  const child = spawn(
    executable,
    [
      `--remote-debugging-port=${String(port)}`,
      `--talking-quill-user-data=${profile}`,
      '--use-fake-device-for-media-stream',
      `--use-file-for-fake-audio-capture=${fakeAudioFile}`,
      ...(mediaHarness
        ? ['--talking-quill-task6-real-media', '--use-fake-ui-for-media-stream']
        : []),
    ],
    {
      stdio: ['ignore', 'pipe', 'pipe'],
      windowsHide: true,
      env: {
        ...process.env,
        ELECTRON_RENDERER_URL: 'http://127.0.0.1:9',
        TALKING_QUILL_VERIFY_WHISPER_RUNTIME: '1',
        TALKING_QUILL_PACKAGED_TEST: '1',
        ...(mediaHarness ? { TALKING_QUILL_PACKAGED_MEDIA_HARNESS: '1' } : {}),
        TALKING_QUILL_EGRESS_PROOF: egressProof ? '1' : '0',
        CI: 'true',
        ...(interactivePiAppData === undefined
          ? {}
          : {
              PATH: [
                dirname(process.execPath),
                resolve(process.env.SystemRoot ?? 'C:\\Windows', 'System32'),
              ].join(delimiter),
              TALKING_QUILL_TEST_INTERACTIVE_APPDATA: interactivePiAppData,
              TALKING_QUILL_TEST_INTERACTIVE_HOME: resolve(interactivePiAppData, '..', '..'),
              PI_CODING_AGENT_DIR: resolve(interactivePiAppData, '..', '..', '.pi', 'agent'),
            }),
        ...(process.platform === 'win32'
          ? { APPDATA: dirname(profile) }
          : { HOME: resolve(profile, '..', '..', '..') }),
      },
    },
  );
  let diagnostics = '';
  for (const stream of [child.stdout, child.stderr]) {
    stream.on('data', (chunk: Buffer) => {
      diagnostics = `${diagnostics}${chunk.toString()}`.slice(-8_192);
    });
  }
  child.once('error', (error) => {
    diagnostics =
      `${diagnostics}\nNative process error: ${error.name}: ${error.message}${error.stack === undefined ? '' : `\n${error.stack}`}`.slice(
        -8_192,
      );
  });
  child.once('exit', (code, signal) => {
    diagnostics =
      `${diagnostics}\nExited with code ${String(code)}, signal ${String(signal)}`.slice(-8_192);
  });

  const failure = async (message: string): Promise<Error> => {
    const processTree = await readProcessTree(child.pid);
    return new Error(
      `${message} (stage=${stage}, pid=${String(child.pid ?? 'unavailable')})\nProcess tree:\n${processTree}\nNative output:\n${diagnostics || '(none)'}`,
    );
  };
  stage = 'debug-endpoint';
  const endpoint = `http://127.0.0.1:${String(port)}`;
  const deadline = Date.now() + 20_000;
  let browser: Browser | null = null;
  while (Date.now() < deadline && browser === null) {
    await delay(125);
    browser = await chromium.connectOverCDP(endpoint, { timeout: 500 }).catch(() => null);
  }
  if (browser === null) {
    const error = await failure('Packaged app did not expose a renderer debugging endpoint');
    child.kill();
    throw error;
  }

  stage = 'renderer-roles';
  let main: Page | undefined;
  let widget: Page | undefined;
  let capture: Page | undefined;
  while (
    Date.now() < deadline &&
    (main === undefined || widget === undefined || capture === undefined)
  ) {
    const pages = browser.contexts().flatMap((context) => context.pages());
    main = pages.find((candidate) => candidate.url().includes('/main/index.html'));
    widget = pages.find((candidate) => candidate.url().includes('/widget/index.html'));
    capture = pages.find((candidate) => candidate.url().includes('/capture/index.html'));
    if (main === undefined || widget === undefined || capture === undefined) await delay(125);
  }
  if (main === undefined || widget === undefined || capture === undefined) {
    const error = await failure('Packaged renderer roles are incomplete');
    await browser.close();
    child.kill();
    throw error;
  }
  stage = 'ready';
  return { child, browser, main, widget, capture, diagnostics: () => diagnostics };
}

async function readProcessTree(pid: number | undefined): Promise<string> {
  if (pid === undefined) return '(native process did not receive a pid)';
  const command =
    process.platform === 'win32'
      ? {
          file: 'powershell.exe',
          args: [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            `$all=Get-CimInstance Win32_Process; $ids=@(${String(pid)}); do {$before=$ids.Count; $ids+=@($all|Where-Object {$ids -contains $_.ParentProcessId}|ForEach-Object ProcessId); $ids=@($ids|Sort-Object -Unique)} while ($ids.Count -ne $before); $all|Where-Object {$ids -contains $_.ProcessId}|Select-Object ProcessId,ParentProcessId,Name|Format-Table -AutoSize`,
          ],
        }
      : { file: 'ps', args: ['-axo', 'pid=,ppid=,state=,comm='] };
  return new Promise((resolveSnapshot) => {
    const probe = spawn(command.file, command.args, { stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    let settled = false;
    const finish = (value: string) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolveSnapshot(value.slice(-16_384));
    };
    const timer = setTimeout(() => {
      probe.kill();
      finish('process-tree probe timed out');
    }, 3_000);
    probe.stdout.on(
      'data',
      (chunk: Buffer) => (output = `${output}${chunk.toString()}`.slice(-32_768)),
    );
    probe.stderr.on(
      'data',
      (chunk: Buffer) => (output = `${output}${chunk.toString()}`.slice(-32_768)),
    );
    probe.once('error', (error) => finish(`process-tree probe error: ${error.message}`));
    probe.once('close', () => {
      const safeOutput =
        process.platform === 'win32' ? output : filterPosixProcessTree(output, pid);
      finish(safeOutput.trim() || '(no owned processes found)');
    });
  });
}

function filterPosixProcessTree(source: string, rootPid: number): string {
  const rows = source.split(/\r?\n/u).flatMap((line) => {
    const match = /^\s*(\d+)\s+(\d+)\s+(\S+)\s+(.*)$/u.exec(line);
    return match?.[1] === undefined || match[2] === undefined
      ? []
      : [{ line, pid: Number(match[1]), parent: Number(match[2]) }];
  });
  const owned = new Set([rootPid]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const row of rows) {
      if (owned.has(row.parent) && !owned.has(row.pid)) {
        owned.add(row.pid);
        changed = true;
      }
    }
  }
  return rows
    .filter((row) => owned.has(row.pid))
    .map((row) => row.line)
    .join('\n');
}

function waitForExit(child: ChildProcess, timeout = 10_000): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) {
    const error = abnormalExit(child.exitCode, child.signalCode);
    return error === null ? Promise.resolve() : Promise.reject(error);
  }
  return new Promise((resolveExit, reject) => {
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error('Packaged app did not exit'));
    }, timeout);
    const cleanup = () => {
      clearTimeout(timer);
      child.off('exit', onExit);
      child.off('error', onError);
    };
    const onExit = (code: number | null, signal: NodeJS.Signals | null) => {
      cleanup();
      const error = abnormalExit(code, signal);
      if (error === null) resolveExit();
      else reject(error);
    };
    const onError = (error: Error) => {
      cleanup();
      reject(error);
    };
    child.once('exit', onExit);
    child.once('error', onError);
  });
}

function abnormalExit(code: number | null, signal: NodeJS.Signals | null): Error | null {
  return code === 0 && signal === null
    ? null
    : new Error(`Packaged app exited abnormally (code ${String(code)}, signal ${String(signal)})`);
}

async function readEgressCategories(profile: string): Promise<string[]> {
  const source = await readFile(resolve(profile, 'tmp', 'egress-proof.jsonl'), 'utf8');
  return source
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((line) => (JSON.parse(line) as { readonly category: string }).category);
}

async function dispose(application: PackagedApplication | null): Promise<void> {
  if (application === null) return;
  await application.browser.close().catch(() => undefined);
  if (application.child.exitCode === null && application.child.signalCode === null) {
    application.child.kill();
    await waitForExit(application.child, 5_000).catch(() => undefined);
  }
}

test('packaged fake media traverses capture, session, and widget with reset', async () => {
  test.skip(
    process.env.TALKING_QUILL_PACKAGED_MEDIA_HARNESS !== '1',
    'requires the isolated packaged media harness',
  );
  const profile = resolve('tmp/tests/instrumented-packaged-media-profile');
  await rm(profile, { recursive: true, force: true });
  await mkdir(profile, { recursive: true });
  const launched = await launchPackaged(profile, undefined, false, true);
  try {
    await launched.widget.emulateMedia({ reducedMotion: 'reduce' });
    const meter = launched.widget.getByRole('meter', { name: 'Microphone level' });
    await expect
      .poll(async () => Number(await meter.getAttribute('aria-valuenow')))
      .toBeGreaterThan(0);
    await launched.widget.screenshot({
      path: resolve('tmp', 'instrumented-packaged-widget.png'),
      animations: 'disabled',
    });
    await expect(meter).toHaveAttribute('aria-valuetext', /percent, (?:speaking|loud)/iu);
    expect(
      await launched.widget.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches),
    ).toBe(true);
    await launched.widget.getByRole('button', { name: 'Cancel dictation' }).click();
    await expect(meter).toHaveAttribute('aria-valuenow', '0');
    await expect(meter).toHaveAttribute('aria-valuetext', '0 percent, silent');
    await launched.widget.reload();
    await expect(launched.widget.getByRole('meter', { name: 'Microphone level' })).toHaveAttribute(
      'aria-valuenow',
      '0',
    );
  } finally {
    await dispose(launched);
  }
});

test('real packaged app opens every renderer, isolates Node, and persists a setting', async () => {
  test.setTimeout(480_000);
  const target = process.env.TALKING_QUILL_PACKAGE_TARGET;
  if (target === 'mac') expect(process.platform).toBe('darwin');
  if (target === 'win') expect(process.platform).toBe('win32');

  const profile =
    process.platform === 'darwin'
      ? resolve('tmp', 'packaged-smoke', 'home', 'Library', 'Application Support', 'Talking Quill')
      : resolve('tmp', 'packaged-smoke', 'profile', 'Talking Quill');
  await rm(profile, { recursive: true, force: true, maxRetries: 3 });
  await mkdir(profile, { recursive: true });
  await seedRetainedScreenshotHistory(profile);
  let launched: PackagedApplication | null = null;
  try {
    launched = await launchPackaged(profile);
    const rendererErrors: string[] = [];
    launched.main.on('console', (message) => {
      if (message.type() === 'error') rendererErrors.push(message.text());
    });
    launched.main.on('pageerror', (error) => rendererErrors.push(error.message));
    await launched.main.evaluate(() => {
      const violations: string[] = [];
      Reflect.set(globalThis, '__packagedThumbnailCspViolations', violations);
      window.addEventListener('securitypolicyviolation', (event) => {
        if (event.violatedDirective.startsWith('img-src') || event.blockedURI.startsWith('blob:')) {
          violations.push(`${event.violatedDirective}:${event.blockedURI}`);
        }
      });
    });
    // Mount the dictation history after diagnostics are attached so thumbnail loading and CSP
    // enforcement are observed rather than relying on renderer events emitted before CDP connected.
    await launched.main.getByRole('button', { name: 'Dictation history' }).click();
    await expect(
      launched.main.getByRole('heading', { name: 'Dictation history', exact: true }),
    ).toBeVisible();
    // Categorized in-process instrumentation is intentionally blocked before socket I/O. This is
    // scenario-routing evidence, not OS packet-capture evidence.
    expect(await readEgressCategories(profile)).toEqual([]);
    const validEntry = launched.main.locator('li.history-entry').filter({
      hasText: 'Packaged valid thumbnail',
    });
    const validThumbnail = validEntry.getByRole('img', {
      name: 'On-Screen Awareness context thumbnail',
    });
    await expect(validThumbnail).toBeVisible();
    await expect
      .poll(() => validThumbnail.evaluate((image: HTMLImageElement) => image.naturalWidth))
      .toBeGreaterThan(0);
    const malformedEntry = launched.main.locator('li.history-entry').filter({
      hasText: 'Packaged malformed thumbnail',
    });
    await expect(malformedEntry.getByText('Screenshot unavailable')).toBeVisible();
    await expect(malformedEntry.locator('img')).toHaveCount(0);
    expect(
      await launched.main.evaluate(() => {
        const value: unknown = Reflect.get(globalThis, '__packagedThumbnailCspViolations');
        return Array.isArray(value) && value.every((item) => typeof item === 'string')
          ? value
          : null;
      }),
    ).toEqual([]);
    expect(rendererErrors).toEqual([]);
    await launched.main.getByRole('button', { name: 'Dashboard' }).click();
    await expect(launched.main.getByRole('heading', { name: 'Almost there' })).toBeVisible();
    const helperReadiness = launched.main
      .locator('.readiness-row')
      .filter({ hasText: 'Typing helper' });
    await expect(helperReadiness).toContainText(
      process.platform === 'win32' ? 'Available' : /Available|Needs permission/,
    );
    expect(await launched.main.evaluate(() => Reflect.has(window.talkingQuill, 'helper'))).toBe(
      false,
    );
    expect(
      await launched.main.evaluate(() => ({
        main: Reflect.has(window, 'talkingQuill'),
        widget: Reflect.has(window, 'talkingQuillWidget'),
        capture: Reflect.has(window, 'talkingQuillCapture'),
        frozen: Object.isFrozen(window.talkingQuill),
        arbitrary: Reflect.has(window.talkingQuill, 'ipc:unknown'),
        rawIpc: typeof Reflect.get(globalThis, 'ipcRenderer'),
      })),
    ).toEqual({
      main: true,
      widget: false,
      capture: false,
      frozen: true,
      arbitrary: false,
      rawIpc: 'undefined',
    });
    expect(
      await launched.widget.evaluate(() => ({
        main: Reflect.has(window, 'talkingQuill'),
        widget: Reflect.has(window, 'talkingQuillWidget'),
        capture: Reflect.has(window, 'talkingQuillCapture'),
        keys: Object.keys(window.talkingQuillWidget),
        frozen: Object.isFrozen(window.talkingQuillWidget),
        arbitrary: Reflect.has(window.talkingQuillWidget, 'ipc:unknown'),
        rawIpc: typeof Reflect.get(globalThis, 'ipcRenderer'),
      })),
    ).toEqual({
      main: false,
      widget: true,
      capture: false,
      keys: ['ready', 'stop', 'cancel', 'setInteractive', 'onSessionChanged'],
      frozen: true,
      arbitrary: false,
      rawIpc: 'undefined',
    });
    expect(
      await launched.capture.evaluate(() => ({
        main: Reflect.has(window, 'talkingQuill'),
        widget: Reflect.has(window, 'talkingQuillWidget'),
        capture: Reflect.has(window, 'talkingQuillCapture'),
        keys: Object.keys(window.talkingQuillCapture),
        frozen: Object.isFrozen(window.talkingQuillCapture),
        arbitrary: Reflect.has(window.talkingQuillCapture, 'ipc:unknown'),
        rawIpc: typeof Reflect.get(globalThis, 'ipcRenderer'),
      })),
    ).toEqual({
      main: false,
      widget: false,
      capture: true,
      keys: ['ready'],
      frozen: true,
      arbitrary: false,
      rawIpc: 'undefined',
    });
    // The hidden session widget reports its idle session state; setup readiness is owned by main.
    await expect(launched.widget.getByText('Ready', { exact: true }).first()).toBeVisible();
    await expect
      .poll(() => launched?.capture.evaluate(() => document.documentElement.dataset.ready))
      .toBe('true');
    await expect
      .poll(async () => {
        try {
          const result = JSON.parse(
            await readFile(resolve(profile, 'tmp', 'whisper-runtime-check.json'), 'utf8'),
          ) as { readonly code?: unknown };
          return result.code;
        } catch {
          return null;
        }
      })
      .toBe('MODEL_MISSING');
    expect(launched.main.url()).toContain('talking-quill://app/main/index.html');
    expect(await launched.main.evaluate(() => typeof Reflect.get(globalThis, 'require'))).toBe(
      'undefined',
    );
    await launched.main.keyboard.press(
      process.platform === 'darwin' ? 'Meta+Alt+i' : 'Control+Shift+i',
    );
    await delay(250);
    expect(
      launched.browser
        .contexts()
        .flatMap((context) => context.pages())
        .every((page) => !page.url().startsWith('devtools://')),
    ).toBe(true);

    await launched.main.getByRole('button', { name: 'Settings' }).click();
    await launched.main.getByRole('button', { name: 'Recording' }).click();
    await launched.main.getByRole('button', { name: 'Test my microphone' }).click();
    await expect(launched.main.getByText('Listening — say something')).toBeVisible({
      timeout: 15_000,
    });
    await expect
      .poll(
        async () =>
          Number(
            await launched?.main
              .getByRole('progressbar', { name: 'How loud you are' })
              .getAttribute('value'),
          ),
        { timeout: 15_000 },
      )
      .toBeGreaterThan(0.01);
    await launched.main.getByRole('button', { name: 'Stop test' }).click();
    await expect(launched.main.getByText('Not testing')).toBeVisible();
    await launched.main.getByRole('button', { name: 'General' }).click();
    await launched.main
      .getByRole('checkbox', { name: 'Keep running in the tray when I close the window' })
      .click();
    await expect(
      launched.main.getByRole('checkbox', {
        name: 'Keep running in the tray when I close the window',
      }),
    ).not.toBeChecked();
    await launched.main.getByRole('button', { name: 'Smart processing' }).click();
    await launched.main.getByRole('button', { name: /Ollama.*Run LLMs locally/i }).click();
    const providerLogos = launched.main
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
    // Opening Smart processing now starts automatic model discovery, which is the first
    // categorized provider choke point the proof observer records.
    await expect.poll(() => readEgressCategories(profile)).toEqual(['provider']);
    const providerForm = launched.main.locator('form.stack');
    await providerForm
      .getByRole('combobox', { name: 'Model', exact: true })
      .selectOption('\u0000custom-model');
    await providerForm
      .getByRole('textbox', { name: 'Model name', exact: true })
      .fill('proof-model');
    await providerForm.getByRole('button', { name: 'Save configuration' }).click();
    await expect(providerForm.getByText('Saved', { exact: true })).toBeVisible();
    await launched.main.getByRole('button', { name: 'Test connection' }).click();
    await expect.poll(() => readEgressCategories(profile)).toEqual(['provider', 'provider']);

    await launched.main.getByRole('button', { name: 'About' }).click();
    await expect(launched.main.getByRole('heading', { name: 'About Talking Quill' })).toBeVisible();
    await launched.main.getByRole('button', { name: 'Check for updates' }).click();
    await expect
      .poll(() => readEgressCategories(profile))
      .toEqual(['provider', 'provider', 'update']);
    await launched.main.getByRole('button', { name: 'Settings' }).click();
    await launched.main.getByRole('button', { name: 'Transcription model' }).click();
    await launched.main.getByRole('button', { name: 'Download', exact: true }).click();
    await expect
      .poll(() => readEgressCategories(profile))
      .toEqual(['provider', 'provider', 'update', 'model-download']);
    const firstExit = waitForExit(launched.child);
    await launched.main.getByRole('button', { name: 'Close window' }).click();
    await firstExit;
    await launched.browser.close();
    launched = null;

    launched = await launchPackaged(profile);
    await launched.main.getByRole('button', { name: 'Settings' }).click();
    await launched.main.getByRole('button', { name: 'General' }).click();
    await expect(
      launched.main.getByRole('checkbox', {
        name: 'Keep running in the tray when I close the window',
      }),
    ).not.toBeChecked();
    const secondExit = waitForExit(launched.child);
    await launched.main.getByRole('button', { name: 'Close window' }).click();
    await secondExit;
    await launched.browser.close();
    launched = null;
  } finally {
    await dispose(launched);
  }
});

test('packaged Windows discovers authentic AppData npm Pi under a stale PATH', async () => {
  test.skip(
    process.platform !== 'win32' || process.env.TALKING_QUILL_REAL_NPM_PI_PREFIX === undefined,
    'requires the owned authentic npm-global Pi fixture',
  );
  test.setTimeout(360_000);
  const fixturePrefix = process.env.TALKING_QUILL_REAL_NPM_PI_PREFIX;
  if (fixturePrefix === undefined) return;
  const root = resolve('tmp', 'packaged-authentic-pi');
  const profile = resolve(root, 'profile', 'Talking Quill');
  const interactivePiAppData = resolve(root, 'Users', 'MaSliusareva', 'AppData', 'Roaming');
  const npm = resolve(interactivePiAppData, 'npm');
  await rm(root, { recursive: true, force: true, maxRetries: 3 });
  await mkdir(profile, { recursive: true });
  await cp(fixturePrefix, npm, { recursive: true });
  await rename(resolve(npm, 'pi.cmd'), resolve(npm, 'Pi.CmD'));
  const agent = resolve(interactivePiAppData, '..', '..', '.pi', 'agent');
  await mkdir(agent, { recursive: true });
  await writeFile(
    resolve(agent, 'models.json'),
    JSON.stringify({
      providers: {
        'talking-quill-packaged-fixture': {
          baseUrl: 'http://127.0.0.1:9/v1',
          api: 'openai-completions',
          apiKey: 'synthetic-nonbillable-fixture',
          models: [{ id: 'packaged-model', contextWindow: 8192, maxTokens: 1024 }],
        },
      },
    }),
  );

  let launched: PackagedApplication | null = null;
  try {
    launched = await launchPackaged(profile, interactivePiAppData, false);
    const evidence = await launched.main.evaluate(async () => {
      const status = await window.talkingQuill.providers.piInstallationStatus();
      const models = await window.talkingQuill.providers.listModels(
        'pi',
        'packaged-authentic-pi-models',
        true,
      );
      return { status, models };
    });
    expect(evidence.status).toMatchObject({
      state: 'ready',
      version: '0.81.1',
      source: 'appdata-npm',
    });
    expect(evidence.models).toEqual([
      expect.objectContaining({ id: 'talking-quill-packaged-fixture/packaged-model' }),
    ]);
  } catch (error: unknown) {
    throw new Error(
      `Packaged authentic Pi evidence failed: ${error instanceof Error ? error.message : String(error)}\n${launched?.diagnostics() ?? ''}`,
    );
  } finally {
    await dispose(launched);
  }
});
