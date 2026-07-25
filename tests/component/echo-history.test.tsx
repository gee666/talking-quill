// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MainApi } from '../../app/src/shared/bridge/api';
import type { HistoryListItem } from '../../app/src/shared/schemas/history';
import { PastEchoes } from '../../app/src/renderer/main/history/PastEchoes';

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
  changed = null;
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: {
      history: {
        list,
        delete: remove,
        deleteAll: removeAll,
        copy,
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
afterEach(cleanup);

describe('Past Echoes', () => {
  it('loads, copies, deletes, and refreshes after a change event', async () => {
    const user = userEvent.setup();
    render(<PastEchoes />);
    expect(await screen.findByText('A locally stored transcript.')).toBeVisible();
    await user.click(
      screen.getByRole('button', { name: 'Copy Echo: A locally stored transcript.' }),
    );
    expect(copy).toHaveBeenCalledWith(item.id);
    expect(await screen.findByText('Echo copied to the clipboard.')).toBeVisible();
    await user.click(
      screen.getByRole('button', { name: 'Delete Echo: A locally stored transcript.' }),
    );
    expect(remove).toHaveBeenCalledWith(item.id);
    changed?.(1);
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
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
    render(<PastEchoes />);
    expect(await screen.findByLabelText('Smart provider and model')).toHaveTextContent(
      'Provider: pi · Model: anthropic/claude-test',
    );
    expect(screen.getByRole('status')).toHaveTextContent(
      'Fell back to Raw because Pi authentication failed.',
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
    render(<PastEchoes />);

    expect(await screen.findByText('Polished text must not replace raw.')).toBeVisible();
    expect(screen.getByText('Second polished transcript.')).toBeVisible();
    expect(screen.queryByText('A locally stored transcript.')).not.toBeInTheDocument();
    expect(screen.queryByText('Second raw transcript.')).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Copy Echo: Polished text must not replace raw.' }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Copy Echo: Second polished transcript.' }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Delete Echo: Polished text must not replace raw.' }),
    ).toBeVisible();
    expect(
      screen.getByRole('button', { name: 'Delete Echo: Second polished transcript.' }),
    ).toBeVisible();
  });

  it('requires confirmation before deleting all', async () => {
    const user = userEvent.setup();
    render(<PastEchoes />);
    await screen.findByText('A locally stored transcript.');
    await user.click(screen.getByRole('button', { name: 'Delete all Past Echoes' }));
    expect(screen.getByRole('dialog', { name: 'Delete all Past Echoes?' })).toBeVisible();
    expect(removeAll).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: /^Delete all$/ }));
    expect(removeAll).toHaveBeenCalledOnce();
  });

  it('truthfully warns when screenshot cleanup remains pending after deletion', async () => {
    remove.mockResolvedValueOnce({ deleted: true, revision: 1, screenshotCleanup: 'pending' });
    const user = userEvent.setup();
    render(<PastEchoes />);
    await user.click(
      await screen.findByRole('button', { name: 'Delete Echo: A locally stored transcript.' }),
    );
    expect(
      await screen.findByText(
        'The Echo was deleted, but screenshot cleanup is incomplete and will be retried.',
      ),
    ).toBeVisible();
    expect(screen.queryByText('The Echo could not be deleted.')).not.toBeInTheDocument();
  });

  it('warns truthfully and closes confirmation after partial Delete All cleanup', async () => {
    removeAll.mockResolvedValueOnce({
      deletedCount: 1,
      revision: 1,
      screenshotCleanup: 'partial',
    });
    const user = userEvent.setup();
    render(<PastEchoes />);
    await screen.findByText('A locally stored transcript.');
    await user.click(screen.getByRole('button', { name: 'Delete all Past Echoes' }));
    await user.click(screen.getByRole('button', { name: /^Delete all$/ }));
    expect(
      await screen.findByText(
        'All Past Echoes were deleted, but screenshot cleanup is incomplete and will be retried.',
      ),
    ).toBeVisible();
    expect(
      screen.queryByRole('dialog', { name: 'Delete all Past Echoes?' }),
    ).not.toBeInTheDocument();
  });

  it('renders empty and bounded public-error states', async () => {
    list.mockResolvedValueOnce({ items: [], nextCursor: null, revision: 0 });
    const { unmount } = render(<PastEchoes />);
    expect(await screen.findByText('No Past Echoes yet')).toBeVisible();
    unmount();
    list.mockRejectedValueOnce(new Error('private database path'));
    render(<PastEchoes />);
    expect(await screen.findByText('Past Echoes could not be loaded.')).toBeVisible();
    expect(screen.queryByText(/private database path/i)).not.toBeInTheDocument();
  });
});
