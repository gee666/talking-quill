import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { _electron as electron, expect, test } from '@playwright/test';
import { SETTINGS_SCHEMA_VERSION } from '../../app/src/shared/schemas/settings';
import { rendererPages, resetProfile } from './helpers';

const electronModule: unknown = createRequire(resolve('package.json'))('electron');
const electronExecutable = readElectronExecutable(electronModule);
const fakeAudioFile = resolve('tests/fixtures/fake-microphone.wav');

function readElectronExecutable(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Electron executable is unavailable');
  return value;
}

async function launch(profile: string, useFakePermissionUi = true) {
  return electron.launch({
    executablePath: electronExecutable,
    args: [
      resolve('app'),
      `--talking-quill-user-data=${profile}`,
      '--use-fake-device-for-media-stream',
      ...(useFakePermissionUi ? ['--use-fake-ui-for-media-stream'] : []),
      `--use-file-for-fake-audio-capture=${fakeAudioFile}`,
    ],
    env: { ...process.env, NODE_ENV: 'test' },
  });
}

interface ReturnedStreamSummary {
  readonly streamCount: number;
  readonly tracks: readonly (readonly MediaStreamTrackState[])[];
}

async function returnedStreamSummary(
  capture: Awaited<ReturnType<typeof rendererPages>>['capture'],
): Promise<ReturnedStreamSummary> {
  return capture.evaluate(() => {
    const value: unknown = Reflect.get(globalThis, '__talkingQuillReturnedStreams');
    const streams = Array.isArray(value) ? (value as MediaStream[]) : [];
    return {
      streamCount: streams.length,
      tracks: streams.map((stream) => stream.getTracks().map((track) => track.readyState)),
    };
  });
}

async function expectEveryReturnedTrackEnded(
  capture: Awaited<ReturnType<typeof rendererPages>>['capture'],
  minimumStreams: number,
): Promise<void> {
  await expect
    .poll(async () => {
      const summary = await returnedStreamSummary(capture);
      return (
        summary.streamCount >= minimumStreams &&
        summary.tracks.length === summary.streamCount &&
        summary.tracks.every(
          (tracks) => tracks.length > 0 && tracks.every((state) => state === 'ended'),
        )
      );
    })
    .toBe(true);
}

async function fakeAudioRms(): Promise<number> {
  const wav = await readFile(fakeAudioFile);
  expect(wav.subarray(0, 4).toString('ascii')).toBe('RIFF');
  expect(wav.subarray(8, 12).toString('ascii')).toBe('WAVE');
  expect(wav.readUInt16LE(20)).toBe(1);
  expect(wav.readUInt16LE(22)).toBe(2);
  expect(wav.readUInt32LE(24)).toBe(44_100);
  expect(wav.readUInt16LE(34)).toBe(16);
  const dataLength = wav.readUInt32LE(40);
  expect(dataLength).toBe(wav.length - 44);
  let sumSquares = 0;
  const sampleCount = dataLength / 2;
  for (let offset = 44; offset < 44 + dataLength; offset += 2) {
    const sample = wav.readInt16LE(offset) / 32_768;
    sumSquares += sample * sample;
  }
  return Math.sqrt(sumSquares / sampleCount);
}

