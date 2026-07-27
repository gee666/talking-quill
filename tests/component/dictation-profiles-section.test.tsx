// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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
import { shortcutFromLegacyActivation, type Shortcut } from '../../app/src/shared/schemas/shortcut';

const CUSTOM_ID = '11111111-1111-4111-8111-111111111111' as CustomDictationProfileId;
const CUSTOM: DictationProfile = {
  id: CUSTOM_ID,
  name: 'Notes',
  shortcut: shortcutFromLegacyActivation('Q', false),
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
    value: {
      profiles,
      shortcutCapture: {
        start: vi.fn(() => Promise.resolve()),
        stop: vi.fn(() => Promise.resolve()),
      },
    },
  });
  function Harness() {
    const [settings, setSettings] = useState(initial);
    return (
      <DictationProfilesSection
        settings={settings}
        platform="win32"
        onSettingsSaved={setSettings}
      />
    );
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

async function captureShortcut(
  input: HTMLElement,
  code: `Key${string}`,
  modifiers: {
    readonly altKey?: boolean;
    readonly shiftKey?: boolean;
    readonly ctrlKey?: boolean;
    readonly metaKey?: boolean;
  },
) {
  input.focus();
  await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
  const result = fireEvent.keyDown(input, { key: code.slice(3), code, ...modifiers });
  fireEvent.keyUp(input, { key: code.slice(3), code, ...modifiers });
  return result;
}

describe('DictationProfilesSection', () => {
  it('creates a custom profile and renders the authoritative API result', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const created = {
      ...CUSTOM,
      name: 'Meeting notes',
      shortcut: shortcutFromLegacyActivation('A', false),
    };
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
        shortcut: shortcutFromLegacyActivation('A', false),
        processingMode: 'raw',
        smartPrompt: null,
      }),
    );
    expect(await screen.findByRole('group', { name: 'Meeting notes' })).toBeInTheDocument();
  });

  it('captures held letters in physical down order and saves the complete chord', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const captured: Shortcut = {
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['X', 'P'],
    };
    const movedGeneral = { ...DEFAULT_GENERAL_PROFILE, shortcut: captured };
    const update = vi.fn<MainApi['profiles']['update']>(() =>
      Promise.resolve(settingsWith([movedGeneral, DEFAULT_PROMPT_PROFILE])),
    );
    const user = renderProfiles(initial, profileApi({ update }));

    const input = within(group('General')).getByRole('textbox', {
      name: 'General keyboard shortcut',
    });
    input.focus();
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    expect(fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true })).toBe(false);
    expect(input).toHaveValue('Alt + X');
    expect(fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true })).toBe(false);
    expect(input).toHaveValue('Alt + X + P');
    fireEvent.keyUp(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'x', code: 'KeyX', altKey: true });
    expect(within(group('General')).getByRole('status')).toHaveTextContent(
      'Alt + X + P captured. P is the final trigger.',
    );

    await user.click(within(group('General')).getByRole('button', { name: 'Save profile' }));
    await waitFor(() =>
      expect(update).toHaveBeenCalledWith('general', {
        shortcut: captured,
      }),
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
      shortcut: shortcutFromLegacyActivation('G', false),
    };
    const modifiedPrompt = {
      ...DEFAULT_PROMPT_PROFILE,
      name: 'Changed Prompt',
      shortcut: shortcutFromLegacyActivation('P', true),
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

    await user.clear(within(group('Changed Prompt')).getByLabelText('Changed Prompt profile name'));
    await user.type(
      within(group('Changed Prompt')).getByLabelText('Changed Prompt profile name'),
      'Unsaved Prompt',
    );
    await user.click(within(group('Changed General')).getByRole('button', { name: 'Reset' }));
    expect(await screen.findByRole('group', { name: 'General' })).toBeInTheDocument();
    expect(
      within(screen.getByRole('group', { name: 'Changed Prompt' })).getByLabelText(
        'Changed Prompt profile name',
      ),
    ).toHaveValue('Unsaved Prompt');

    await user.click(within(group('Changed Prompt')).getByRole('button', { name: 'Reset' }));
    expect(await screen.findByRole('group', { name: 'Prompt' })).toBeInTheDocument();
    expect(reset.mock.calls.map(([id]) => id)).toEqual(['general', 'prompt']);
  });

  it('blocks and accessibly announces exact, prefix, and reserved full-chord conflicts', async () => {
    const generalShortcut = shortcutFromLegacyActivation('X', false);
    const initial = settingsWith([
      { ...DEFAULT_GENERAL_PROFILE, shortcut: generalShortcut },
      { ...DEFAULT_PROMPT_PROFILE, shortcut: shortcutFromLegacyActivation('Y', true) },
    ]);
    const create = vi.fn<MainApi['profiles']['create']>();
    const user = renderProfiles(initial, profileApi({ create }));

    await user.click(screen.getByRole('button', { name: 'Add custom profile' }));
    const editor = group('New custom profile');
    const input = within(editor).getByRole('textbox', {
      name: 'New profile keyboard shortcut',
    });

    await captureShortcut(input, 'KeyX', { altKey: true });
    expect(
      within(editor).getByText('Alt + X is already used by General (Alt + X).', {
        selector: '.me-field__error',
      }),
    ).toBeVisible();
    expect(within(editor).getByRole('status')).toHaveTextContent(
      'Alt + X is already used by General (Alt + X).',
    );
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeDisabled();

    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'x', code: 'KeyX', altKey: true });
    expect(
      within(editor).getByText(
        'Alt + X + P conflicts with General (Alt + X) because one chord is a prefix of the other.',
        { selector: '.me-field__error' },
      ),
    ).toBeVisible();
    expect(within(editor).getByRole('status')).toHaveTextContent('Alt + X + P');

    await captureShortcut(input, 'KeyZ', { altKey: true, shiftKey: true });
    expect(
      within(editor).getByText(
        'Alt + Shift + Z matches the reserved Prompt default Alt + Shift + Z.',
        { selector: '.me-field__error' },
      ),
    ).toBeVisible();
    expect(within(editor).getByRole('status')).toHaveTextContent('Alt + Shift + Z');

    await captureShortcut(input, 'KeyP', { ctrlKey: true, shiftKey: true });
    expect(input).toHaveValue('Ctrl + Shift + P');
    expect(within(editor).queryByText(/conflicts|already used|reserved/i)).toBeNull();
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeEnabled();
    expect(create).not.toHaveBeenCalled();
  });

  it('guides unsupported input without changing the shortcut or blocking a name-only save', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const renamed = { ...DEFAULT_GENERAL_PROFILE, name: 'Renamed General' };
    const update = vi.fn<MainApi['profiles']['update']>(() =>
      Promise.resolve(settingsWith([renamed, DEFAULT_PROMPT_PROFILE])),
    );
    const user = renderProfiles(initial, profileApi({ update }));
    const editor = group('General');
    const input = within(editor).getByRole('textbox', {
      name: 'General keyboard shortcut',
    });
    const bubbled = vi.fn();
    document.addEventListener('keydown', bubbled);

    expect(await captureShortcut(input, 'KeyQ', {})).toBe(false);
    expect(input).toHaveValue('Alt + Z');
    expect(input).not.toHaveAttribute('aria-invalid');
    expect(within(editor).getByRole('status')).toHaveTextContent(/first letter must be pressed/i);
    expect(bubbled).not.toHaveBeenCalled();

    expect(fireEvent.keyDown(input, { key: 'F1', code: 'F1' })).toBe(false);
    expect(within(editor).getByRole('status')).toHaveTextContent(/Only physical letter keys A–Z/i);
    expect(fireEvent.keyDown(input, { key: 'Shift', code: 'ShiftLeft', shiftKey: true })).toBe(
      false,
    );
    expect(within(editor).getByRole('status')).toHaveTextContent(
      /Keep at least one modifier held/i,
    );
    expect(fireEvent.keyDown(input, { key: 'Tab', code: 'Tab' })).toBe(true);
    expect(fireEvent.keyDown(input, { key: 'Tab', code: 'Tab', shiftKey: true })).toBe(true);
    expect(bubbled).toHaveBeenCalledTimes(2);

    expect(
      fireEvent.keyDown(input, {
        key: 'p',
        code: 'KeyP',
        ctrlKey: true,
        shiftKey: true,
        repeat: true,
      }),
    ).toBe(false);
    expect(
      fireEvent.keyDown(input, {
        key: 'p',
        code: 'KeyP',
        ctrlKey: true,
        shiftKey: true,
        isComposing: true,
      }),
    ).toBe(false);
    expect(input).toHaveValue('Alt + Z');

    await user.clear(within(editor).getByLabelText('General profile name'));
    await user.type(within(editor).getByLabelText('General profile name'), 'Renamed General');
    await user.click(within(editor).getByRole('button', { name: 'Save profile' }));
    await waitFor(() =>
      expect(update).toHaveBeenCalledWith('general', { name: 'Renamed General' }),
    );

    document.removeEventListener('keydown', bubbled);
  });

  it('remounts only the failed profile and preserves unrelated unsaved drafts', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE, CUSTOM]);
    const update = vi.fn<MainApi['profiles']['update']>().mockRejectedValue(new Error('failed'));
    const user = renderProfiles(initial, profileApi({ update }));

    await user.clear(within(group('Notes')).getByLabelText('Notes profile name'));
    await user.type(within(group('Notes')).getByLabelText('Notes profile name'), 'Unsaved Notes');
    await user.clear(within(group('General')).getByLabelText('General profile name'));
    await user.type(
      within(group('General')).getByLabelText('General profile name'),
      'Broken General',
    );
    await user.click(within(group('General')).getByRole('button', { name: 'Save profile' }));

    expect(await screen.findByText(/profile could not be saved/i)).toBeVisible();
    expect(within(group('General')).getByLabelText('General profile name')).toHaveValue('General');
    expect(within(group('Notes')).getByLabelText('Notes profile name')).toHaveValue(
      'Unsaved Notes',
    );
  });

  it('preserves a failed create draft and closes it only after a successful create', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const create = vi
      .fn<MainApi['profiles']['create']>()
      .mockRejectedValueOnce(new Error('failed'))
      .mockImplementationOnce((input) =>
        Promise.resolve(
          settingsWith([
            ...initial.dictationProfiles,
            { id: CUSTOM_ID, ...structuredClone(input) },
          ]),
        ),
      );
    const user = renderProfiles(initial, profileApi({ create }));

    await user.click(screen.getByRole('button', { name: 'Add custom profile' }));
    const createEditor = group('New custom profile');
    await user.clear(within(createEditor).getByLabelText('New profile profile name'));
    await user.type(
      within(createEditor).getByLabelText('New profile profile name'),
      'Draft profile',
    );
    await user.click(within(createEditor).getByRole('button', { name: 'Create profile' }));

    expect(await screen.findByText(/profile could not be saved/i)).toBeVisible();
    expect(
      within(group('New custom profile')).getByLabelText('New profile profile name'),
    ).toHaveValue('Draft profile');

    await user.click(
      within(group('New custom profile')).getByRole('button', { name: 'Create profile' }),
    );
    expect(await screen.findByRole('group', { name: 'Draft profile' })).toBeVisible();
    expect(screen.queryByRole('group', { name: 'New custom profile' })).toBeNull();
    expect(create).toHaveBeenCalledTimes(2);
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
