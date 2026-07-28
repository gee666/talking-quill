import type { RefObject } from 'react';
import type { PiInstallationStatus } from '../../../shared/schemas/pi-installation';
import type {
  Destination,
  RunnableProviderId,
  VisionCapability,
} from '../../../shared/schemas/providers';
import { Button, Dialog, Input, Status, Toggle } from '../../design';
import {
  destinationLabel,
  destinationTone,
  formatOperationElapsed,
  type ConnectionState,
  type RequestState,
} from './provider-utils';

const CLOUD_COST_NOTE =
  'Until this is checked, assume your text may leave this computer. A cloud provider may charge you for what it processes. Talking Quill never adds a fee of its own.';

export function DestinationSummary({
  destination,
  providerName,
  verified,
}: {
  readonly destination: Destination | null;
  readonly providerName: string;
  readonly verified: boolean;
}) {
  return (
    <div className="provider-summary stack">
      <Status tone={destinationTone(destination)}>
        {destinationLabel(destination, providerName)} — {verified ? 'checked' : 'not checked yet'}
      </Status>
      {destination === 'cloud' || !verified ? (
        <p className="cloud-cost-note body-copy">{CLOUD_COST_NOTE}</p>
      ) : null}
    </div>
  );
}

export function PiInstallationPanel({
  path,
  pathState,
  disabled,
  installation,
  modelState,
  modelElapsedMs,
  onPathChange,
  onAction,
}: {
  readonly path: string;
  readonly pathState: RequestState;
  readonly disabled: boolean;
  readonly installation: PiInstallationStatus | null;
  readonly modelState: RequestState;
  readonly modelElapsedMs: number;
  readonly onPathChange: (path: string) => void;
  readonly onAction: (action: 'save' | 'browse' | 'automatic') => void;
}) {
  return (
    <section className="stack" aria-labelledby="pi-installation-heading">
      <h3 className="subhead" id="pi-installation-heading">
        Where Pi is installed
      </h3>
      <p className="body-copy">
        Talking Quill runs the Pi command that is already on your computer. If you know where it
        lives, point us at it. Otherwise let us look for it.
      </p>
      <Input
        label="Pi installation path"
        value={path}
        placeholder="C:\\Users\\you\\AppData\\Roaming\\npm\\pi.cmd"
        spellCheck={false}
        disabled={disabled || pathState === 'loading'}
        hint="A full path to the pi program, or the folder that holds it. Auto-detect also checks PATH, %APPDATA%\npm, %PNPM_HOME% and %LOCALAPPDATA%\pnpm."
        onChange={(event) => onPathChange(event.currentTarget.value)}
      />
      <div className="provider-actions">
        <Button
          variant="secondary"
          busy={pathState === 'loading'}
          disabled={disabled || path.trim().length === 0}
          onClick={() => onAction('save')}
        >
          Save path
        </Button>
        <Button
          variant="secondary"
          disabled={disabled || pathState === 'loading'}
          onClick={() => onAction('browse')}
        >
          Browse folder…
        </Button>
        <Button
          variant="quiet"
          disabled={disabled || pathState === 'loading'}
          onClick={() => onAction('automatic')}
        >
          Auto-detect
        </Button>
      </div>
      {installation === null ? (
        <Status tone="info" live>
          Looking for Pi…
        </Status>
      ) : modelState === 'loading' ? (
        <Status tone="info" live>
          Reading the models Pi offers… {formatOperationElapsed(modelElapsedMs)}
        </Status>
      ) : installation.state === 'ready' ? (
        <Status tone={modelState === 'error' ? 'error' : 'success'} live>
          Pi {installation.version} —{' '}
          {modelState === 'error' ? 'could not read its model list' : 'ready to use'} —{' '}
          {formatOperationElapsed(modelElapsedMs)}
        </Status>
      ) : installation.state === 'invalid' ? (
        <Status tone="error" live>
          Nothing usable is at that path any more. Pick a valid Pi program or use Auto-detect —
          Talking Quill will not quietly run a different one.
        </Status>
      ) : installation.state === 'incompatible' ? (
        <Status tone="error" live>
          This copy of Pi is too old for Talking Quill. Update Pi, or point us at a newer one.
        </Status>
      ) : installation.errorCode === 'PI_LAUNCH_FAILED' ? (
        <Status tone="error" live>
          Pi did not finish starting up. Try again, or pick a different Pi program.
        </Status>
      ) : (
        <Status tone="warning" live>
          We could not find Pi. Install it with npm install -g @earendil-works/pi-coding-agent, then
          choose Auto-detect.
        </Status>
      )}
    </section>
  );
}

