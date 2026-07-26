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

export function DestinationSummary({
  destination,
  verified,
}: {
  readonly destination: Destination | null;
  readonly verified: boolean;
}) {
  return (
    <div className="provider-summary">
      <Status tone={destinationTone(destination)}>
        {destinationLabel(destination)} — {verified ? 'verified' : 'not yet verified'}
      </Status>
      {destination === 'cloud' || !verified ? (
        <p className="cloud-cost-note">
          Until verified, assume transcript data may leave this device and the provider may charge
          your account. Talking Quill does not add a fee.
        </p>
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
    <section className="connection-panel" aria-labelledby="pi-installation-heading">
      <div>
        <h3 id="pi-installation-heading">Pi installation path</h3>
        <p>
          Auto-detect searches PATH, %APPDATA%\npm, %PNPM_HOME%, and %LOCALAPPDATA%\pnpm. Example
          npm global shim: %APPDATA%\npm\pi.cmd.
        </p>
      </div>
      <Input
        label="Pi installation path"
        value={path}
        placeholder="C:\\Users\\you\\AppData\\Roaming\\npm\\pi.cmd"
        spellCheck={false}
        disabled={disabled || pathState === 'loading'}
        hint="Paste an absolute .cmd, .bat, .exe, extensionless executable, symlink, or containing directory. Talking Quill checks CLI capabilities, not package ownership or version."
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
          Locating Pi…
        </Status>
      ) : modelState === 'loading' ? (
        <Status tone="info" live>
          Reading Pi models… {formatOperationElapsed(modelElapsedMs)}
        </Status>
      ) : installation.state === 'ready' ? (
        <Status tone={modelState === 'error' ? 'error' : 'success'} live>
          Pi {installation.version} — {modelState === 'error' ? 'model read failed' : 'ready'} —{' '}
          {formatOperationElapsed(modelElapsedMs)}
        </Status>
      ) : installation.state === 'invalid' ? (
        <Status tone="error" live>
          The configured path is stale or invalid. Choose a valid Pi installation or select
          Auto-detect; Talking Quill will not silently use another Pi.
        </Status>
      ) : installation.state === 'incompatible' ? (
        <Status tone="error" live>
          This Pi command is missing required CLI capabilities. Update Pi or choose a different
          compatible executable.
        </Status>
      ) : installation.errorCode === 'PI_LAUNCH_FAILED' ? (
        <Status tone="error" live>
          Pi could not complete its bounded version and help checks. Retry or choose a different
          executable.
        </Status>
      ) : (
        <Status tone="warning" live>
          Pi was not found. Install it with npm install -g @earendil-works/pi-coding-agent, then
          select Auto-detect.
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
    <section className="credential-panel" aria-labelledby="provider-credential-heading">
      <div className="credential-panel__heading">
        <div>
          <h3 id="provider-credential-heading">{bedrock ? 'AWS credentials' : 'API key'}</h3>
          <p>Write-only: stored credentials can be replaced or deleted, but never read back.</p>
        </div>
        <Status tone={configured && !bindingDirty ? 'success' : 'neutral'} live>
          {configured && !bindingDirty ? 'Configured' : 'Not configured'}
        </Status>
      </div>
      {bindingDirty ? (
        <p className="operation-message operation-message--error" role="status">
          Provider destination changed. Save it, then re-enter credentials; old credentials will
          never be sent to the new destination.
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
            hint="All fields are cleared immediately and stored together in the encrypted vault."
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
          hint="The input is cleared immediately when submitted."
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
        {state === 'error' ? <Status tone="error">Credential action failed</Status> : null}
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
    <section className="connection-panel" aria-labelledby="connection-test-heading">
      <div>
        <h3 id="connection-test-heading">Connection test</h3>
        <p>
          {providerManagedModel
            ? 'Verifies endpoint safety and availability using the model currently loaded by the provider.'
            : 'Verifies authentication, endpoint safety, the selected model, and availability. For Pi, this sends a minimal fixed prompt to the selected model and may contact or charge its provider.'}
        </p>
      </div>
      <div className="provider-actions">
        <Button busy={state === 'loading'} disabled={disabled} onClick={onTest}>
          Test connection
        </Button>
        {!configurationDirty && missingModel ? (
          <Status tone="warning">Select and save a model before testing</Status>
        ) : null}
        {state === 'loading' ? (
          <Status tone="info" live>
            Testing selected model… {formatOperationElapsed(elapsedMs)}
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
    <section className="connection-panel" aria-labelledby="osa-heading">
      <div>
        <h3 id="osa-heading">On-Screen Awareness</h3>
        <p>
          Opt in to send one screenshot of the focused display with each Smart cleanup. Capture
          happens only after transcription and voice-command matching. The image is never reused.
        </p>
      </div>
      {capability === 'supported' ? (
        <Toggle
          checked={enabled}
          disabled={!controlsEnabled || screenPermission === 'denied'}
          onChange={(event) => onUpdate(event.currentTarget.checked)}
          label="Use the focused display for Smart context"
          hint="The display is downscaled to 1568 px maximum edge and encoded as JPEG quality 80."
        />
      ) : capability === 'unsupported' ? (
        <Status tone="neutral">The selected model does not accept images.</Status>
      ) : manualVisionAllowed ? (
        <div>
          <Status tone="warning">Vision support is unknown and remains off.</Status>
          <p>
            Generic endpoints vary. Only a successful live image-echo test can enable a manual
            override, bound to this endpoint, credential revision, and model.
          </p>
          <Button variant="secondary" disabled={!controlsEnabled} onClick={onBeginVisionTest}>
            Run disclosed image-echo test
          </Button>
        </div>
      ) : (
        <Status tone="neutral">Vision support is unknown; On-Screen Awareness stays off.</Status>
      )}
      {screenPermission === 'denied' ? (
        <p className="operation-message operation-message--error" role="status">
          Screen Recording is denied. On macOS, open System Settings → Privacy &amp; Security →
          Screen Recording, allow Talking Quill, then restart it.
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
      title="Live image-echo verification"
      description="This test captures the code below and sends that one screenshot to the configured provider. No image is retained. Success creates a narrowly bound manual vision override."
      onClose={onClose}
      actions={
        <>
          <Button variant="secondary" disabled={commitPending} onClick={onClose}>
            {commitPending ? 'Saving verification…' : state === 'success' ? 'Close' : 'Cancel'}
          </Button>
          <Button
            busy={state === 'loading'}
            disabled={!controlsEnabled || commitPending || state === 'success'}
            onClick={onVerify}
          >
            Capture and verify
          </Button>
        </>
      }
    >
      <p aria-label="Image echo verification code" className="vision-test-code">
        {nonce}
      </p>
      {state === 'success' ? (
        <Status tone="success" live>
          Verified. The override is bound to this exact configuration.
        </Status>
      ) : null}
      {state === 'error' ? (
        <Status tone="error" live>
          The model did not echo the visible code exactly. No override was saved.
        </Status>
      ) : null}
    </Dialog>
  );
}
