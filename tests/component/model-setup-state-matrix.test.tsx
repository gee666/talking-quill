// @vitest-environment jsdom
/* eslint-disable @typescript-eslint/require-await -- async mocks model the preload Promise API. */
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { StrictMode } from 'react';
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

  it('prevents a cancel action from overlapping a pending pause', async () => {
    const downloading = {
      modelId: DEFAULT_SETTINGS.transcription.modelId,
      state: 'downloading' as const,
      downloadedBytes: 5,
      totalBytes: 10,
      detail: null,
      repairable: false,
    };
    const paused = { ...downloading, state: 'paused' as const };
    let resolvePause!: (status: typeof paused) => void;
    const pause = vi.fn(
      () =>
        new Promise<typeof paused>((resolve) => {
          resolvePause = resolve;
        }),
    );
    const cancel = vi.fn();
    Object.defineProperty(window, 'talkingQuill', {
      configurable: true,
      value: {
        models: {
          status: vi.fn(async () => downloading),
          onProgress: vi.fn(() => () => undefined),
          download: vi.fn(),
          pause,
          cancel,
          retry: vi.fn(),
          delete: vi.fn(),
        },
        settings: { update: vi.fn() },
      },
    });

    render(<ModelSetup settings={structuredClone(DEFAULT_SETTINGS)} onSettingsSaved={vi.fn()} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Pause download' }));
    expect(screen.getByRole('button', { name: 'Cancel download' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel download' }));
    expect(cancel).not.toHaveBeenCalled();
    await act(async () => resolvePause(paused));
    expect(await screen.findByText('Download paused')).toBeVisible();
  });

  it('continues applying model actions after the StrictMode effect replay', async () => {
    const status = {
      modelId: DEFAULT_SETTINGS.transcription.modelId,
      state: 'missing' as const,
      downloadedBytes: 0,
      totalBytes: 10,
      detail: null,
      repairable: false,
    };
    const ready = { ...status, state: 'ready' as const, downloadedBytes: 10 };
    const download = vi.fn(async () => ready);
    Object.defineProperty(window, 'talkingQuill', {
      configurable: true,
      value: {
        models: {
          status: vi.fn(async () => status),
          onProgress: vi.fn(() => () => undefined),
          download,
          pause: vi.fn(),
          cancel: vi.fn(),
          retry: vi.fn(),
          delete: vi.fn(),
        },
        settings: { update: vi.fn() },
      },
    });

    render(
      <StrictMode>
        <ModelSetup settings={structuredClone(DEFAULT_SETTINGS)} onSettingsSaved={vi.fn()} />
      </StrictMode>,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Download model' }));

    expect(await screen.findByText('Model ready for offline transcription')).toBeVisible();
    expect(download).toHaveBeenCalledOnce();
  });

  it('shows the safe phase-specific status detail instead of generic connection advice', async () => {
    const status = vi
      .fn()
      .mockResolvedValueOnce({
        modelId: DEFAULT_SETTINGS.transcription.modelId,
        state: 'missing',
        downloadedBytes: 0,
        totalBytes: 10,
        detail: null,
        repairable: false,
      })
      .mockResolvedValueOnce({
        modelId: DEFAULT_SETTINGS.transcription.modelId,
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
