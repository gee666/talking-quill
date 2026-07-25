import { readdir } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { _electron as electron, expect, test, type ElectronApplication } from '@playwright/test';
import { rendererPages, resetProfile } from './helpers';

const electronModule: unknown = createRequire(resolve('package.json'))('electron');
if (typeof electronModule !== 'string') throw new Error('Electron executable is unavailable');

type SessionPhase =
  | 'idle'
  | 'arming'
  | 'recordingQuick'
  | 'recordingExtended'
  | 'transcribing'
  | 'processingSmart'
  | 'inserting'
  | 'completed'
  | 'cancelled'
  | 'error';
interface DriverSnapshot {
  readonly session: {
    readonly phase: SessionPhase;
    readonly completion: 'inserted' | 'copied' | null;
    readonly abortReason: string | null;
  };
  readonly recording: { readonly starts: number; readonly stops: number; readonly active: boolean };
  readonly insertion: { readonly calls: number; readonly targetText: string };
  readonly history: readonly {
    readonly outcome: string;
    readonly rawText: string | null;
    readonly processingMode: string;
    readonly voiceTrigger: string | null;
    readonly voiceSnippet: string | null;
  }[];
}

async function driverCall<Result = void>(
  application: ElectronApplication,
  method: string,
  args: readonly unknown[] = [],
): Promise<Result> {
  return application.evaluate(
    (_electron, input) => {
      const driver: unknown = Reflect.get(
        globalThis,
        Symbol.for('talking-quill:task6-test-driver'),
      );
      if (typeof driver !== 'object' || driver === null)
        throw new Error('Task 6 driver unavailable');
      const value: unknown = (driver as Readonly<Record<string, unknown>>)[input.method];
      if (typeof value !== 'function') throw new Error(`Task 6 driver method unavailable`);
      const invoke = value as (...parameters: readonly unknown[]) => unknown;
      return invoke(...input.args);
    },
    { method, args },
  ) as Promise<Result>;
}

async function snapshot(application: ElectronApplication): Promise<DriverSnapshot> {
  return driverCall<DriverSnapshot>(application, 'snapshot');
}

async function waitForPhase(application: ElectronApplication, phase: SessionPhase) {
  await expect.poll(async () => (await snapshot(application)).session.phase).toBe(phase);
}

async function waitForCapture(application: ElectronApplication, starts: number) {
  try {
    await expect.poll(async () => (await snapshot(application)).recording.starts).toBe(starts);
  } catch (error: unknown) {
    throw new Error(
      `Capture ${String(starts)} did not start: ${JSON.stringify(await snapshot(application))}`,
      {
        cause: error,
      },
    );
  }
}

async function resetSession(application: ElectronApplication) {
  await waitForPhase(application, 'idle');
}

async function beginQuick(application: ElectronApplication, captureNumber: number) {
  await driverCall(application, 'activationDown');
  await waitForCapture(application, captureNumber);
  await driverCall(application, 'frames', [0.2, 15]);
  await driverCall(application, 'activationUp');
  await waitForPhase(application, 'recordingQuick');
}

async function beginExtended(application: ElectronApplication, captureNumber: number) {
  await driverCall(application, 'activationDown');
  await waitForCapture(application, captureNumber);
  await new Promise((resolveWait) => setTimeout(resolveWait, 650));
  await waitForPhase(application, 'recordingExtended');
  await driverCall(application, 'activationUp');
  await driverCall(application, 'frames', [0.2, 15]);
}

