// @vitest-environment jsdom
/* eslint-disable @typescript-eslint/require-await -- async mocks model the preload Promise API. */
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ModelSetup } from '../../app/src/renderer/main/setup/ModelSetup';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import type { ModelState } from '../../app/src/shared/schemas/transcription';

afterEach(cleanup);
const cases: readonly [ModelState, string][] = [
  ['missing', 'Model download required'],
  ['checking', 'Checking model integrity'],
  ['downloading', 'Downloading model'],
  ['verifying', 'Verifying downloaded model'],
  ['installing', 'Installing verified model'],
  ['paused', 'Download paused'],
  ['ready', 'Model ready for offline transcription'],
  ['corrupt', 'Model files are corrupt and need repair'],
  ['offline', 'Offline — reconnect to finish the download'],
  ['error', 'Model setup failed'],
];

describe('model setup state matrix', () => {
  it.each(cases)(
    'renders %s with explicit non-color text and an appropriate action',
    async (state, label) => {
      Object.defineProperty(window, 'talkingQuill', {
        configurable: true,
        value: {
          models: {
            status: vi.fn(async (modelId: string) => ({
              modelId,
              state,
              downloadedBytes: state === 'ready' ? 10 : 2,
              totalBytes: 10,
              detail: null,
              repairable: state === 'corrupt' || state === 'error',
            })),
            onProgress: vi.fn(() => () => undefined),
            download: vi.fn(),
            pause: vi.fn(),
            cancel: vi.fn(),
            retry: vi.fn(),
            delete: vi.fn(),
          },
          settings: { update: vi.fn() },
        },
      });
      render(<ModelSetup settings={structuredClone(DEFAULT_SETTINGS)} onSettingsSaved={vi.fn()} />);
      expect(await screen.findByText(label)).toBeVisible();
      if (state === 'ready')
        expect(screen.getByRole('button', { name: 'Delete model' })).toBeVisible();
      if (state === 'downloading')
        expect(screen.getByRole('button', { name: 'Pause download' })).toBeVisible();
      if (state === 'verifying' || state === 'installing')
        expect(screen.getByRole('button', { name: 'Pause model setup' })).toBeVisible();
      if (state === 'offline' || state === 'error' || state === 'corrupt')
        expect(screen.getByRole('button', { name: 'Retry download' })).toBeVisible();
    },
  );

  it('shows the safe phase-specific status detail instead of generic connection advice', async () => {
    const status = vi
      .fn()
      .mockResolvedValueOnce({
        modelId: 'onnx-community/whisper-large-v3-turbo',
        state: 'missing',
        downloadedBytes: 0,
        totalBytes: 10,
        detail: null,
        repairable: false,
      })
      .mockResolvedValueOnce({
        modelId: 'onnx-community/whisper-large-v3-turbo',
        state: 'error',
        downloadedBytes: 10,
        totalBytes: 10,
        detail:
          'Windows could not install the verified model because a file is temporarily locked. Retry to reuse the verified download.',
        repairable: true,
      });
    Object.defineProperty(window, 'talkingQuill', {
      configurable: true,
      value: {
        models: {
          status,
          onProgress: vi.fn(() => () => undefined),
          download: vi.fn().mockRejectedValue(new Error('The operation could not be completed.')),
          pause: vi.fn(),
          cancel: vi.fn(),
          retry: vi.fn(),
          delete: vi.fn(),
        },
        settings: { update: vi.fn() },
      },
    });
    render(<ModelSetup settings={structuredClone(DEFAULT_SETTINGS)} onSettingsSaved={vi.fn()} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Download model' }));
    expect(
      await screen.findAllByText(
        'Windows could not install the verified model because a file is temporarily locked. Retry to reuse the verified download.',
      ),
    ).toHaveLength(2);
    expect(screen.queryByText(/Check the connection and available disk space/i)).toBeNull();
  });
});
