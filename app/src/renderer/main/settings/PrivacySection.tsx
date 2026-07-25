import { useEffect, useState } from 'react';
import type { Settings } from '../../../shared/schemas/settings';
import { RESET_CONFIRMATION } from '../../../shared/schemas/data-lifecycle';
import { Button, Card, Dialog, Input, Select, Toggle } from '../../design';

export function PrivacySection({
  settings,
  disabled,
  onSave,
}: {
  readonly settings: Settings;
  readonly disabled: boolean;
  readonly onSave: (
    patch: Parameters<typeof window.talkingQuill.settings.update>[0],
    success: string,
  ) => Promise<void>;
}) {
  const [resetOpen, setResetOpen] = useState(false);
  const [confirmation, setConfirmation] = useState('');
  const [resetting, setResetting] = useState(false);
  const [resetAccepted, setResetAccepted] = useState(false);
  const [resetError, setResetError] = useState<string | null>(null);

  useEffect(
    () =>
      window.talkingQuill.data.onResetAccepted(() => {
        setResetAccepted(true);
      }),
    [],
  );

  const closeReset = () => {
    if (resetting) return;
    setResetOpen(false);
    setConfirmation('');
    setResetError(null);
    setResetAccepted(false);
  };

  const resetAll = async () => {
    if (confirmation !== RESET_CONFIRMATION) return;
    setResetting(true);
    setResetError(null);
    try {
      await window.talkingQuill.data.resetAll(RESET_CONFIRMATION);
      // The accepted event is sent before invoke resolution so it normally paints first. Treat a
      // successful invoke as the forced fallback when that event was dropped during relaunch.
      setResetAccepted(true);
    } catch {
      setResetError('The reset could not be prepared. No application data was removed.');
      setResetting(false);
    }
  };

  return (
    <Card title="Privacy & data" description="Control local history storage and retention.">
      <Toggle
        checked={settings.privacy.historyEnabled}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { privacy: { historyEnabled: event.currentTarget.checked } },
            'History preference saved.',
          )
        }
        label="Store completed dictations in the history"
        hint="Turning this off prevents future entries. Existing entries remain until you explicitly delete them."
      />
      <div className="setting-divider" />
      <Toggle
        checked={settings.privacy.retainSmartScreenshots}
        disabled={disabled || !settings.privacy.historyEnabled}
        onChange={(event) =>
          void onSave(
            { privacy: { retainSmartScreenshots: event.currentTarget.checked } },
            'Screenshot retention preference saved.',
          )
        }
        label="Retain On-Screen Awareness screenshots"
        hint="Off by default. When enabled, successful Smart entries keep an user-profile JPEG and thumbnail until that history entry is deleted. Screenshots are otherwise used once and never written to disk."
      />
      <div className="setting-divider" />
      <Select
        label="History retention"
        value={settings.privacy.historyRetentionDays?.toString() ?? 'off'}
        disabled={disabled}
        hint="Old entries are pruned locally when Talking Quill starts. Off keeps entries until you delete them."
        onChange={(event) => {
          const value = event.currentTarget.value;
          const days = value === 'off' ? null : Number(value);
          if (days !== null && days !== 7 && days !== 30 && days !== 90) return;
          void onSave({ privacy: { historyRetentionDays: days } }, 'Retention preference saved.');
        }}
      >
        <option value="off">Off</option>
        <option value="7">7 days</option>
        <option value="30">30 days</option>
        <option value="90">90 days</option>
      </Select>
      <div className="setting-divider" />
      <Toggle
        checked={settings.privacy.diagnosticLoggingEnabled}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { privacy: { diagnosticLoggingEnabled: event.currentTarget.checked } },
            event.currentTarget.checked
              ? 'Diagnostic logging enabled.'
              : 'Diagnostic logging disabled.',
          )
        }
        label="Diagnostic logging"
        hint="Off by default. Logs contain only bounded operational event codes, never audio, transcripts, screenshots, prompts, credentials, request headers, bodies, or provider responses."
      />
      <div className="setting-divider" />
      <div className="setting-action">
        <div>
          <strong>Reset all application data</strong>
          <p>
            Removes settings, history, credentials, downloaded Whisper models, screenshots, logs,
            and temporary files. It never removes Ollama or Ollama models.
          </p>
        </div>
        <Button
          variant="danger"
          disabled={disabled}
          onClick={() => {
            setConfirmation('');
            setResetError(null);
            setResetOpen(true);
          }}
        >
          Reset all application data
        </Button>
      </div>
      <Dialog
        open={resetOpen}
        title="Reset all application data?"
        description="Talking Quill will restart with first-run defaults. This cannot be undone."
        onClose={closeReset}
        actions={
          <>
            <Button variant="secondary" disabled={resetting} onClick={closeReset}>
              Cancel
            </Button>
            <Button
              variant="danger"
              busy={resetting}
              disabled={confirmation !== RESET_CONFIRMATION}
              onClick={() => void resetAll()}
            >
              Reset and restart
            </Button>
          </>
        }
      >
        <Input
          data-autofocus
          label={`Type ${RESET_CONFIRMATION} to confirm`}
          value={confirmation}
          disabled={resetting}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => setConfirmation(event.currentTarget.value)}
        />
        {resetAccepted ? (
          <p role="status" aria-live="assertive">
            Reset accepted. Talking Quill will now relaunch.
          </p>
        ) : null}
        {resetError === null ? null : <p role="alert">{resetError}</p>}
      </Dialog>
    </Card>
  );
}
