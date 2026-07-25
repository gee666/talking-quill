import { useState } from 'react';
import type { ActivationKey } from '../../../shared/helper/protocol';
import {
  GENERAL_PROFILE_ID,
  MAX_DICTATION_PROFILES,
  PROMPT_PROFILE_ID,
  RESERVED_DICTATION_BINDING_ERROR,
  isReservedBindingForAnotherProfile,
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../../shared/schemas/dictation-profiles';
import type { Settings } from '../../../shared/schemas/settings';
import { Button, Card, Input, Select, Status, TextArea } from '../../design';

const KEYS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split('') as ActivationKey[];

export function DictationProfilesSection({
  settings,
  onSettingsSaved,
}: {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [recoveryRevision, setRecoveryRevision] = useState(0);
  const mutate = async (operation: () => Promise<Settings>) => {
    setBusy(true);
    setError(null);
    try {
      onSettingsSaved(await operation());
      setCreating(false);
    } catch {
      setRecoveryRevision((revision) => revision + 1);
      setError(
        'The profile could not be saved. Shortcuts must be distinct, and built-in defaults stay reserved.',
      );
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card
      title="Dictation profiles"
      description="Each profile owns one exact Alt or Option shortcut and one processing mode."
    >
      <p>
        Shift is part of a shortcut; it does not switch modes. General and Prompt can be edited and
        reset, but not deleted.
      </p>
      {settings.dictationProfiles.map((profile) => (
        <ProfileEditor
          key={[profile.id, String(recoveryRevision), JSON.stringify(profile)].join(':')}
          profile={profile}
          profiles={settings.dictationProfiles}
          disabled={busy}
          onSave={(next) => {
            void mutate(() =>
              window.talkingQuill.profiles.update(profile.id, profilePatch(profile, next)),
            );
          }}
          {...(profile.id === GENERAL_PROFILE_ID || profile.id === PROMPT_PROFILE_ID
            ? {
                onReset: () => {
                  void mutate(() =>
                    window.talkingQuill.profiles.reset(
                      profile.id === GENERAL_PROFILE_ID ? GENERAL_PROFILE_ID : PROMPT_PROFILE_ID,
                    ),
                  );
                },
              }
            : {
                onDelete: () => {
                  void mutate(() => window.talkingQuill.profiles.delete(profile.id));
                },
              })}
        />
      ))}
      {creating ? (
        <ProfileEditor
          key={`create:${String(recoveryRevision)}`}
          profile={{
            id: '00000000-0000-4000-8000-000000000000',
            name: 'New profile',
            activationKey: firstAvailableKey(settings.dictationProfiles),
            shift: false,
            processingMode: 'raw',
            smartPrompt: null,
          }}
          profiles={settings.dictationProfiles}
          disabled={busy}
          create
          onSave={(next) => mutate(() => window.talkingQuill.profiles.create(withoutId(next)))}
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
  disabled,
  create = false,
  onSave,
  onDelete,
  onReset,
}: {
  readonly profile: DictationProfile;
  readonly profiles: readonly DictationProfile[];
  readonly disabled: boolean;
  readonly create?: boolean;
  readonly onSave: (profile: DictationProfile) => void;
  readonly onDelete?: () => void;
  readonly onReset?: () => void;
}) {
  const [draft, setDraft] = useState(profile);
  const duplicate = profiles.some(
    (candidate) =>
      candidate.id !== profile.id &&
      candidate.activationKey === draft.activationKey &&
      candidate.shift === draft.shift,
  );
  const reserved = isReservedBindingForAnotherProfile(
    create ? 'custom' : profile.id,
    draft.activationKey,
    draft.shift,
  );
  return (
    <fieldset className="gesture-test">
      <legend>{create ? 'New custom profile' : profile.name}</legend>
      <Input
        label={`${profile.name} profile name`}
        value={draft.name}
        maxLength={80}
        disabled={disabled}
        onChange={(event) => setDraft({ ...draft, name: event.currentTarget.value })}
      />
      <Select
        label={
          profile.id === GENERAL_PROFILE_ID ? 'Activation key' : `${profile.name} activation key`
        }
        value={draft.activationKey}
        disabled={disabled}
        onChange={(event) =>
          setDraft({ ...draft, activationKey: event.currentTarget.value as ActivationKey })
        }
      >
        {KEYS.map((key) => (
          <option key={key} value={key}>
            {key}
          </option>
        ))}
      </Select>
      <Select
        label={`${profile.name} Shift modifier`}
        value={draft.shift ? 'shift' : 'plain'}
        disabled={disabled}
        onChange={(event) => setDraft({ ...draft, shift: event.currentTarget.value === 'shift' })}
      >
        <option value="plain">Alt/Option only</option>
        <option value="shift">Alt/Option + Shift</option>
      </Select>
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
        hint="Additional formatting preference. Core safety and same-language rules always apply."
      />
      {duplicate ? <Status tone="error">That exact shortcut is already used.</Status> : null}
      {reserved ? <Status tone="error">{RESERVED_DICTATION_BINDING_ERROR}</Status> : null}
      <div>
        <Button
          disabled={
            disabled ||
            duplicate ||
            reserved ||
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

function profilePatch(current: DictationProfile, next: DictationProfile): DictationProfilePatch {
  const bindingChanged =
    current.activationKey !== next.activationKey || current.shift !== next.shift;
  return {
    ...(current.name === next.name ? {} : { name: next.name }),
    ...(bindingChanged ? { activationKey: next.activationKey, shift: next.shift } : {}),
    ...(current.processingMode === next.processingMode
      ? {}
      : { processingMode: next.processingMode }),
    ...(current.smartPrompt === next.smartPrompt ? {} : { smartPrompt: next.smartPrompt }),
  };
}

function withoutId(profile: DictationProfile): DictationProfileCreate {
  return {
    name: profile.name,
    activationKey: profile.activationKey,
    shift: profile.shift,
    processingMode: profile.processingMode,
    smartPrompt: profile.smartPrompt,
  };
}

function firstAvailableKey(profiles: readonly DictationProfile[]): ActivationKey {
  return (
    KEYS.find(
      (key) => !profiles.some((profile) => profile.activationKey === key && !profile.shift),
    ) ?? 'A'
  );
}