export function CredentialPanel({
  providerId,
  configured,
  bindingDirty,
  state,
  dirty,
  disabled,
  accessKeyRef,
  secretRef,
  sessionTokenRef,
  onSave,
  onDelete,
}: {
  readonly providerId: RunnableProviderId;
  readonly configured: boolean;
  readonly bindingDirty: boolean;
  readonly state: RequestState;
  readonly dirty: boolean;
  readonly disabled: boolean;
  readonly accessKeyRef: RefObject<HTMLInputElement | null>;
  readonly secretRef: RefObject<HTMLInputElement | null>;
  readonly sessionTokenRef: RefObject<HTMLInputElement | null>;
  readonly onSave: () => void;
  readonly onDelete: () => void;
}) {
  const bedrock = providerId === 'bedrock';
  return (
    <section className="stack" aria-labelledby="provider-credential-heading">
      <div className="readiness-row">
        <h3 className="subhead" id="provider-credential-heading">
          {bedrock ? 'AWS credentials' : 'API key'}
        </h3>
        <Status tone={configured && !bindingDirty ? 'success' : 'neutral'} live>
          {configured && !bindingDirty ? 'Configured' : 'Not configured'}
        </Status>
      </div>
      <p className="body-copy">
        {bedrock
          ? 'Your AWS keys are stored securely on this computer. You can replace or remove them at any time, but we will never show them to you again.'
          : 'Your API key is stored securely on this computer. You can replace or remove it at any time, but we will never show it to you again.'}
      </p>
      {bindingDirty ? (
        <p className="operation-message operation-message--error" role="status">
          You changed where this service lives. Save that first, then enter the key again — the old
          key is never sent to a new address.
        </p>
      ) : null}
      {bedrock ? (
        <>
          <Input
            ref={accessKeyRef}
            label="AWS access key ID"
            type="password"
            minLength={16}
            autoComplete="off"
            spellCheck={false}
            disabled={disabled || state === 'loading' || dirty}
          />
          <Input
            ref={secretRef}
            label="AWS secret access key"
            type="password"
            minLength={16}
            autoComplete="off"
            spellCheck={false}
            disabled={disabled || state === 'loading' || dirty}
          />
          <Input
            ref={sessionTokenRef}
            label="AWS session token (optional)"
            type="password"
            minLength={16}
            autoComplete="off"
            spellCheck={false}
            disabled={disabled || state === 'loading' || dirty}
            hint="All three boxes are emptied the moment you save."
          />
        </>
      ) : (
        <Input
          ref={secretRef}
          label={configured && !bindingDirty ? 'Replacement API key' : 'API key'}
          type="password"
          minLength={8}
          autoComplete="off"
          spellCheck={false}
          disabled={disabled || state === 'loading' || dirty}
          hint="The box is emptied the moment you save."
        />
      )}
      <div className="provider-actions">
        <Button
          variant="secondary"
          busy={state === 'loading'}
          disabled={disabled || dirty}
          onClick={onSave}
        >
          {bedrock
            ? configured && !bindingDirty
              ? 'Replace AWS credentials'
              : 'Store AWS credentials'
            : configured && !bindingDirty
              ? 'Replace API key'
              : 'Store API key'}
        </Button>
        {configured ? (
          <Button
            variant="danger"
            disabled={disabled || state === 'loading' || dirty}
            onClick={onDelete}
          >
            {bedrock ? 'Delete AWS credentials' : 'Delete API key'}
          </Button>
        ) : null}
        {state === 'error' ? (
          <Status tone="error">That did not work. Check the key and try again.</Status>
        ) : null}
      </div>
    </section>
  );
}

export function ConnectionTestPanel({
  state,
  message,
  elapsedMs,
  disabled,
  configurationDirty,
  missingModel,
  providerManagedModel,
  onTest,
  onCancel,
}: {
  readonly state: ConnectionState;
  readonly message: string | null;
  readonly elapsedMs: number;
  readonly disabled: boolean;
  readonly configurationDirty: boolean;
  readonly missingModel: boolean;
  readonly providerManagedModel: boolean;
  readonly onTest: () => void;
  readonly onCancel: () => void;
}) {
  return (
    <section className="stack" aria-labelledby="connection-test-heading">
      <h3 className="subhead" id="connection-test-heading">
        Test connection
      </h3>
      <p className="body-copy">
        {providerManagedModel
          ? 'This sends one tiny message to the service using whichever model it has loaded, so you can see right away that everything works.'
          : 'This sends one tiny message to the model you picked, so you can see right away that the address, the key and the model all work. On a paid service it may cost a fraction of a cent.'}
      </p>
      <div className="provider-actions">
        <Button busy={state === 'loading'} disabled={disabled} onClick={onTest}>
          Test connection
        </Button>
        {!configurationDirty && missingModel ? (
          <Status tone="warning">Pick a model and save before testing</Status>
        ) : null}
        {state === 'loading' ? (
          <Status tone="info" live>
            Talking to the service… {formatOperationElapsed(elapsedMs)}
          </Status>
        ) : null}
        {state === 'loading' ? (
          <Button variant="secondary" onClick={onCancel}>
            Cancel test
          </Button>
        ) : null}
        {state === 'error' ? (
          <Button variant="secondary" onClick={onTest}>
            Retry test
          </Button>
        ) : null}
      </div>
      {message === null ? null : (
        <p
          className={`operation-message operation-message--${state}`}
          role="status"
          aria-live="polite"
        >
          {message}
        </p>
      )}
    </section>
  );
}

