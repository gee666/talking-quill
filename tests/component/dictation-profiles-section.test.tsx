// @vitest-environment jsdom
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DictationProfilesSection } from '../../app/src/renderer/main/settings/DictationProfilesSection';
import type { MainApi } from '../../app/src/shared/bridge/api';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  type CustomDictationProfileId,
  type DictationProfile,
} from '../../app/src/shared/schemas/dictation-profiles';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';

const CUSTOM_ID = '11111111-1111-4111-8111-111111111111' as CustomDictationProfileId;
const CUSTOM: DictationProfile = {
  id: CUSTOM_ID,
  name: 'Notes',
  activationKey: 'Q',
  shift: false,
  processingMode: 'raw',
  smartPrompt: null,
};

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function settingsWith(profiles: readonly DictationProfile[]): Settings {
  return {
    ...structuredClone(DEFAULT_SETTINGS),
    dictationProfiles: profiles.map((profile) => structuredClone(profile)),
  };
}

function renderProfiles(
  initial: Settings,
  profiles: MainApi['profiles'],
): ReturnType<typeof userEvent.setup> {
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: { profiles },
  });
  function Harness() {
    const [settings, setSettings] = useState(initial);
    return <DictationProfilesSection settings={settings} onSettingsSaved={setSettings} />;
  }
  render(<Harness />);
  return userEvent.setup();
}

function profileApi(overrides: Partial<MainApi['profiles']> = {}): MainApi['profiles'] {
  return {
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
    reset: vi.fn(),
    ...overrides,
  };
}

function group(name: string) {
  return screen.getByRole('group', { name });
}

