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
  ['missing', 'Not downloaded yet'],
  ['checking', 'Checking the download'],
  ['downloading', 'Downloading'],
  ['verifying', 'Checking everything arrived'],
  ['installing', 'Almost ready'],
  ['paused', 'Download paused'],
  ['ready', 'Ready — works without internet'],
  ['corrupt', 'The download is damaged — download it again'],
  ['offline', 'No internet — reconnect to finish the download'],
  ['error', 'The download didn’t finish'],
];

describe('model setup state matrix', () => {
  it('shows each selected model’s exact id and size beneath its readable label', () => {
    Object.defineProperty(window, 'talkingQuill', {
      configurable: true,
      value: {
        models: {
          status: vi.fn(() => new Promise(() => undefined)),
          onProgress: vi.fn(() => () => undefined),
        },
        settings: { update: vi.fn() },
      },
    });
    const small = structuredClone(DEFAULT_SETTINGS);
    const view = render(<ModelSetup settings={small} onSettingsSaved={vi.fn()} />);

    expect(screen.getByRole('combobox', { name: 'Which model to use' })).toHaveValue(
      'Xenova/whisper-small',
    );
    expect(screen.getByText('Xenova/whisper-small (about 250 MB)')).toBeVisible();
    expect(
      screen.queryByText('onnx-community/whisper-large-v3-turbo (about 1.09 GB)'),
    ).not.toBeInTheDocument();

    const large = structuredClone(DEFAULT_SETTINGS);
    large.transcription.modelId = 'onnx-community/whisper-large-v3-turbo';
    view.rerender(<ModelSetup settings={large} onSettingsSaved={vi.fn()} />);

    expect(screen.getByRole('combobox', { name: 'Which model to use' })).toHaveValue(
      'onnx-community/whisper-large-v3-turbo',
    );
    expect(screen.getByText('onnx-community/whisper-large-v3-turbo (about 1.09 GB)')).toBeVisible();
    expect(screen.queryByText('Xenova/whisper-small (about 250 MB)')).not.toBeInTheDocument();
  });

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
      if (state === 'ready') expect(screen.getByRole('button', { name: 'Delete' })).toBeVisible();
      if (state === 'downloading') {
        expect(screen.getByRole('button', { name: 'Pause download' })).toBeVisible();
        expect(screen.getByRole('button', { name: 'Cancel download' })).toBeVisible();
      }
      if (state === 'verifying' || state === 'installing')
        expect(screen.getByRole('button', { name: 'Pause' })).toBeVisible();
      if (state === 'paused')
        expect(screen.getByRole('button', { name: 'Resume download' })).toBeVisible();
      if (state === 'missing' || state === 'checking')
        expect(screen.getByRole('button', { name: 'Download' })).toBeVisible();
      if (state === 'offline' || state === 'error' || state === 'corrupt')
        expect(screen.getByRole('button', { name: 'Try again' })).toBeVisible();
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
    fireEvent.click(await screen.findByRole('button', { name: 'Download' }));

    expect(await screen.findByText('Ready — works without internet')).toBeVisible();
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
    fireEvent.click(await screen.findByRole('button', { name: 'Download' }));
    expect(
      await screen.findAllByText(
        'Windows could not install the verified model because a file is temporarily locked. Retry to reuse the verified download.',
      ),
    ).toHaveLength(2);
    expect(screen.queryByText(/Check the connection and available disk space/i)).toBeNull();
  });
});
