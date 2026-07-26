// @vitest-environment jsdom

import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { VoiceCommandsSection } from '../../app/src/renderer/main/settings/VoiceCommandsSection';
import { CustomVocabularySection } from '../../app/src/renderer/main/settings/CustomVocabularySection';
import type { MainApi } from '../../app/src/shared/bridge/api';

const command = {
  id: '11111111-1111-4111-8111-111111111111',
  trigger: 'send report',
  snippet: 'Weekly report sent',
  createdAt: 1,
  updatedAt: 1,
};
const entry = {
  id: '22222222-2222-4222-8222-222222222222',
  value: 'GraphQL',
  createdAt: 1,
  updatedAt: 1,
};

function installApi() {
  const createCommand = vi.fn(() => Promise.resolve(command));
  const preview = vi.fn<MainApi['commands']['preview']>(() =>
    Promise.resolve({ command, kind: 'exact' as const, score: 1 }),
  );
  const deleteCommand = vi.fn(() => Promise.resolve(true));
  const createVocabulary = vi.fn(() => Promise.resolve(entry));
  const deleteVocabulary = vi.fn(() => Promise.resolve(true));
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: {
      commands: {
        list: () => Promise.resolve([command]),
        create: createCommand,
        update: () => Promise.resolve(command),
        delete: deleteCommand,
        preview,
      },
      vocabulary: {
        list: () => Promise.resolve([entry]),
        create: createVocabulary,
        update: () => Promise.resolve(entry),
        delete: deleteVocabulary,
        importFile: () => Promise.resolve({ status: 'imported' as const, count: 2 }),
        exportFile: () => Promise.resolve({ status: 'exported' as const, count: 1 }),
      },
    } satisfies Pick<MainApi, 'commands' | 'vocabulary'>,
  });
  return { createCommand, preview, deleteCommand, createVocabulary, deleteVocabulary };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('Task 8 settings', () => {
  it('supports keyboard command creation and a production matcher preview', async () => {
    const api = installApi();
    const user = userEvent.setup();
    render(<VoiceCommandsSection commands={[command]} />);
    expect(screen.getByText(/Say “send report”/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Edit send report' })).toBeVisible();
    await user.type(screen.getByLabelText(/Trigger phrase/), 'archive project');
    await user.type(screen.getByRole('textbox', { name: 'Snippet' }), 'Archived');
    await user.click(screen.getByRole('button', { name: 'Add voice command' }));
    expect(api.createCommand).toHaveBeenCalledWith({
      trigger: 'archive project',
      snippet: 'Archived',
    });
    await user.type(screen.getByLabelText('Match preview transcript'), 'send report');
    await user.click(screen.getByRole('button', { name: 'Preview match' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Exact match');
    expect(api.preview).toHaveBeenCalledWith('send report');
  });

  it('ignores a preview result after the transcript changes', async () => {
    const api = installApi();
    const pending = deferred<Awaited<ReturnType<MainApi['commands']['preview']>>>();
    api.preview.mockReturnValueOnce(pending.promise);
    const user = userEvent.setup();
    render(<VoiceCommandsSection commands={[command]} />);

    const transcript = screen.getByLabelText('Match preview transcript');
    await user.type(transcript, 'send report');
    await user.click(screen.getByRole('button', { name: 'Preview match' }));
    await user.clear(transcript);
    await user.type(transcript, 'archive project');
    pending.resolve(null);

    await waitForMicrotasks();
    expect(screen.queryByText('No voice command matches the full transcript.')).toBeNull();
  });

  it('announces preview and command delete IPC failures', async () => {
    const api = installApi();
    api.preview.mockRejectedValueOnce(new Error('Preview unavailable.'));
    api.deleteCommand.mockRejectedValueOnce(new Error('Delete unavailable.'));
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    const user = userEvent.setup();
    render(<VoiceCommandsSection commands={[command]} />);
    await user.type(screen.getByLabelText('Match preview transcript'), 'send report');
    await user.click(screen.getByRole('button', { name: 'Preview match' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Preview unavailable.');
    await user.click(screen.getByRole('button', { name: 'Delete send report' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Delete unavailable.');
  });

  it('states Smart-only scope and exposes accessible vocabulary CRUD and file actions', async () => {
    const api = installApi();
    const user = userEvent.setup();
    render(<CustomVocabularySection entries={[entry]} />);
    expect(screen.getByText(/applies to Smart Transcription only/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Edit GraphQL' })).toBeVisible();
    await user.type(screen.getByLabelText(/Word or phrase/), 'AnythingLLM');
    await user.click(screen.getByRole('button', { name: 'Add to vocabulary' }));
    expect(api.createVocabulary).toHaveBeenCalledWith('AnythingLLM');
    await user.click(screen.getByRole('button', { name: 'Import plain text' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Imported 2');
  });

  it('announces vocabulary delete IPC failures', async () => {
    const api = installApi();
    api.deleteVocabulary.mockRejectedValueOnce(new Error('Vocabulary delete unavailable.'));
    const user = userEvent.setup();
    render(<CustomVocabularySection entries={[entry]} />);
    await user.click(screen.getByRole('button', { name: 'Delete GraphQL' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Vocabulary delete unavailable.');
  });
});

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

async function waitForMicrotasks(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}