describe('DictationProfilesSection', () => {
  it('creates a custom profile and renders the authoritative API result', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const created = { ...CUSTOM, name: 'Meeting notes', activationKey: 'A' as const };
    const create = vi.fn<MainApi['profiles']['create']>(() =>
      Promise.resolve(settingsWith([...initial.dictationProfiles, created])),
    );
    const user = renderProfiles(initial, profileApi({ create }));

    await user.click(screen.getByRole('button', { name: 'Add custom profile' }));
    await user.clear(screen.getByLabelText('New profile profile name'));
    await user.type(screen.getByLabelText('New profile profile name'), 'Meeting notes');
    await user.click(screen.getByRole('button', { name: 'Create profile' }));

    await waitFor(() => expect(create).toHaveBeenCalledOnce());
    expect(create).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'Meeting notes',
        activationKey: 'A',
        shift: false,
        processingMode: 'raw',
        smartPrompt: null,
      }),
    );
    expect(await screen.findByRole('group', { name: 'Meeting notes' })).toBeInTheDocument();
  });

  it('emits a complete key-and-Shift pair for every binding edit', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const movedGeneral = { ...DEFAULT_GENERAL_PROFILE, activationKey: 'Q' as const };
    const update = vi.fn<MainApi['profiles']['update']>(() =>
      Promise.resolve(settingsWith([movedGeneral, DEFAULT_PROMPT_PROFILE])),
    );
    const user = renderProfiles(initial, profileApi({ update }));

    await user.selectOptions(within(group('General')).getByLabelText('Activation key'), 'Q');
    await user.click(within(group('General')).getByRole('button', { name: 'Save profile' }));

    await waitFor(() =>
      expect(update).toHaveBeenCalledWith('general', { activationKey: 'Q', shift: false }),
    );
  });

  it('edits and then deletes a custom profile using each authoritative response', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE, CUSTOM]);
    const updated = { ...CUSTOM, name: 'Daily notes', processingMode: 'smart' as const };
    const update = vi.fn<MainApi['profiles']['update']>(() =>
      Promise.resolve(settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE, updated])),
    );
    const remove = vi.fn<MainApi['profiles']['delete']>(() =>
      Promise.resolve(settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE])),
    );
    const user = renderProfiles(initial, profileApi({ update, delete: remove }));

    await user.clear(within(group('Notes')).getByLabelText('Notes profile name'));
    await user.type(within(group('Notes')).getByLabelText('Notes profile name'), 'Daily notes');
    await user.selectOptions(
      within(group('Notes')).getByLabelText('Notes processing mode'),
      'smart',
    );
    await user.click(within(group('Notes')).getByRole('button', { name: 'Save profile' }));

    await waitFor(() =>
      expect(update).toHaveBeenCalledWith(CUSTOM_ID, {
        name: 'Daily notes',
        processingMode: 'smart',
      }),
    );
    const updatedGroup = await screen.findByRole('group', { name: 'Daily notes' });
    await user.click(within(updatedGroup).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith(CUSTOM_ID));
    expect(screen.queryByRole('group', { name: 'Daily notes' })).not.toBeInTheDocument();
  });

  it('resets General and Prompt independently without replacing the other profile', async () => {
    const modifiedGeneral = {
      ...DEFAULT_GENERAL_PROFILE,
      name: 'Changed General',
      activationKey: 'G' as const,
    };
    const modifiedPrompt = {
      ...DEFAULT_PROMPT_PROFILE,
      name: 'Changed Prompt',
      activationKey: 'P' as const,
      smartPrompt: 'Changed prompt preference.',
    };
    let authoritative = settingsWith([modifiedGeneral, modifiedPrompt]);
    const reset = vi.fn<MainApi['profiles']['reset']>((id) => {
      authoritative = settingsWith(
        authoritative.dictationProfiles.map((profile) =>
          profile.id === id
            ? structuredClone(id === 'general' ? DEFAULT_GENERAL_PROFILE : DEFAULT_PROMPT_PROFILE)
            : profile,
        ),
      );
      return Promise.resolve(authoritative);
    });
    const user = renderProfiles(authoritative, profileApi({ reset }));

    await user.click(within(group('Changed General')).getByRole('button', { name: 'Reset' }));
    expect(await screen.findByRole('group', { name: 'General' })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'Changed Prompt' })).toBeInTheDocument();

    await user.click(within(group('Changed Prompt')).getByRole('button', { name: 'Reset' }));
    expect(await screen.findByRole('group', { name: 'Prompt' })).toBeInTheDocument();
    expect(reset.mock.calls.map(([id]) => id)).toEqual(['general', 'prompt']);
  });

  it('blocks only duplicate exact key-and-Shift bindings and explains the conflict', async () => {
    const initial = settingsWith([
      DEFAULT_GENERAL_PROFILE,
      { ...DEFAULT_PROMPT_PROFILE, activationKey: 'Y' },
    ]);
    const create = vi.fn<MainApi['profiles']['create']>();
    const user = renderProfiles(initial, profileApi({ create }));

    await user.click(screen.getByRole('button', { name: 'Add custom profile' }));
    const editor = group('New custom profile');
    await user.selectOptions(within(editor).getByLabelText('New profile activation key'), 'Z');
    expect(within(editor).getByText('That exact shortcut is already used.')).toBeInTheDocument();
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeDisabled();

    await user.selectOptions(within(editor).getByLabelText('New profile Shift modifier'), 'shift');
    expect(within(editor).queryByText('That exact shortcut is already used.')).toBeNull();
    expect(
      within(editor).getByText(/default General and Prompt shortcuts are reserved/i),
    ).toBeInTheDocument();
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeDisabled();

    await user.selectOptions(within(editor).getByLabelText('New profile activation key'), 'X');
    expect(within(editor).queryByText(/shortcuts are reserved/i)).toBeNull();
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeEnabled();
    expect(create).not.toHaveBeenCalled();
  });

  it('recovers from API failure by restoring the authoritative profile and can retry', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE, CUSTOM]);
    const saved = { ...CUSTOM, name: 'Authoritative notes' };
    const update = vi
      .fn<MainApi['profiles']['update']>()
      .mockRejectedValueOnce(new Error('persistence failed'))
      .mockResolvedValueOnce(
        settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE, saved]),
      );
    const user = renderProfiles(initial, profileApi({ update }));

    const name = within(group('Notes')).getByLabelText('Notes profile name');
    await user.clear(name);
    await user.type(name, 'Unsaved notes');
    await user.click(within(group('Notes')).getByRole('button', { name: 'Save profile' }));

    expect(await screen.findByText(/profile could not be saved/i)).toBeInTheDocument();
    expect(within(group('Notes')).getByLabelText('Notes profile name')).toHaveValue('Notes');

    await user.clear(within(group('Notes')).getByLabelText('Notes profile name'));
    await user.type(within(group('Notes')).getByLabelText('Notes profile name'), 'Recovered notes');
    await user.click(within(group('Notes')).getByRole('button', { name: 'Save profile' }));
    expect(await screen.findByRole('group', { name: 'Authoritative notes' })).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'Recovered notes' })).toBeNull();
    expect(update).toHaveBeenLastCalledWith(CUSTOM_ID, { name: 'Recovered notes' });
    expect(update).toHaveBeenCalledTimes(2);
  });
});