test('real Chromium fake audio reaches the worklet meter and releases every track', async () => {
  test.setTimeout(90_000);
  const profile = await resetProfile('source-audio');
  expect(await fakeAudioRms()).toBeGreaterThan(0.05);
  const application = await launch(profile, false);
  try {
    expect(
      await application.evaluate(({ app }) =>
        app.commandLine.getSwitchValue('use-file-for-fake-audio-capture'),
      ),
    ).toBe(fakeAudioFile);
    const { main, capture } = await rendererPages(application);
    await capture.addInitScript(() => {
      const streams: MediaStream[] = [];
      Reflect.set(globalThis, '__talkingQuillReturnedStreams', streams);
      const mediaDevices = navigator.mediaDevices;
      const deviceChangeListeners = new Set<EventListenerOrEventListenerObject>();
      const addEventListener = mediaDevices.addEventListener.bind(mediaDevices);
      const removeEventListener = mediaDevices.removeEventListener.bind(mediaDevices);
      Object.defineProperty(mediaDevices, 'addEventListener', {
        configurable: true,
        value(
          type: string,
          listener: EventListenerOrEventListenerObject | null,
          options?: boolean | AddEventListenerOptions,
        ) {
          if (listener === null) return;
          if (type === 'devicechange') deviceChangeListeners.add(listener);
          addEventListener(type, listener, options);
        },
      });
      Object.defineProperty(mediaDevices, 'removeEventListener', {
        configurable: true,
        value(
          type: string,
          listener: EventListenerOrEventListenerObject | null,
          options?: boolean | EventListenerOptions,
        ) {
          if (listener === null) return;
          if (type === 'devicechange') deviceChangeListeners.delete(listener);
          removeEventListener(type, listener, options);
        },
      });
      Reflect.set(globalThis, '__talkingQuillDeviceChangeListeners', deviceChangeListeners);
      const enumerateDevices = mediaDevices.enumerateDevices.bind(mediaDevices);
      Object.defineProperty(mediaDevices, 'enumerateDevices', {
        configurable: true,
        value: async () =>
          (await enumerateDevices()).map((device) =>
            device.kind === 'audioinput' && device.deviceId.length > 0
              ? {
                  deviceId: device.deviceId,
                  groupId: device.groupId,
                  kind: device.kind,
                  label: 'Bluetooth Hands-Free Microphone',
                  toJSON: () => ({}),
                }
              : device,
          ),
      });
      const getUserMedia = mediaDevices.getUserMedia.bind(mediaDevices);
      Object.defineProperty(mediaDevices, 'getUserMedia', {
        configurable: true,
        value: async (constraints: MediaStreamConstraints) => {
          const stream = await getUserMedia(constraints);
          streams.push(stream);
          return stream;
        },
      });
    });
    await capture.reload();
    await expect
      .poll(() => capture.evaluate(() => document.documentElement.dataset.ready))
      .toBe('true');
    await capture.evaluate(() => window.talkingQuillCapture.ready());
    await expect
      .poll(() =>
        capture.evaluate(() => {
          const listeners: unknown = Reflect.get(globalThis, '__talkingQuillDeviceChangeListeners');
          return listeners instanceof Set ? listeners.size : -1;
        }),
      )
      .toBe(1);

    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Recording' }).click();
    await main.getByRole('button', { name: 'Test microphone' }).click();
    await expect(main.getByText('Microphone active')).toBeVisible({ timeout: 15_000 });
    await expect
      .poll(
        async () =>
          Number(
            await main.getByRole('progressbar', { name: 'Microphone level' }).getAttribute('value'),
          ),
        { timeout: 15_000 },
      )
      .toBeGreaterThan(0.01);

    await main.reload();
    await expect(main.getByRole('button', { name: 'Settings' })).toBeVisible();
    await expectEveryReturnedTrackEnded(capture, 1);
    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Recording' }).click();
    const picker = main.getByRole('combobox', { name: 'Microphone' });
    await expect(picker).toBeEnabled();
    const fakeDeviceOption = picker.locator('option').last();
    await expect(fakeDeviceOption).toHaveText('Bluetooth Hands-Free Microphone');
    const fakeDeviceId = await fakeDeviceOption.getAttribute('value');
    if (fakeDeviceId === null || fakeDeviceId.length === 0) {
      throw new Error('Chromium fake microphone was not enumerated');
    }
    await picker.selectOption(fakeDeviceId);
    await expect(main.getByText('Preferred microphone saved.')).toBeVisible();
    await main.getByRole('button', { name: 'Test microphone' }).click();
    await expect(main.getByText('Microphone active')).toBeVisible({ timeout: 15_000 });
    await expect
      .poll(
        async () =>
          Number(
            await main.getByRole('progressbar', { name: 'Microphone level' }).getAttribute('value'),
          ),
        { timeout: 15_000 },
      )
      .toBeGreaterThan(0.01);

    await main.getByRole('button', { name: 'Stop microphone test' }).click();
    await expect(main.getByText('Test stopped')).toBeVisible();
    await expectEveryReturnedTrackEnded(capture, 2);

    await main.getByRole('combobox', { name: 'Silence detection' }).selectOption('relaxed');
    await expect(main.getByText('Silence detection preset saved.')).toBeVisible();
    const persisted = JSON.parse(await readFile(resolve(profile, 'settings.json'), 'utf8')) as {
      readonly schemaVersion: number;
      readonly recording: { readonly silencePreset: string };
    };
    expect(persisted).toMatchObject({
      schemaVersion: SETTINGS_SCHEMA_VERSION,
      recording: { silencePreset: 'relaxed' },
    });

    await main.getByRole('button', { name: 'Test microphone' }).click();
    await expect(main.getByText('Microphone active')).toBeVisible({ timeout: 15_000 });
    await main.getByRole('button', { name: 'Info' }).click();
    await expectEveryReturnedTrackEnded(capture, 3);
    const summary = await returnedStreamSummary(capture);
    expect(summary.tracks).toHaveLength(summary.streamCount);
    expect(summary.tracks.every((tracks) => tracks.every((state) => state === 'ended'))).toBe(true);
  } finally {
    await application.close();
  }
});

test('Electron policy denial does not masquerade as Windows privacy denial', async () => {
  const profile = await resetProfile('source-audio-denied');
  const application = await launch(profile, false);
  try {
    const { main } = await rendererPages(application);
    await application.evaluate(({ session }) => {
      const captureSession = session.fromPartition('persist:talking-quill-capture');
      captureSession.setPermissionCheckHandler(() => false);
      captureSession.setPermissionRequestHandler((_webContents, _permission, callback) =>
        callback(false),
      );
    });
    await main.getByRole('button', { name: 'Settings' }).click();
    await main.getByRole('button', { name: 'Recording' }).click();
    await main.getByRole('button', { name: 'Test microphone' }).click();
    await expect(main.getByText('Microphone unavailable')).toBeVisible();
    await expect(main.getByRole('alert')).toContainText(
      'could not complete Electron’s microphone authorization',
    );
    await expect(main.getByRole('button', { name: 'Open microphone settings' })).not.toBeVisible();
  } finally {
    await application.close();
  }
});
