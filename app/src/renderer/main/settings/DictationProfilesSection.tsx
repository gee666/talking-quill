import { useState } from 'react';
import {
  BuiltInDictationProfileIdSchema,
  MAX_DICTATION_PROFILES,
  RESERVED_DICTATION_BINDINGS,
  builtInDictationProfileMetadata,
  builtInDictationProfileName,
  dictationProfileBindingsConflict,
  isReservedBindingForProfile,
  reservedBindingOwner,
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../../shared/schemas/dictation-profiles';
import type { Settings } from '../../../shared/schemas/settings';
import {
  ShortcutKeySchema,
  shortcutFromLegacyActivation,
  shortcutsConflict,
  shortcutsEqual,
  type Shortcut,
} from '../../../shared/schemas/shortcut';
import { Button, Card, Input, Select, Status, TextArea } from '../../design';
import { formatKeyboardShortcut } from '../format-keyboard-shortcut';
import { KeyboardShortcutInput } from './KeyboardShortcutInput';

export function DictationProfilesSection({
  settings,
  platform,
  onSettingsSaved,
}: {
  readonly settings: Settings;
  readonly platform: string;
  readonly onSettingsSaved: (settings: Settings) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [recoveryRevisions, setRecoveryRevisions] = useState<Readonly<Record<string, number>>>({});
  const mutate = async (
    scope: string,
    operation: () => Promise<Settings>,
    onSuccess?: () => void,
  ) => {
    setBusy(true);
    setError(null);
    try {
      onSettingsSaved(await operation());
      onSuccess?.();
    } catch {
      if (scope !== 'create') {
        setRecoveryRevisions((revisions) => ({
          ...revisions,
          [scope]: (revisions[scope] ?? 0) + 1,
        }));
      }
      setError(
        'The profile could not be saved. Shortcut chords must be distinct, may not share a same-modifier prefix, and built-in defaults stay reserved.',
      );
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card
      title="Dictation profiles"
      description="Each profile owns one customizable keyboard chord and one processing mode."
    >
      <p>
        Hold the same nonempty modifier combination while pressing letter keys in order. The final
        letter is the trigger; its key down/up timing chooses Quick or Extended Dictation. Built-in
        profiles can be edited and reset, but not deleted.
      </p>
      {settings.dictationProfiles.map((profile) => (
        <ProfileEditor
          key={[
            profile.id,
            String(recoveryRevisions[profile.id] ?? 0),
            JSON.stringify(profile),
          ].join(':')}
          profile={profile}
          profiles={settings.dictationProfiles}
          platform={platform}
          disabled={busy}
          onSave={(next) => {
            void mutate(profile.id, () =>
              window.talkingQuill.profiles.update(profile.id, profilePatch(profile, next)),
            );
          }}
          {...(() => {
            const builtInId = BuiltInDictationProfileIdSchema.safeParse(profile.id);
            return builtInId.success
              ? {
                  onReset: () => {
                    void mutate(profile.id, () =>
                      window.talkingQuill.profiles.reset(builtInId.data),
                    );
                  },
                }
              : {
                  onDelete: () => {
                    void mutate(profile.id, () => window.talkingQuill.profiles.delete(profile.id));
                  },
                };
          })()}
        />
      ))}
      {creating ? (
        <ProfileEditor
          key="create"
          profile={{
            id: '00000000-0000-4000-8000-000000000000',
            name: 'New profile',
            shortcut: firstAvailableShortcut(settings.dictationProfiles),
            processingMode: 'raw',
            smartPrompt: null,
          }}
          profiles={settings.dictationProfiles}
          platform={platform}
          disabled={busy}
          create
          onSave={(next) =>
            mutate(
              'create',
              () => window.talkingQuill.profiles.create(withoutId(next)),
              () => setCreating(false),
            )
          }
          onDelete={() => setCreating(false)}
        />
      ) : (
        <Button
          disabled={busy || settings.dictationProfiles.length >= MAX_DICTATION_PROFILES}
          onClick={() => setCreating(true)}
        >
          Add custom profile
        </Button>
      )}
      {error === null ? null : <Status tone="error">{error}</Status>}
    </Card>
  );
}

function ProfileEditor({
  profile,
  profiles,
  platform,
  disabled,
  create = false,
  onSave,
  onDelete,
  onReset,
}: {
  readonly profile: DictationProfile;
  readonly profiles: readonly DictationProfile[];
  readonly platform: string;
  readonly disabled: boolean;
  readonly create?: boolean;
  readonly onSave: (profile: DictationProfile) => void;
  readonly onDelete?: () => void;
  readonly onReset?: () => void;
}) {
  const [draft, setDraft] = useState(profile);
  const [shortcutValid, setShortcutValid] = useState(true);
  const builtInMetadata = builtInDictationProfileMetadata(profile.id);
  const conflictingProfile = profiles.find(
    (candidate) =>
      candidate.id !== profile.id &&
      dictationProfileBindingsConflict(
        candidate.id,
        candidate.shortcut,
        create ? 'custom' : profile.id,
        draft.shortcut,
      ),
  );
  const conflictError =
    conflictingProfile === undefined
      ? undefined
      : profileConflictMessage(draft.shortcut, conflictingProfile, platform);
  const reservationOwner = reservedBindingOwner(draft.shortcut);
  const reserved = isReservedBindingForProfile(create ? 'custom' : profile.id, draft.shortcut);
  const reservationError = reserved
    ? reservedConflictMessage(draft.shortcut, reservationOwner, platform)
    : undefined;
  return (
    <fieldset className="gesture-test">
      <legend>{create ? 'New custom profile' : profile.name}</legend>
      {builtInMetadata === null ? null : <p>{builtInMetadata.description}</p>}
      <Input
        label={`${profile.name} profile name`}
        value={draft.name}
        maxLength={80}
        disabled={disabled}
        onChange={(event) => setDraft({ ...draft, name: event.currentTarget.value })}
      />
      <KeyboardShortcutInput
        label={`${profile.name} keyboard shortcut`}
        shortcut={draft.shortcut}
        platform={platform}
        disabled={disabled}
        error={conflictError ?? reservationError}
        onChange={(shortcut) => setDraft({ ...draft, shortcut })}
        onCaptureValidityChange={setShortcutValid}
      />
      <Select
        label={`${profile.name} processing mode`}
        value={draft.processingMode}
        disabled={disabled}
        onChange={(event) =>
          setDraft({ ...draft, processingMode: event.currentTarget.value as 'raw' | 'smart' })
        }
      >
        <option value="raw">Raw transcription</option>
        <option value="smart">Smart transcription</option>
      </Select>
      <TextArea
        label={`${profile.name} Smart prompt (optional)`}
        value={draft.smartPrompt ?? ''}
        maxLength={4_096}
        rows={4}
        disabled={disabled}
        onChange={(event) =>
          setDraft({
            ...draft,
            smartPrompt:
              event.currentTarget.value.trim().length === 0 ? null : event.currentTarget.value,
          })
        }
        hint="Additional cleanup, formatting, or translation instruction. Core content-safety rules always apply."
      />
      <div>
        <Button
          disabled={
            disabled ||
            conflictingProfile !== undefined ||
            reserved ||
            !shortcutValid ||
            draft.name.trim().length === 0 ||
            (!create && JSON.stringify(draft) === JSON.stringify(profile))
          }
          onClick={() => onSave(draft)}
        >
          {create ? 'Create profile' : 'Save profile'}
        </Button>{' '}
        {onReset === undefined ? null : (
          <Button variant="quiet" disabled={disabled} onClick={onReset}>
            Reset
          </Button>
        )}{' '}
        {onDelete === undefined ? null : (
          <Button variant="quiet" disabled={disabled} onClick={onDelete}>
            {create ? 'Cancel' : 'Delete'}
          </Button>
        )}
      </div>
    </fieldset>
  );
}

function profileConflictMessage(
  shortcut: Shortcut,
  conflictingProfile: DictationProfile,
  platform: string,
): string {
  const candidate = formatKeyboardShortcut(shortcut, platform);
  const existing = formatKeyboardShortcut(conflictingProfile.shortcut, platform);
  if (shortcutsEqual(shortcut, conflictingProfile.shortcut)) {
    return `${candidate} is already used by ${conflictingProfile.name} (${existing}).`;
  }
  return `${candidate} conflicts with ${conflictingProfile.name} (${existing}) because one chord is a prefix of the other.`;
}

function reservedConflictMessage(
  shortcut: Shortcut,
  ownerId: ReturnType<typeof reservedBindingOwner>,
  platform: string,
): string {
  const candidate = formatKeyboardShortcut(shortcut, platform);
  const binding = RESERVED_DICTATION_BINDINGS.find(({ ownerId: owner }) => owner === ownerId);
  if (binding === undefined || ownerId === null) {
    return `${candidate} conflicts with a reserved built-in shortcut chord.`;
  }
  const ownerName = builtInDictationProfileName(ownerId);
  const reservedShortcut = formatKeyboardShortcut(binding.shortcut, platform);
  if (shortcutsEqual(shortcut, binding.shortcut)) {
    return `${candidate} matches the reserved ${ownerName} default ${reservedShortcut}.`;
  }
  return `${candidate} conflicts with the reserved ${ownerName} default ${reservedShortcut} because one chord is a prefix of the other.`;
}

function profilePatch(current: DictationProfile, next: DictationProfile): DictationProfilePatch {
  const bindingChanged = !shortcutsEqual(current.shortcut, next.shortcut);
  return {
    ...(current.name === next.name ? {} : { name: next.name }),
    ...(bindingChanged ? { shortcut: next.shortcut } : {}),
    ...(current.processingMode === next.processingMode
      ? {}
      : { processingMode: next.processingMode }),
    ...(current.smartPrompt === next.smartPrompt ? {} : { smartPrompt: next.smartPrompt }),
  };
}

function withoutId(profile: DictationProfile): DictationProfileCreate {
  return {
    name: profile.name,
    shortcut: profile.shortcut,
    processingMode: profile.processingMode,
    smartPrompt: profile.smartPrompt,
  };
}

function firstAvailableShortcut(profiles: readonly DictationProfile[]): Shortcut {
  for (const key of ShortcutKeySchema.options) {
    const shortcut = shortcutFromLegacyActivation(key, false);
    if (
      !profiles.some((profile) => shortcutsConflict(profile.shortcut, shortcut)) &&
      !isReservedBindingForProfile('custom', shortcut)
    ) {
      return shortcut;
    }
  }
  return shortcutFromLegacyActivation('A', false);
}
