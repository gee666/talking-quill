import { useRef, useState } from 'react';
import {
  RESERVED_DICTATION_BINDINGS,
  builtInDictationProfileMetadata,
  builtInDictationProfileName,
  dictationProfileBindingsConflict,
  isReservedBindingForProfile,
  reservedBindingOwner,
  type DictationProfile,
} from '../../../shared/schemas/dictation-profiles';
import { shortcutPlatformPolicy } from '../../../shared/schemas/shortcut-platform-policy';
import type { Shortcut } from '../../../shared/schemas/shortcut';
import { Button, Input, Select, TextArea } from '../../design';
import { formatKeyboardShortcut } from '../format-keyboard-shortcut';
import { KeyboardShortcutInput } from './KeyboardShortcutInput';
import { waitForShortcutCaptureRestorations } from './shortcut-capture-restoration';

export function ProfileEditor({
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
  readonly onSave: (profile: DictationProfile) => Promise<void>;
  readonly onDelete?: () => void;
  readonly onReset?: () => void;
}) {
  const [draft, setDraft] = useState(profile);
  const [shortcutValid, setShortcutValid] = useState(true);
  const [saveQueued, setSaveQueued] = useState(false);
  const saveQueuedRef = useRef(false);
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
  const platformPolicy = shortcutPlatformPolicy(draft.shortcut, platform);
  const platformError = platformPolicy.status === 'impossible' ? platformPolicy.message : undefined;
  return (
    <fieldset className="gesture-test">
      <legend>{create ? 'New custom profile' : profile.name}</legend>
      {builtInMetadata === null ? null : <p className="body-copy">{builtInMetadata.description}</p>}
      <Input
        label="Name"
        hint="Something you will recognise in this list."
        value={draft.name}
        maxLength={80}
        disabled={disabled}
        onChange={(event) => setDraft({ ...draft, name: event.currentTarget.value })}
      />
      <KeyboardShortcutInput
        label="Shortcut"
        shortcut={draft.shortcut}
        platform={platform}
        disabled={disabled}
        error={reservationError ?? conflictError ?? platformError}
        onChange={(shortcut) => setDraft({ ...draft, shortcut })}
        onCaptureValidityChange={setShortcutValid}
      />
      <Select
        label="What happens to your words"
        hint="Raw types what you said. Smart sends it to your AI service to be cleaned up first."
        value={draft.processingMode}
        disabled={disabled}
        onChange={(event) => {
          const processingMode = event.currentTarget.value;
          if (processingMode === 'raw' || processingMode === 'smart') {
            setDraft({ ...draft, processingMode });
          }
        }}
      >
        <option value="raw">Type it exactly as I said it</option>
        <option value="smart">Clean it up with AI</option>
      </Select>
      <TextArea
        label="Extra instructions for the AI (optional)"
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
        hint="Tell the AI what to do with your words — for example “make it sound formal” or “translate it into Spanish”. Used only when you choose to clean it up with AI."
      />
      <div className="provider-actions">
        <Button
          disabled={
            disabled ||
            saveQueued ||
            conflictingProfile !== undefined ||
            reserved ||
            platformPolicy.status === 'impossible' ||
            !shortcutValid ||
            draft.name.trim().length === 0 ||
            (!create && JSON.stringify(draft) === JSON.stringify(profile))
          }
          onClick={() => {
            if (saveQueuedRef.current) return;
            saveQueuedRef.current = true;
            setSaveQueued(true);
            const candidate = structuredClone(draft);
            void waitForShortcutCaptureRestorations()
              .then(() => onSave(candidate))
              .catch(() => undefined)
              .finally(() => {
                saveQueuedRef.current = false;
                setSaveQueued(false);
              });
          }}
        >
          {create ? 'Create profile' : 'Save profile'}
        </Button>
        {onReset === undefined ? null : (
          <Button variant="quiet" disabled={disabled} onClick={onReset}>
            Reset
          </Button>
        )}
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
  return `${candidate} is already used by ${conflictingProfile.name} (${existing}). Pick a different one.`;
}

function reservedConflictMessage(
  shortcut: Shortcut,
  ownerId: ReturnType<typeof reservedBindingOwner>,
  platform: string,
): string {
  const candidate = formatKeyboardShortcut(shortcut, platform);
  const binding = RESERVED_DICTATION_BINDINGS.find(({ ownerId: owner }) => owner === ownerId);
  if (binding === undefined || ownerId === null) {
    return `${candidate} is kept free for one of the shortcuts that come with Talking Quill. Pick a different one.`;
  }
  const ownerName = builtInDictationProfileName(ownerId);
  const reservedShortcut = formatKeyboardShortcut(binding.shortcut, platform);
  return `${candidate} is the original shortcut for ${ownerName} (${reservedShortcut}), which stays reserved for that profile. Pick a different one.`;
}