export function OnScreenAwarenessPanel({
  enabled,
  controlsEnabled,
  capability,
  manualVisionAllowed,
  screenPermission,
  onUpdate,
  onBeginVisionTest,
}: {
  readonly enabled: boolean;
  readonly controlsEnabled: boolean;
  readonly capability: VisionCapability;
  readonly manualVisionAllowed: boolean;
  readonly screenPermission: 'granted' | 'denied' | 'unknown';
  readonly onUpdate: (enabled: boolean) => void;
  readonly onBeginVisionTest: () => void;
}) {
  return (
    <section className="stack" aria-labelledby="osa-heading">
      <h3 className="subhead" id="osa-heading">
        Let the AI see your screen
      </h3>
      <p className="body-copy">
        Turn this on and one picture of the screen you are working on is sent along with each Smart
        clean-up, so the AI understands what you were talking about. The picture is taken after you
        stop speaking, used once, and never stored.
      </p>
      {capability === 'supported' ? (
        <Toggle
          checked={enabled}
          disabled={!controlsEnabled || screenPermission === 'denied'}
          onChange={(event) => onUpdate(event.currentTarget.checked)}
          label="Let the AI see your screen"
          hint="The picture is shrunk before it is sent."
        />
      ) : capability === 'unsupported' ? (
        <Status tone="neutral">The model you chose cannot look at pictures.</Status>
      ) : manualVisionAllowed ? (
        <div className="stack">
          <Status tone="warning">
            We cannot tell whether this model can see pictures, so this stays off.
          </Status>
          <p className="body-copy">
            Run a quick test: we show a short code on screen and check that the model reads it back.
            If it does, you can turn this on for this exact setup.
          </p>
          <Button variant="secondary" disabled={!controlsEnabled} onClick={onBeginVisionTest}>
            Run a quick screen test
          </Button>
        </div>
      ) : (
        <Status tone="neutral">
          We cannot tell whether this model can see pictures, so this stays off.
        </Status>
      )}
      {screenPermission === 'denied' ? (
        <p className="operation-message operation-message--error" role="status">
          Your Mac is blocking screen capture. Open System Settings → Privacy &amp; Security →
          Screen Recording, switch on Talking Quill, then restart the app.
        </p>
      ) : null}
    </section>
  );
}

export function VisionVerificationDialog({
  open,
  nonce,
  state,
  commitPending,
  controlsEnabled,
  onClose,
  onVerify,
}: {
  readonly open: boolean;
  readonly nonce: string;
  readonly state: RequestState;
  readonly commitPending: boolean;
  readonly controlsEnabled: boolean;
  readonly onClose: () => void;
  readonly onVerify: () => void;
}) {
  return (
    <Dialog
      open={open}
      title="Check that the AI can see your screen"
      description="We take one picture of your screen showing the code below and send that one picture to the AI service. Nothing is kept. If the code comes back correctly, you can switch this feature on for this exact setup."
      onClose={onClose}
      actions={
        <>
          <Button variant="secondary" disabled={commitPending} onClick={onClose}>
            {commitPending ? 'Saving…' : state === 'success' ? 'Close' : 'Cancel'}
          </Button>
          <Button
            busy={state === 'loading'}
            disabled={!controlsEnabled || commitPending || state === 'success'}
            onClick={onVerify}
          >
            Capture and check
          </Button>
        </>
      }
    >
      <p aria-label="Screen test code" className="vision-test-code">
        {nonce}
      </p>
      {state === 'success' ? (
        <Status tone="success" live>
          It worked. You can now let the AI see your screen with these settings.
        </Status>
      ) : null}
      {state === 'error' ? (
        <Status tone="error" live>
          The model did not read the code back correctly, so nothing changed.
        </Status>
      ) : null}
    </Dialog>
  );
}