test('real Chromium fake media traverses capture, session, and widget with reset', async () => {
  test.setTimeout(60_000);
  const profile = await resetProfile('task6-real-media');
  const application = await electron.launch({
    executablePath: electronModule,
    args: [
      resolve('app'),
      `--talking-quill-user-data=${profile}`,
      '--talking-quill-task6-real-media',
      '--use-fake-device-for-media-stream',
      '--use-fake-ui-for-media-stream',
      `--use-file-for-fake-audio-capture=${resolve('tests/fixtures/fake-microphone.wav')}`,
    ],
    env: { ...process.env, NODE_ENV: 'test' },
  });
  try {
    const { capture, main, widget } = await rendererPages(application);
    await widget.emulateMedia({ reducedMotion: 'reduce' });
    await expect
      .poll(() => capture.evaluate(() => document.documentElement.dataset.ready))
      .toBe('true');
    await expect
      .poll(
        async () =>
          (await main.evaluate(() => window.talkingQuill.app.getBootstrap())).state.status,
      )
      .toBe('ready');
    await driverCall(application, 'activationDown');
    await expect
      .poll(async () => (await snapshot(application)).session.phase)
      .toMatch(/recording/u);
    const meter = widget.getByRole('meter', { name: 'Microphone level' });
    await expect
      .poll(async () => Number(await meter.getAttribute('aria-valuenow')))
      .toBeGreaterThan(0);
    await expect(meter).toHaveAttribute('aria-valuetext', /percent, (?:speaking|loud)/iu);
    expect(
      await widget.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches),
    ).toBe(true);
    await driverCall(application, 'key', ['escape']);
    await waitForPhase(application, 'cancelled');
    await resetSession(application);
    await expect(meter).toHaveAttribute('aria-valuenow', '0');
    await expect(meter).toHaveAttribute('aria-valuetext', '0 percent, silent');
    await widget.reload();
    await expect(widget.getByRole('meter', { name: 'Microphone level' })).toHaveAttribute(
      'aria-valuenow',
      '0',
    );
  } finally {
    await application.close();
  }
});

test('widget sizes keep DIP geometry, content, actions, and screenshots aligned', async () => {
  test.setTimeout(90_000);
  const profile = await resetProfile('task6-widget-layout');
  const application = await electron.launch({
    executablePath: electronModule,
    args: [resolve('app'), `--talking-quill-user-data=${profile}`, '--talking-quill-task6-test'],
    env: { ...process.env, NODE_ENV: 'test' },
  });
  try {
    const { main, widget } = await rendererPages(application);
    const sizes = [
      ['default', 360, 96],
      ['large', 440, 112],
      ['huge', 520, 136],
      ['max', 640, 160],
    ] as const;
    for (const [size, width, height] of sizes) {
      await main.evaluate(
        async (widgetSize) => window.talkingQuill.settings.update({ app: { widgetSize } }),
        size,
      );
      await beginExtended(application, sizes.findIndex(([candidate]) => candidate === size) + 1);
      const geometry = await application.evaluate(({ BrowserWindow, screen }) => {
        const window = BrowserWindow.getAllWindows().find(
          (candidate) => candidate.getTitle() === 'Talking Quill Widget',
        );
        if (window === undefined) throw new Error('Widget window unavailable');
        const bounds = window.getContentBounds();
        return { bounds, workArea: screen.getDisplayMatching(bounds).workArea };
      });
      expect(geometry.bounds).toMatchObject({ width, height });
      expect(geometry.bounds.x + width).toBeLessThanOrEqual(
        geometry.workArea.x + geometry.workArea.width,
      );
      expect(geometry.bounds.y + height).toBeLessThanOrEqual(
        geometry.workArea.y + geometry.workArea.height,
      );
      const layout = await widget.evaluate(() => {
        const shell = document.querySelector<HTMLElement>('.widget-shell');
        if (shell === null) throw new Error('Widget shell unavailable');
        const elements = [
          shell,
          document.querySelector<HTMLElement>('.widget-level'),
          document.querySelector<HTMLElement>('.widget-copy'),
          document.querySelector<HTMLElement>('time'),
          ...document.querySelectorAll<HTMLElement>('.widget-actions button'),
        ].filter((element): element is HTMLElement => element !== null);
        const shellRect = shell.getBoundingClientRect();
        return {
          viewport: { width: innerWidth, height: innerHeight },
          overflow: {
            width: document.documentElement.scrollWidth,
            height: document.documentElement.scrollHeight,
          },
          shell: {
            left: shellRect.left,
            top: shellRect.top,
            right: shellRect.right,
            bottom: shellRect.bottom,
          },
          contained: elements.every((element) => {
            const rect = element.getBoundingClientRect();
            return (
              rect.left >= shellRect.left - 1 &&
              rect.top >= shellRect.top - 1 &&
              rect.right <= shellRect.right + 1 &&
              rect.bottom <= shellRect.bottom + 1
            );
          }),
        };
      });
      expect(layout.viewport).toEqual({ width, height });
      expect(layout.overflow.width).toBeLessThanOrEqual(width);
      expect(layout.overflow.height).toBeLessThanOrEqual(height);
      expect(layout.shell.left).toBeCloseTo(0, 1);
      expect(layout.shell.top).toBeCloseTo(0, 1);
      expect(layout.shell.right).toBeCloseTo(width, 1);
      expect(layout.shell.bottom).toBeCloseTo(height, 1);
      expect(layout.contained).toBe(true);
      await widget.screenshot({
        path: resolve('tmp', `widget-${size}.png`),
        animations: 'disabled',
      });
      await widget.getByRole('button', { name: 'Cancel dictation' }).click();
      await waitForPhase(application, 'cancelled');
      await resetSession(application);
    }
  } finally {
    await application.close();
  }
});

