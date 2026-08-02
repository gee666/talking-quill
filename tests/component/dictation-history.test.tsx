// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MainApi } from '../../app/src/shared/bridge/api';
import type { HistoryListItem } from '../../app/src/shared/schemas/history';
import { DictationHistory } from '../../app/src/renderer/main/history/DictationHistory';

const item: HistoryListItem = {
  id: '11111111-1111-4111-8111-111111111111',
  createdAt: 1_000,
  dictationMode: 'quick',
  processingMode: 'raw',
  outcome: 'raw-completed',
  rawText: 'A locally stored transcript.',
  processedText: null,
  providerId: null,
  modelId: null,
  fellBack: false,
  errorCategory: null,
  voiceTrigger: null,
  voiceSnippet: null,
  hasScreenshot: false,
};
const list = vi.fn<MainApi['history']['list']>();
const remove = vi.fn<MainApi['history']['delete']>();
const removeAll = vi.fn<MainApi['history']['deleteAll']>();
const copy = vi.fn<MainApi['history']['copy']>();
const thumbnail = vi.fn<MainApi['history']['thumbnail']>();
let changed: ((revision: number) => void) | null = null;

beforeEach(() => {
  list.mockReset();
  list.mockResolvedValue({ items: [item], nextCursor: null, revision: 0 });
  remove.mockReset();
  remove.mockResolvedValue({ deleted: true, revision: 1, screenshotCleanup: 'complete' });
  removeAll.mockReset();
  removeAll.mockResolvedValue({
    deletedCount: 1,
    revision: 1,
    screenshotCleanup: 'complete',
  });
  copy.mockReset();
  copy.mockResolvedValue();
  thumbnail.mockReset();
  thumbnail.mockResolvedValue(null);
  changed = null;
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: {
      history: {
        list,
        delete: remove,
        deleteAll: removeAll,
        copy,
        thumbnail,
        onChanged: (listener: (revision: number) => void) => {
          changed = listener;
          return () => {
            changed = null;
          };
        },
      },
    },
  });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('Dictation history', () => {
  it('subscribes immediately and defers its initial IPC until the renderer is idle', async () => {
    let runIdle!: IdleRequestCallback;
    const requestIdleCallback = vi.fn((callback: IdleRequestCallback) => {
      runIdle = callback;
      return 1;
    });
    vi.stubGlobal('requestIdleCallback', requestIdleCallback);
    vi.stubGlobal('cancelIdleCallback', vi.fn());

    render(<DictationHistory />);

    expect(changed).not.toBeNull();
    expect(list).not.toHaveBeenCalled();
    act(() => runIdle({ didTimeout: false, timeRemaining: () => 50 }));
    expect(await screen.findByText('A locally stored transcript.')).toBeVisible();
    expect(list).toHaveBeenCalledOnce();
  });

  it('cancels its deferred initial load and subscription on unmount', () => {
    let runIdle!: IdleRequestCallback;
    const cancelIdleCallback = vi.fn();
    vi.stubGlobal(
      'requestIdleCallback',
      vi.fn((callback: IdleRequestCallback) => {
        runIdle = callback;
        return 1;
      }),
    );
    vi.stubGlobal('cancelIdleCallback', cancelIdleCallback);

    const { unmount } = render(<DictationHistory />);
    unmount();

    expect(cancelIdleCallback).toHaveBeenCalledWith(1);
    expect(changed).toBeNull();
    act(() => runIdle({ didTimeout: false, timeRemaining: () => 50 }));
    expect(list).not.toHaveBeenCalled();
  });

  it('does not run a canceled initial load after an earlier change event', async () => {
    let runIdle!: IdleRequestCallback;
    const cancelIdleCallback = vi.fn();
    vi.stubGlobal(
      'requestIdleCallback',
      vi.fn((callback: IdleRequestCallback) => {
        runIdle = callback;
        return 1;
      }),
    );
    vi.stubGlobal('cancelIdleCallback', cancelIdleCallback);

    render(<DictationHistory />);
    act(() => changed?.(1));

    expect(await screen.findByText('A locally stored transcript.')).toBeVisible();
    expect(cancelIdleCallback).toHaveBeenCalledWith(1);
    act(() => runIdle({ didTimeout: false, timeRemaining: () => 50 }));
    expect(list).toHaveBeenCalledOnce();
  });

  it('loads screenshot thumbnails only when their entries approach the viewport', async () => {
    const idleCallbacks: IdleRequestCallback[] = [];
    const cancelIdleCallback = vi.fn();
    let intersectionCallback!: IntersectionObserverCallback;
    const observe = vi.fn();
    const disconnect = vi.fn();
    vi.stubGlobal(
      'requestIdleCallback',
      vi.fn((callback: IdleRequestCallback) => {
        idleCallbacks.push(callback);
        return idleCallbacks.length;
      }),
    );
    vi.stubGlobal('cancelIdleCallback', cancelIdleCallback);
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(callback: IntersectionObserverCallback) {
          intersectionCallback = callback;
        }
        observe = observe;
        disconnect = disconnect;
      },
    );
    list.mockResolvedValueOnce({
      items: [{ ...item, hasScreenshot: true }],
      nextCursor: null,
      revision: 0,
    });

    render(<DictationHistory />);
    act(() => idleCallbacks[0]?.({ didTimeout: false, timeRemaining: () => 50 }));
    expect(await screen.findByText('A locally stored transcript.')).toBeVisible();
    await waitFor(() => expect(idleCallbacks).toHaveLength(2));
    expect(observe).toHaveBeenCalledOnce();
    expect(thumbnail).not.toHaveBeenCalled();

    act(() => {
      const entries = [{ isIntersecting: true } as IntersectionObserverEntry];
      intersectionCallback(entries, {} as IntersectionObserver);
      intersectionCallback(entries, {} as IntersectionObserver);
      idleCallbacks[1]?.({ didTimeout: false, timeRemaining: () => 50 });
    });
    await waitFor(() => expect(thumbnail).toHaveBeenCalledWith(item.id));
    expect(thumbnail).toHaveBeenCalledOnce();
    expect(cancelIdleCallback).toHaveBeenCalledWith(2);
    expect(disconnect).toHaveBeenCalledOnce();
  });

  it('cancels pending offscreen thumbnail work on unmount', async () => {
    const idleCallbacks: IdleRequestCallback[] = [];
    const cancelIdleCallback = vi.fn();
    let intersectionCallback!: IntersectionObserverCallback;
    const disconnect = vi.fn();
    vi.stubGlobal(
      'requestIdleCallback',
      vi.fn((callback: IdleRequestCallback) => {
        idleCallbacks.push(callback);
        return idleCallbacks.length;
      }),
    );
    vi.stubGlobal('cancelIdleCallback', cancelIdleCallback);
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(callback: IntersectionObserverCallback) {
          intersectionCallback = callback;
        }
        observe = vi.fn();
        disconnect = disconnect;
      },
    );
    list.mockResolvedValueOnce({
      items: [{ ...item, hasScreenshot: true }],
      nextCursor: null,
      revision: 0,
    });

    const { unmount } = render(<DictationHistory />);
    act(() => idleCallbacks[0]?.({ didTimeout: false, timeRemaining: () => 50 }));
    expect(await screen.findByText('A locally stored transcript.')).toBeVisible();
    await waitFor(() => expect(idleCallbacks).toHaveLength(2));

    unmount();
    expect(cancelIdleCallback).toHaveBeenCalledWith(2);
    expect(disconnect).toHaveBeenCalledOnce();
    act(() => {
      idleCallbacks[1]?.({ didTimeout: false, timeRemaining: () => 50 });
      intersectionCallback(
        [{ isIntersecting: true } as IntersectionObserverEntry],
        {} as IntersectionObserver,
      );
    });
    expect(thumbnail).not.toHaveBeenCalled();
  });

  it('loads offscreen thumbnails during deferred time without duplicates and cleans up', async () => {
    const idleCallbacks: IdleRequestCallback[] = [];
    let intersectionCallback!: IntersectionObserverCallback;
    const disconnect = vi.fn();
    const createObjectURL = vi.fn(() => 'blob:history-thumbnail');
    const revokeObjectURL = vi.fn();
    vi.stubGlobal(
      'requestIdleCallback',
      vi.fn((callback: IdleRequestCallback) => {
        idleCallbacks.push(callback);
        return idleCallbacks.length;
      }),
    );
    vi.stubGlobal('cancelIdleCallback', vi.fn());
    vi.stubGlobal('URL', { createObjectURL, revokeObjectURL });
    vi.stubGlobal(
      'IntersectionObserver',
      class {
        constructor(callback: IntersectionObserverCallback) {
          intersectionCallback = callback;
        }
        observe = vi.fn();
        disconnect = disconnect;
      },
    );
    list.mockResolvedValueOnce({
      items: [{ ...item, hasScreenshot: true }],
      nextCursor: null,
      revision: 0,
    });
    thumbnail.mockResolvedValueOnce('YQ==');

    const { unmount } = render(<DictationHistory />);
    act(() => idleCallbacks[0]?.({ didTimeout: false, timeRemaining: () => 50 }));
    expect(await screen.findByText('A locally stored transcript.')).toBeVisible();
    await waitFor(() => expect(idleCallbacks).toHaveLength(2));

    act(() => idleCallbacks[1]?.({ didTimeout: false, timeRemaining: () => 50 }));
    expect(
      await screen.findByRole('img', { name: 'On-Screen Awareness context thumbnail' }),
    ).toHaveAttribute('src', 'blob:history-thumbnail');
    act(() => {
      intersectionCallback(
        [{ isIntersecting: true } as IntersectionObserverEntry],
        {} as IntersectionObserver,
      );
      idleCallbacks[1]?.({ didTimeout: false, timeRemaining: () => 50 });
    });
    expect(thumbnail).toHaveBeenCalledOnce();
    expect(disconnect).toHaveBeenCalledOnce();

    unmount();
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:history-thumbnail');
  });

  it('keeps unchanged thumbnails mounted across history refreshes', async () => {
    const createObjectURL = vi.fn(() => 'blob:history-thumbnail');
    const revokeObjectURL = vi.fn();
    vi.stubGlobal('URL', { createObjectURL, revokeObjectURL });
    list.mockResolvedValue({
      items: [{ ...item, hasScreenshot: true }],
      nextCursor: null,
      revision: 0,
    });
    thumbnail.mockResolvedValue('YQ==');

    const { unmount } = render(<DictationHistory />);
    expect(
      await screen.findByRole('img', { name: 'On-Screen Awareness context thumbnail' }),
    ).toBeVisible();

    act(() => changed?.(1));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    expect(
      screen.getByRole('img', { name: 'On-Screen Awareness context thumbnail' }),
    ).toBeVisible();
    expect(thumbnail).toHaveBeenCalledOnce();
    expect(revokeObjectURL).not.toHaveBeenCalled();

    unmount();
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:history-thumbnail');
  });

  it('loads, copies, deletes, and refreshes after a change event', async () => {
    const user = userEvent.setup();
    render(<DictationHistory />);
    expect(await screen.findByText('A locally stored transcript.')).toBeVisible();
    await user.click(
      screen.getByRole('button', { name: 'Copy transcript: A locally stored transcript.' }),
    );
    expect(copy).toHaveBeenCalledWith(item.id);
    expect(await screen.findByText('Copied to your clipboard.')).toBeVisible();
    await user.click(
      screen.getByRole('button', { name: 'Delete transcript: A locally stored transcript.' }),
    );
    expect(remove).toHaveBeenCalledWith(item.id);
    changed?.(1);
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  });

  it('separates the dictation and processing mode chips for both sighted and screen-reader users', async () => {
    list.mockResolvedValueOnce({
      items: [
        item,
        {
          ...item,
          id: '33333333-3333-4333-8333-333333333333',
          dictationMode: 'extended',
          processingMode: 'smart',
          outcome: 'smart-completed',
          processedText: 'Second polished transcript.',
        },
      ],
      nextCursor: null,
      revision: 0,
    });
    const { container } = render(<DictationHistory />);

    await screen.findByText('A locally stored transcript.');
    const chips = [...container.querySelectorAll('.history-entry__chips')];
    expect(chips).toHaveLength(2);
    expect(chips.map((chip) => chip.textContent)).toEqual(['Quick · Raw', 'Extended · Smart']);
    // The separator is decorative, so the accessible text still needs its own spacing.
    const accessible = chips.map((chip) =>
      [...chip.childNodes]
        .filter(
          (node) => !(node instanceof HTMLElement && node.getAttribute('aria-hidden') === 'true'),
        )
        .map((node) => node.textContent ?? '')
        .join('')
        .replace(/\s+/gu, ' ')
        .trim(),
    );
    expect(accessible).toEqual(['Quick Raw', 'Extended Smart']);
  });

  it('displays accessible Smart provider, model, and explicit fallback metadata', async () => {
    list.mockResolvedValueOnce({
      items: [
        {
          ...item,
          processingMode: 'smart',
          outcome: 'smart-fallback',
          providerId: 'pi',
          modelId: 'anthropic/claude-test',
          fellBack: true,
          errorCategory: 'pi-authentication-failed',
        },
      ],
      nextCursor: null,
      revision: 0,
    });
    render(<DictationHistory />);
    expect(await screen.findByLabelText('Smart provider and model')).toHaveTextContent(
      'Provider: pi · Model: anthropic/claude-test',
    );
    expect(screen.getByRole('status')).toHaveTextContent(
      'The AI service could not sign in, so your words were inserted exactly as you said them.',
    );
  });

  it('displays the copyable outcome text and gives repeated controls entry-specific names', async () => {
    const second: HistoryListItem = {
      ...item,
      id: '22222222-2222-4222-8222-222222222222',
      rawText: 'Second raw transcript.',
      processedText: 'Second polished transcript.',
      processingMode: 'smart',
      outcome: 'smart-completed',
    };
    list.mockResolvedValueOnce({
      items: [{ ...item, processedText: 'Polished text must not replace raw.' }, second],
      nextCursor: null,
      revision: 0,
    });
    render(<DictationHistory />);

    expect(await screen.findByText('Polished text must not replace raw.')).toBeVisible();
    expect(screen.getByText('Second polished transcript.')).toBeVisible();
    expect(screen.queryByText('A locally stored transcript.')).not.toBeInTheDocument();
    expect(screen.queryByText('Second raw transcript.')).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Copy transcript: Polished text must not replace raw.' }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Copy transcript: Second polished transcript.' }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', {
        name: 'Delete transcript: Polished text must not replace raw.',
      }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Delete transcript: Second polished transcript.' }),
    ).toBeVisible();
  });

  it('requires confirmation before deleting all', async () => {
    const user = userEvent.setup();
    render(<DictationHistory />);
    await screen.findByText('A locally stored transcript.');
    await user.click(screen.getByRole('button', { name: 'Delete all history' }));
    expect(screen.getByRole('dialog', { name: 'Delete all history?' })).toBeVisible();
    expect(removeAll).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: /^Delete all$/ }));
    expect(removeAll).toHaveBeenCalledOnce();
  });

  it('truthfully warns when screenshot cleanup remains pending after deletion', async () => {
    remove.mockResolvedValueOnce({ deleted: true, revision: 1, screenshotCleanup: 'pending' });
    const user = userEvent.setup();
    render(<DictationHistory />);
    await user.click(
      await screen.findByRole('button', {
        name: 'Delete transcript: A locally stored transcript.',
      }),
    );
    expect(
      await screen.findByText(
        'Deleted. The screenshot is taking a moment to clear and will go shortly.',
      ),
    ).toBeVisible();
    expect(screen.queryByText('That entry could not be deleted.')).not.toBeInTheDocument();
  });

  it('warns truthfully and closes confirmation after partial Delete All cleanup', async () => {
    removeAll.mockResolvedValueOnce({
      deletedCount: 1,
      revision: 1,
      screenshotCleanup: 'partial',
    });
    const user = userEvent.setup();
    render(<DictationHistory />);
    await screen.findByText('A locally stored transcript.');
    await user.click(screen.getByRole('button', { name: 'Delete all history' }));
    await user.click(screen.getByRole('button', { name: /^Delete all$/ }));
    expect(
      await screen.findByText(
        'Your dictation history is now empty. The screenshots are taking a moment to clear and will go shortly.',
      ),
    ).toBeVisible();
    expect(screen.queryByRole('dialog', { name: 'Delete all history?' })).not.toBeInTheDocument();
  });

  it('renders empty and bounded public-error states', async () => {
    list.mockResolvedValueOnce({ items: [], nextCursor: null, revision: 0 });
    const { unmount } = render(<DictationHistory />);
    expect(await screen.findByText('Nothing here yet')).toBeVisible();
    unmount();
    list.mockRejectedValueOnce(new Error('private database path'));
    render(<DictationHistory />);
    expect(await screen.findByText('History could not be loaded')).toBeVisible();
    expect(screen.getByText('Your dictation history could not be loaded.')).toBeVisible();
    expect(screen.queryByText(/private database path/i)).not.toBeInTheDocument();
  });
});
