import { useState } from 'react';
import {
  BuiltInDictationProfileIdSchema,
  MAX_DICTATION_PROFILES,
  isReservedBindingForProfile,
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../../shared/schemas/dictation-profiles';
import type { Settings } from '../../../shared/schemas/settings';
import {
  ShortcutKeySchema,
  shortcutFromLegacyActivation,
  shortcutsEqual,
  type Shortcut,
} from '../../../shared/schemas/shortcut';
import { Button, Card, Status } from '../../design';
import { publicErrorMessage } from '../public-error';
import { ProfileEditor } from './ProfileEditor';

export function DictationProfilesSection({
  settings,
  platform,
  onSettingsSaved,
  heading = 'Dictation profiles',
}: {
  readonly settings: Settings;
  readonly platform: string;
  readonly onSettingsSaved: (settings: Settings) => void;
  readonly heading?: string | null;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState('');
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
        'That profile couldn’t be saved. Give it a valid shortcut no other profile uses. The exact shortcuts that come with Talking Quill are always kept free for their built-in profiles.',
      );
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card {...(heading === null ? {} : { title: heading })}>
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
          onSave={(next) =>
            mutate(profile.id, () =>
              window.talkingQuill.profiles.update(profile.id, profilePatch(profile, next)),
            )
          }
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
        <div className="provider-actions">
          <Button
            disabled={busy || settings.dictationProfiles.length >= MAX_DICTATION_PROFILES}
            onClick={() => setCreating(true)}
          >
            Add custom profile
          </Button>
        </div>
      )}
      <div className="provider-actions">
        <Button
          variant="secondary"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError(null);
            setMessage('');
            try {
              const result = await window.talkingQuill.profiles.importFile();
              setCreating(false);
              setMessage(
                result.status === 'cancelled'
                  ? 'Import cancelled.'
                  : `Imported ${String(result.count)} dictation profiles.`,
              );
            } catch (transferError: unknown) {
              setError(
                publicErrorMessage(
                  transferError,
                  'Those dictation profiles couldn’t be imported. Check that every shortcut is valid and unique.',
                ),
              );
            } finally {
              setBusy(false);
            }
          }}
        >
          Import profiles
        </Button>
        <Button
          variant="secondary"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError(null);
            setMessage('');
            try {
              const result = await window.talkingQuill.profiles.exportFile();
              setMessage(
                result.status === 'cancelled'
                  ? 'Export cancelled.'
                  : `Exported ${String(result.count)} dictation profiles.`,
              );
            } catch (transferError: unknown) {
              setError(
                publicErrorMessage(transferError, 'Those dictation profiles couldn’t be exported.'),
              );
            } finally {
              setBusy(false);
            }
          }}
        >
          Export profiles
        </Button>
      </div>
      {error === null ? null : <Status tone="error">{error}</Status>}
      <p className="operation-message" role="status" aria-live="polite">
        {message}
      </p>
    </Card>
  );
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
      !profiles.some((profile) => shortcutsEqual(profile.shortcut, shortcut)) &&
      !isReservedBindingForProfile('custom', shortcut)
    ) {
      return shortcut;
    }
  }
  return shortcutFromLegacyActivation('A', false);
}
