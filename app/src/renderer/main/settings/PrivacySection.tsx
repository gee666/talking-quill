import { useEffect, useState } from 'react';
import type { Settings } from '../../../shared/schemas/settings';
import { RESET_CONFIRMATION } from '../../../shared/schemas/data-lifecycle';
import { Button, Card, Dialog, Input, Select, Toggle } from '../../design';

export function PrivacySection({
  settings,
  disabled,
  onSave,
  heading = 'Privacy & data',
}: {
  readonly settings: Settings;
  readonly disabled: boolean;
  readonly heading?: string | null;
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
  const [diagnosticExport, setDiagnosticExport] = useState<
    'idle' | 'exporting' | 'exported' | 'error'
  >('idle');
  const [logsFolderError, setLogsFolderError] = useState(false);

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
      setResetError('Nothing was deleted — the reset couldn’t start. Please try again.');
      setResetting(false);
    }
  };

  return (
    <Card
      {...(heading === null ? {} : { title: heading })}
      description="Your dictations stay on this computer. Here you decide what gets kept, and for how long."
    >
      <Toggle
        checked={settings.privacy.historyEnabled}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { privacy: { historyEnabled: event.currentTarget.checked } },
            'History preference saved.',
          )
        }
        label="Keep a history of what you dictated"
        hint="A list on this computer of the text you dictated, so you can copy something again later. Turn it off and nothing new is saved — what you already have stays until you delete it."
      />
      <Toggle
        checked={settings.privacy.retainSmartScreenshots}
        disabled={disabled || !settings.privacy.historyEnabled}
        onChange={(event) =>
          void onSave(
            { privacy: { retainSmartScreenshots: event.currentTarget.checked } },
            'Screenshot retention preference saved.',
          )
        }
        label="Keep the picture of your screen with each entry"
        hint="Smart mode can look at your screen to understand what you are working on. Off by default: the picture is used once and never saved. Turn it on to keep it with that history entry until you delete the entry."
      />
      <Select
        label="How long to keep history"
        value={settings.privacy.historyRetentionDays?.toString() ?? 'off'}
        disabled={disabled}
        hint="Older entries are deleted from this computer when Talking Quill starts."
        onChange={(event) => {
          const value = event.currentTarget.value;
          const days = value === 'off' ? null : Number(value);
          if (days !== null && days !== 7 && days !== 30 && days !== 90) return;
          void onSave({ privacy: { historyRetentionDays: days } }, 'Retention preference saved.');
        }}
      >
        <option value="off">Keep until I delete it</option>
        <option value="7">7 days</option>
        <option value="30">30 days</option>
        <option value="90">90 days</option>
      </Select>
      <Toggle
        checked={settings.privacy.diagnosticLoggingEnabled}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { privacy: { diagnosticLoggingEnabled: event.currentTarget.checked } },
            event.currentTarget.checked
              ? 'Detailed diagnostic logging enabled.'
              : 'Detailed diagnostic logging disabled. Failure codes remain enabled.',
          )
        }
        label="Debug logging"
        hint="Off by default. When off, Talking Quill keeps only one of four helper failure codes: startup unavailable, startup incompatible, runtime unavailable or runtime incompatible. Activation, startup, shutdown, helper lifecycle and owner connection details require debug logging. Logs never contain typed text, keys, audio, transcripts, models, voice commands, tokens or secrets. Up to four 256 KB files are kept."
      />
      <div className="setting-action">
        <div>
          <strong>Diagnostic logs</strong>
          <p className="body-copy">
            Open the per-user logs folder or export an allowlisted ZIP with a SHA-256 integrity
            manifest. The save dialog defaults to Downloads, not Desktop.
          </p>
          {diagnosticExport === 'exported' ? (
            <p role="status">Diagnostic report saved.</p>
          ) : diagnosticExport === 'error' ? (
            <p role="alert">The diagnostic report could not be saved.</p>
          ) : logsFolderError ? (
            <p role="alert">The logs folder could not be opened.</p>
          ) : null}
        </div>
        <div className="info-actions">
          <Button
            variant="secondary"
            disabled={disabled}
            onClick={() => {
              setLogsFolderError(false);
              void window.talkingQuill.info
                .openLocation('logs')
                .catch(() => setLogsFolderError(true));
            }}
          >
            Open logs folder
          </Button>
          <Button
            variant="secondary"
            busy={diagnosticExport === 'exporting'}
            disabled={disabled}
            onClick={() => {
              setDiagnosticExport('exporting');
              void window.talkingQuill.info.exportDiagnostics().then(
                (status) => setDiagnosticExport(status === 'exported' ? 'exported' : 'idle'),
                () => setDiagnosticExport('error'),
              );
            }}
          >
            Export diagnostic logs
          </Button>
        </div>
      </div>
      <div className="setting-divider" />
      <div className="setting-action">
        <div>
          <strong>Start over</strong>
          <p className="body-copy">
            Deletes everything Talking Quill keeps on this computer: your settings, your history,
            any saved keys, the downloaded speech model, screenshots and logs. Ollama and its models
            are left alone.
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
          Delete everything
        </Button>
      </div>
      <Dialog
        open={resetOpen}
        title="Delete everything and start over?"
        description="Your settings, history, saved keys and the downloaded speech model will be removed, and Talking Quill will restart as if it were brand new. This can’t be undone."
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
              Delete everything and restart
            </Button>
          </>
        }
      >
        <Input
          data-autofocus
          label={`Type ${RESET_CONFIRMATION} to confirm`}
          hint="This is here so it can’t happen by accident."
          value={confirmation}
          disabled={resetting}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => setConfirmation(event.currentTarget.value)}
        />
        {resetAccepted ? (
          <p role="status" aria-live="assertive">
            Confirmed. Talking Quill will restart now.
          </p>
        ) : null}
        {resetError === null ? null : <p role="alert">{resetError}</p>}
      </Dialog>
    </Card>
  );
}