test('Task 6 deterministic composition drives gestures, widget, insertion, and teardown', async () => {
  test.setTimeout(90_000);
  const profile = await resetProfile('task6-scenarios');
  const application = await electron.launch({
    executablePath: electronModule,
    args: [resolve('app'), `--talking-quill-user-data=${profile}`, '--talking-quill-task6-test'],
    env: { ...process.env, NODE_ENV: 'test' },
  });
  try {
    const { main, widget } = await rendererPages(application);
    await main.getByRole('button', { name: 'Settings' }).click();

    // Safe live gesture test: the same helper event route is used, while capture stays untouched.
    await main.getByRole('button', { name: 'Test activation shortcut' }).click();
    await driverCall(application, 'activationDown');
    await driverCall(application, 'activationUp');
    await expect(main.getByText('Quick Dictation gesture recognized')).toBeVisible();
    await driverCall(application, 'activationDown', [true]);
    await main.waitForTimeout(650);
    await driverCall(application, 'activationUp', [true]);
    await expect(main.getByText('Extended Dictation gesture recognized')).toBeVisible();
    expect((await snapshot(application)).recording.starts).toBe(0);
    await main.getByRole('button', { name: 'Stop shortcut test' }).click();

    // Quick trailing-silence submit. Actual injected PCM/RMS reaches the live widget meter.
    await beginQuick(application, 1);
    const widgetMeter = widget.getByRole('meter', { name: 'Microphone level' });
    await expect
      .poll(async () => Number(await widgetMeter.getAttribute('aria-valuenow')))
      .toBeGreaterThan(0);
    await expect(widgetMeter).toHaveAttribute('aria-valuetext', /percent, (?:speaking|loud)/i);
    await driverCall(application, 'frames', [0.001, 90]);
    await waitForPhase(application, 'completed');
    expect((await snapshot(application)).insertion.targetText).toBe('deterministic transcript');
    await resetSession(application);
    await expect(widgetMeter).toHaveAttribute('aria-valuenow', '0');
    await expect(widgetMeter).toHaveAttribute('aria-valuetext', '0 percent, silent');

    // Quick Enter submit.
    await beginQuick(application, 2);
    await driverCall(application, 'key', ['enter']);
    await waitForPhase(application, 'completed');
    await resetSession(application);

    // Quick Esc cancellation.
    const beforeEscape = (await snapshot(application)).insertion.calls;
    await beginQuick(application, 3);
    await driverCall(application, 'key', ['escape']);
    await waitForPhase(application, 'cancelled');
    expect((await snapshot(application)).insertion.calls).toBe(beforeEscape);
    await resetSession(application);

    // Quick shortcut submit.
    await beginQuick(application, 4);
    await driverCall(application, 'activationDown');
    await waitForPhase(application, 'completed');
    await driverCall(application, 'activationUp');
    await resetSession(application);

    // Extended pointer Stop keeps the target window's keyboard focus throughout.
    await driverCall(application, 'setTranscript', ['pointer-preserved target']);
    await beginExtended(application, 5);
    const widgetGeometry = await application.evaluate(({ BrowserWindow, screen }) => {
      const window = BrowserWindow.getAllWindows().find(
        (candidate) => candidate.getTitle() === 'Talking Quill Widget',
      );
      if (window === undefined) throw new Error('Widget window unavailable');
      const bounds = window.getBounds();
      const display = screen.getDisplayMatching(bounds);
      return { bounds, workArea: display.workArea, focusable: window.isFocusable() };
    });
    expect(widgetGeometry).toMatchObject({
      bounds: { width: 360, height: 96 },
      focusable: false,
    });
    expect(widgetGeometry.bounds.x).toBeGreaterThanOrEqual(widgetGeometry.workArea.x);
    expect(widgetGeometry.bounds.y).toBeGreaterThanOrEqual(widgetGeometry.workArea.y);
    const stop = widget.getByRole('button', { name: 'Stop Extended Dictation' });
    await expect(stop).toHaveAccessibleDescription(/global Enter to submit, Escape to cancel/i);
    await stop.hover();
    await expect
      .poll(() =>
        application.evaluate(({ BrowserWindow }) =>
          BrowserWindow.getAllWindows()
            .find((window) => window.getTitle() === 'Talking Quill Widget')
            ?.isFocusable(),
        ),
      )
      .toBe(false);
    await stop.click();
    await waitForPhase(application, 'completed');
    expect((await snapshot(application)).insertion.targetText).toBe('pointer-preserved target');
    await expect
      .poll(() =>
        application.evaluate(({ BrowserWindow }) =>
          BrowserWindow.getAllWindows()
            .find((window) => window.getTitle() === 'Talking Quill Widget')
            ?.isFocusable(),
        ),
      )
      .toBe(false);
    await resetSession(application);

    // Extended Esc and widget Cancel.
    await beginExtended(application, 6);
    await driverCall(application, 'key', ['escape']);
    await waitForPhase(application, 'cancelled');
    await resetSession(application);
    await beginExtended(application, 7);
    const cancel = widget.getByRole('button', { name: 'Cancel dictation' });
    await cancel.hover();
    await cancel.click();
    await waitForPhase(application, 'cancelled');
    await resetSession(application);

    // Smart-unavailable raw fallback plus copied-to-clipboard result.
    await main.getByRole('combobox', { name: 'Default processing mode' }).selectOption('smart');
    await expect(main.getByText('Default mode saved.')).toBeVisible();
    await driverCall(application, 'setCopied', [true]);
    await driverCall(application, 'setTranscript', ['raw fallback text']);
    await beginQuick(application, 8);
    await driverCall(application, 'key', ['enter']);
    await waitForPhase(application, 'completed');
    expect((await snapshot(application)).session).toMatchObject({
      completion: 'copied',
      abortReason: 'provider-error',
    });
    expect((await snapshot(application)).insertion.targetText).toBe('raw fallback text');
    await resetSession(application);

    // Voice Commands run immediately after transcription, bypass unavailable Smart processing,
    // insert the snippet, and persist a typed history outcome.
    await driverCall(application, 'setCopied', [false]);
    await main.getByLabel(/Trigger phrase/).fill('archive this project');
    await main.getByLabel('Snippet').fill('Project archived successfully');
    await main.getByRole('button', { name: 'Add voice command' }).click();
    await expect(main.getByText(/Say “archive this project”/)).toBeVisible();
    await driverCall(application, 'setTranscript', ['Archive this project!']);
    await beginQuick(application, 9);
    await driverCall(application, 'key', ['enter']);
    await waitForPhase(application, 'completed');
    expect((await snapshot(application)).insertion.targetText).toBe(
      'Project archived successfully',
    );
    await resetSession(application);

    const final = await snapshot(application);
    expect(final.recording).toMatchObject({ starts: 9, active: false });
    expect(final.recording.stops).toBeGreaterThanOrEqual(final.recording.starts);
    expect(final.history.find((entry) => entry.outcome === 'voice-command')).toMatchObject({
      rawText: 'Archive this project!',
      processingMode: 'smart',
      voiceTrigger: 'archive this project',
      voiceSnippet: 'Project archived successfully',
    });
  } finally {
    await application.close();
  }
  expect(await readdir(resolve(profile, 'tmp', 'sessions'))).toEqual([]);
});
