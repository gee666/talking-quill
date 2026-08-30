// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DictationProfilesSection } from '../../app/src/renderer/main/settings/DictationProfilesSection';
import type { MainApi } from '../../app/src/shared/bridge/api';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
  builtInDictationProfile,
  defaultDictationProfiles,
  type CustomDictationProfileId,
  type DictationProfile,
} from '../../app/src/shared/schemas/dictation-profiles';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';
import { ShortcutCaptureLeaseIdSchema } from '../../app/src/shared/schemas/shortcut-capture';
import { shortcutFromLegacyActivation, type Shortcut } from '../../app/src/shared/schemas/shortcut';

const CUSTOM_ID = '11111111-1111-4111-8111-111111111111' as CustomDictationProfileId;
const SHORTCUT_CAPTURE_LEASE = ShortcutCaptureLeaseIdSchema.parse(
  '33333333-3333-4333-8333-333333333333',
);
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
  const complete = profiles.map((profile) => structuredClone(profile));
  for (const builtIn of defaultDictationProfiles()) {
    if (!complete.some(({ id }) => id === builtIn.id)) complete.push(builtIn);
  }
  return {
    ...structuredClone(DEFAULT_SETTINGS),
    dictationProfiles: complete,
  };
}

function renderProfiles(
  initial: Settings,
  profiles: MainApi['profiles'],
  heading: string | null = 'Dictation profiles',
  shortcutCapture: MainApi['shortcutCapture'] = {
    start: vi.fn(() => Promise.resolve(SHORTCUT_CAPTURE_LEASE)),
    stop: vi.fn(() => Promise.resolve()),
  },
): ReturnType<typeof userEvent.setup> {
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: {
      profiles,
      shortcutCapture,
    },
  });
  function Harness() {
    const [settings, setSettings] = useState(initial);
    return (
      <DictationProfilesSection
        settings={settings}
        platform="win32"
        onSettingsSaved={setSettings}
        heading={heading}
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
    importFile: vi.fn(() => Promise.resolve({ status: 'cancelled' as const })),
    exportFile: vi.fn(() => Promise.resolve({ status: 'cancelled' as const })),
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
  it('removes all section lead copy while keeping built-in editor descriptions', () => {
    renderProfiles(settingsWith([DEFAULT_GENERAL_PROFILE]), profileApi());

    expect(screen.queryByText(/A profile is a keyboard shortcut/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/You can have several/i)).not.toBeInTheDocument();
    expect(
      screen.queryByText(/keys before the last one still reach whatever app you are in/i),
    ).not.toBeInTheDocument();
    expect(
      within(group('General')).getByText(
        'Default: cleans up and formats the transcript in its source language.',
      ),
    ).toBeVisible();
  });

  it('omits its own title when the screen already renders one', () => {
    renderProfiles(settingsWith([DEFAULT_GENERAL_PROFILE]), profileApi(), null);

    expect(screen.queryByRole('heading', { name: 'Dictation profiles' })).toBeNull();
    expect(group('General')).toBeVisible();
  });

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
    const newEditor = group('New custom profile');
    await user.clear(within(newEditor).getByLabelText('Name'));
    await user.type(within(newEditor).getByLabelText('Name'), 'Meeting notes');
    await user.click(within(newEditor).getByRole('button', { name: 'Create profile' }));

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
      keys: ['Y', 'Q'],
    };
    const movedGeneral = { ...DEFAULT_GENERAL_PROFILE, shortcut: captured };
    const update = vi.fn<MainApi['profiles']['update']>(() =>
      Promise.resolve(settingsWith([movedGeneral, DEFAULT_PROMPT_PROFILE])),
    );
    const user = renderProfiles(initial, profileApi({ update }));

    const input = within(group('General')).getByRole('textbox', {
      name: 'Shortcut',
    });
    input.focus();
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    expect(fireEvent.keyDown(input, { key: 'y', code: 'KeyY', altKey: true })).toBe(false);
    expect(input).toHaveValue('Alt + Y');
    expect(fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true })).toBe(false);
    expect(input).toHaveValue('Alt + Y + Q');
    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyUp(input, { key: 'y', code: 'KeyY', altKey: true });
    expect(
      within(group('General'))
        .getAllByRole('status')
        .map((status) => status.textContent)
        .join(' '),
    ).not.toMatch(/Got it|hold|release/i);

    fireEvent.blur(input);
    await waitFor(() =>
      expect(within(group('General')).getByRole('button', { name: 'Save profile' })).toBeEnabled(),
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

    await user.clear(within(group('Notes')).getByLabelText('Name'));
    await user.type(within(group('Notes')).getByLabelText('Name'), 'Daily notes');
    await user.selectOptions(
      within(group('Notes')).getByLabelText('What happens to your words'),
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

  it('resets every built-in independently without replacing the other profiles', async () => {
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
            ? structuredClone(
                builtInDictationProfile(id) ??
                  (() => {
                    throw new Error('Missing built-in');
                  })(),
              )
            : profile,
        ),
      );
      return Promise.resolve(authoritative);
    });
    const user = renderProfiles(authoritative, profileApi({ reset }));

    await user.clear(within(group('Changed Prompt')).getByLabelText('Name'));
    await user.type(within(group('Changed Prompt')).getByLabelText('Name'), 'Unsaved Prompt');
    await user.click(within(group('Changed General')).getByRole('button', { name: 'Reset' }));
    const resetGeneral = await screen.findByRole('group', { name: 'General' });
    expect(within(resetGeneral).getByLabelText('Shortcut')).toHaveValue('Alt + X');
    expect(
      within(screen.getByRole('group', { name: 'Changed Prompt' })).getByLabelText('Name'),
    ).toHaveValue('Unsaved Prompt');

    await user.click(within(group('Changed Prompt')).getByRole('button', { name: 'Reset' }));
    const resetPrompt = await screen.findByRole('group', { name: 'Prompt' });
    expect(within(resetPrompt).getByLabelText('Shortcut')).toHaveValue('Alt + X + P');
    await user.click(within(group('Markdown')).getByRole('button', { name: 'Reset' }));
    await user.click(within(group('Translate to English')).getByRole('button', { name: 'Reset' }));
    expect(reset.mock.calls.map(([id]) => id)).toEqual([
      'general',
      'prompt',
      'markdown',
      'translate-to-english',
    ]);
  });

  it('rejects exact duplicates and exact built-in reservations while allowing shared prefixes', async () => {
    const initial = settingsWith([
      { ...DEFAULT_GENERAL_PROFILE, shortcut: shortcutFromLegacyActivation('G', true) },
      { ...DEFAULT_PROMPT_PROFILE, shortcut: shortcutFromLegacyActivation('P', true) },
      { ...DEFAULT_MARKDOWN_PROFILE, shortcut: shortcutFromLegacyActivation('M', true) },
      {
        ...DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
        shortcut: shortcutFromLegacyActivation('E', true),
      },
      CUSTOM,
    ]);
    const create = vi.fn<MainApi['profiles']['create']>();
    const user = renderProfiles(initial, profileApi({ create }));

    await user.click(screen.getByRole('button', { name: 'Add custom profile' }));
    const editor = group('New custom profile');
    const input = within(editor).getByRole('textbox', { name: 'Shortcut' });

    await captureShortcut(input, 'KeyQ', { altKey: true });
    expect(
      within(editor).getByText(
        'Alt + Q is already used by Notes (Alt + Q). Pick a different one.',
        {
          selector: '.me-field__error',
        },
      ),
    ).toBeVisible();
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeDisabled();

    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ', altKey: true });
    expect(input).toHaveValue('Alt + Q + P');
    expect(within(editor).queryByText(/Alt \+ Q \+ P is already used/i)).toBeNull();
    expect(within(editor).queryByText(/shared prefix/i)).toBeNull();
    expect(within(editor).queryByText(/release the shorter/i)).toBeNull();
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeEnabled();

    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'x', code: 'KeyX', altKey: true });
    expect(input).toHaveValue('Alt + X + P');
    expect(
      within(editor).getByText(
        'Alt + X + P is the original shortcut for Prompt (Alt + X + P), which stays reserved for that profile. Pick a different one.',
        { selector: '.me-field__error' },
      ),
    ).toBeVisible();
    expect(within(editor).getByRole('button', { name: 'Create profile' })).toBeDisabled();
    expect(create).not.toHaveBeenCalled();
  });

  it('allows noncanonical Alt+X descendants without adding prefix instructions', async () => {
    renderProfiles(settingsWith(defaultDictationProfiles()), profileApi());
    const editor = group('General');
    const input = within(editor).getByRole('textbox', { name: 'Shortcut' });

    input.focus();
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, { key: 'e', code: 'KeyE', altKey: true });
    fireEvent.keyUp(input, { key: 'e', code: 'KeyE', altKey: true });
    fireEvent.keyUp(input, { key: 'x', code: 'KeyX', altKey: true });

    expect(input).toHaveValue('Alt + X + E');
    expect(within(editor).queryByText(/Pick a different one/i)).toBeNull();
    expect(within(editor).queryByText(/shared prefix/i)).toBeNull();
    expect(within(editor).queryByText(/release the shorter/i)).toBeNull();
    expect(within(editor).getByRole('button', { name: 'Save profile' })).toBeEnabled();
  });

  it('does not render automatic copy for nested shared-prefix relationships', async () => {
    const legacyFamily: DictationProfile[] = defaultDictationProfiles().map((profile) =>
      profile.id === 'general'
        ? { ...profile, shortcut: shortcutFromLegacyActivation('X', false) }
        : profile.id === 'prompt'
          ? {
              ...profile,
              shortcut: {
                modifiers: { ctrl: false, alt: true, shift: false, meta: false },
                keys: ['X', 'P'],
              },
            }
          : profile,
    );
    const user = renderProfiles(settingsWith(legacyFamily), profileApi());
    await user.click(screen.getByRole('button', { name: 'Add custom profile' }));
    const editor = group('New custom profile');
    const input = within(editor).getByRole('textbox', { name: 'Shortcut' });
    input.focus();
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));

    for (const key of ['X', 'P', 'E']) {
      fireEvent.keyDown(input, { key: key.toLowerCase(), code: `Key${key}`, altKey: true });
    }

    expect(input).toHaveValue('Alt + X + P + E');
    expect(within(editor).queryByText(/shared prefix/i)).toBeNull();
    expect(within(editor).queryByText(/release the shorter/i)).toBeNull();
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
      name: 'Shortcut',
    });
    const bubbled = vi.fn();
    document.addEventListener('keydown', bubbled);

    expect(await captureShortcut(input, 'KeyQ', {})).toBe(false);
    expect(input).toHaveValue('Alt + X');
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(within(editor).getByRole('status')).toHaveTextContent(
      'Add Ctrl, Alt, Shift, or the Windows key.',
    );
    expect(bubbled).not.toHaveBeenCalled();

    expect(fireEvent.keyDown(input, { key: 'F1', code: 'F1' })).toBe(false);
    expect(within(editor).getByRole('status')).toHaveTextContent('Use only the letters A to Z.');
    expect(fireEvent.keyDown(input, { key: 'Shift', code: 'ShiftLeft', shiftKey: true })).toBe(
      false,
    );
    expect(within(editor).getByRole('status')).toHaveTextContent('Use only the letters A to Z.');
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
    expect(input).toHaveValue('Alt + X');

    await user.clear(within(editor).getByLabelText('Name'));
    await user.type(within(editor).getByLabelText('Name'), 'Renamed General');
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

    await user.clear(within(group('Notes')).getByLabelText('Name'));
    await user.type(within(group('Notes')).getByLabelText('Name'), 'Unsaved Notes');
    await user.clear(within(group('General')).getByLabelText('Name'));
    await user.type(within(group('General')).getByLabelText('Name'), 'Broken General');
    await user.click(within(group('General')).getByRole('button', { name: 'Save profile' }));

    expect(await screen.findByText(/profile couldn’t be saved/i)).toBeVisible();
    expect(within(group('General')).getByLabelText('Name')).toHaveValue('General');
    expect(within(group('Notes')).getByLabelText('Name')).toHaveValue('Unsaved Notes');
  });

  it('queues the first Save click behind exact lease restoration and saves once', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const renamed = { ...DEFAULT_GENERAL_PROFILE, name: 'Restored General' };
    const update = vi.fn<MainApi['profiles']['update']>(() =>
      Promise.resolve(settingsWith([renamed, DEFAULT_PROMPT_PROFILE])),
    );
    let resolveStop!: () => void;
    const shortcutCapture = {
      start: vi.fn(() => Promise.resolve(SHORTCUT_CAPTURE_LEASE)),
      stop: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            resolveStop = resolve;
          }),
      ),
    };
    const user = renderProfiles(
      initial,
      profileApi({ update }),
      'Dictation profiles',
      shortcutCapture,
    );
    const editor = group('General');
    await user.clear(within(editor).getByLabelText('Name'));
    await user.type(within(editor).getByLabelText('Name'), 'Restored General');
    const shortcut = within(editor).getByLabelText('Shortcut');
    shortcut.focus();
    await waitFor(() => expect(shortcut).not.toHaveAttribute('aria-busy'));

    await user.click(within(editor).getByRole('button', { name: 'Save profile' }));
    expect(shortcutCapture.stop).toHaveBeenCalledWith(SHORTCUT_CAPTURE_LEASE);
    expect(update).not.toHaveBeenCalled();

    resolveStop();
    await waitFor(() =>
      expect(update).toHaveBeenCalledWith('general', { name: 'Restored General' }),
    );
    expect(update).toHaveBeenCalledOnce();
  });

  it('fails the queued save safely and restores the old shortcut when lease restoration fails', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const update = vi.fn<MainApi['profiles']['update']>();
    const shortcutCapture = {
      start: vi.fn(() => Promise.resolve(SHORTCUT_CAPTURE_LEASE)),
      stop: vi
        .fn<MainApi['shortcutCapture']['stop']>()
        .mockRejectedValueOnce(new Error('restore failed'))
        .mockResolvedValue(undefined),
    };
    const user = renderProfiles(
      initial,
      profileApi({ update }),
      'Dictation profiles',
      shortcutCapture,
    );

    const editor = group('General');
    const shortcut = within(editor).getByLabelText('Shortcut');
    shortcut.focus();
    await waitFor(() => expect(shortcut).not.toHaveAttribute('aria-busy'));
    fireEvent.keyDown(shortcut, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyUp(shortcut, { key: 'q', code: 'KeyQ', altKey: true });
    expect(shortcut).toHaveValue('Alt + Q');

    await user.click(within(editor).getByRole('button', { name: 'Save profile' }));

    await waitFor(() => expect(shortcutCapture.stop).toHaveBeenCalledOnce());
    await waitFor(() => expect(shortcut).toHaveValue('Alt + X'));
    expect(within(editor).getByRole('status')).toHaveTextContent(
      /shortcuts couldn’t be switched back on/i,
    );
    expect(update).not.toHaveBeenCalled();

    fireEvent(window, new Event('focus'));
    await waitFor(() => expect(shortcutCapture.stop).toHaveBeenCalledTimes(2));
  });

  it('blocks a cross-editor save while a retained lease retry is unresolved or fails', async () => {
    const initial = settingsWith([DEFAULT_GENERAL_PROFILE, DEFAULT_PROMPT_PROFILE]);
    const update = vi.fn<MainApi['profiles']['update']>();
    let rejectRetry!: (error: Error) => void;
    const shortcutCapture = {
      start: vi.fn(() => Promise.resolve(SHORTCUT_CAPTURE_LEASE)),
      stop: vi
        .fn<MainApi['shortcutCapture']['stop']>()
        .mockRejectedValueOnce(new Error('initial restore failed'))
        .mockImplementationOnce(
          () =>
            new Promise<void>((_resolve, reject) => {
              rejectRetry = reject;
            }),
        )
        .mockResolvedValue(undefined),
    };
    const user = renderProfiles(
      initial,
      profileApi({ update }),
      'Dictation profiles',
      shortcutCapture,
    );

    const general = group('General');
    await user.clear(within(general).getByLabelText('Name'));
    await user.type(within(general).getByLabelText('Name'), 'Blocked General');
    const shortcut = within(general).getByLabelText('Shortcut');
    shortcut.focus();
    await waitFor(() => expect(shortcut).not.toHaveAttribute('aria-busy'));
    await user.click(within(general).getByRole('button', { name: 'Save profile' }));
    await waitFor(() => expect(shortcutCapture.stop).toHaveBeenCalledOnce());
    expect(update).not.toHaveBeenCalled();

    fireEvent(window, new Event('focus'));
    await waitFor(() => expect(shortcutCapture.stop).toHaveBeenCalledTimes(2));

    const prompt = group('Prompt');
    await user.clear(within(prompt).getByLabelText('Name'));
    await user.type(within(prompt).getByLabelText('Name'), 'Blocked Prompt');
    await user.click(within(prompt).getByRole('button', { name: 'Save profile' }));
    expect(update).not.toHaveBeenCalled();

    rejectRetry(new Error('retry failed'));
    await waitFor(() =>
      expect(within(prompt).getByRole('button', { name: 'Save profile' })).toBeEnabled(),
    );
    expect(update).not.toHaveBeenCalled();

    fireEvent(window, new Event('focus'));
    await waitFor(() => expect(shortcutCapture.stop).toHaveBeenCalledTimes(3));
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
    await user.clear(within(createEditor).getByLabelText('Name'));
    await user.type(within(createEditor).getByLabelText('Name'), 'Draft profile');
    await user.click(within(createEditor).getByRole('button', { name: 'Create profile' }));

    expect(await screen.findByText(/profile couldn’t be saved/i)).toBeVisible();
    expect(within(group('New custom profile')).getByLabelText('Name')).toHaveValue('Draft profile');

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

    const name = within(group('Notes')).getByLabelText('Name');
    await user.clear(name);
    await user.type(name, 'Unsaved notes');
    await user.click(within(group('Notes')).getByRole('button', { name: 'Save profile' }));

    expect(await screen.findByText(/profile couldn’t be saved/i)).toBeInTheDocument();
    expect(within(group('Notes')).getByLabelText('Name')).toHaveValue('Notes');

    await user.clear(within(group('Notes')).getByLabelText('Name'));
    await user.type(within(group('Notes')).getByLabelText('Name'), 'Recovered notes');
    await user.click(within(group('Notes')).getByRole('button', { name: 'Save profile' }));
    expect(await screen.findByRole('group', { name: 'Authoritative notes' })).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: 'Recovered notes' })).toBeNull();
    expect(update).toHaveBeenLastCalledWith(CUSTOM_ID, { name: 'Recovered notes' });
    expect(update).toHaveBeenCalledTimes(2);
  });
});
