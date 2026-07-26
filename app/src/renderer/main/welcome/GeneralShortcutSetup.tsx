import { useState } from 'react';
import { ActivationKeySchema, type ActivationKey } from '../../../shared/helper/protocol';
import {
  GENERAL_PROFILE_ID,
  RESERVED_DICTATION_BINDING_ERROR,
  isReservedBindingForAnotherProfile,
} from '../../../shared/schemas/dictation-profiles';
import type { Settings } from '../../../shared/schemas/settings';
import { Button, Card, Select, Status } from '../../design';

export function GeneralShortcutSetup({
  settings,
  disabled,
  onSettingsSaved,
}: {
  readonly settings: Settings;
  readonly disabled: boolean;
  readonly onSettingsSaved: (settings: Settings) => void;
}) {
  const general = settings.dictationProfiles.find((profile) => profile.id === GENERAL_PROFILE_ID);
  const [activationKey, setActivationKey] = useState(general?.activationKey ?? 'Z');
  const [shift, setShift] = useState(general?.shift ?? false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (general === undefined) return null;

  const duplicate = settings.dictationProfiles.some(
    (profile) =>
      profile.id !== GENERAL_PROFILE_ID &&
      profile.activationKey === activationKey &&
      profile.shift === shift,
  );
  const reserved = isReservedBindingForAnotherProfile(GENERAL_PROFILE_ID, activationKey, shift);
  const unchanged = general.activationKey === activationKey && general.shift === shift;

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      onSettingsSaved(
        await window.talkingQuill.profiles.update(GENERAL_PROFILE_ID, { activationKey, shift }),
      );
    } catch {
      setError('The shortcut could not be saved. Choose a shortcut that is not already in use.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      title="General shortcut"
      description="Choose the shortcut you will use for the General profile, then test it below."
    >
      <div className="shortcut-setup">
        <Select
          label="Activation key"
          value={activationKey}
          disabled={disabled || busy}
          onChange={(event) => setActivationKey(event.currentTarget.value as ActivationKey)}
        >
          {ActivationKeySchema.options.map((key) => (
            <option key={key} value={key}>
              {key}
            </option>
          ))}
        </Select>
        <Select
          label="General Shift modifier"
          value={shift ? 'shift' : 'plain'}
          disabled={disabled || busy}
          onChange={(event) => setShift(event.currentTarget.value === 'shift')}
        >
          <option value="plain">Alt/Option only</option>
          <option value="shift">Alt/Option + Shift</option>
        </Select>
      </div>
      {duplicate ? <Status tone="error">That exact shortcut is already used.</Status> : null}
      {reserved ? <Status tone="error">{RESERVED_DICTATION_BINDING_ERROR}</Status> : null}
      {error === null ? null : <Status tone="error">{error}</Status>}
      <div className="shortcut-setup__actions">
        <Button
          busy={busy}
          disabled={disabled || busy || duplicate || reserved || unchanged}
          onClick={() => void save()}
        >
          Save General shortcut
        </Button>
      </div>
    </Card>
  );
}
