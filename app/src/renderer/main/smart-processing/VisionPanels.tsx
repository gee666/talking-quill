import type { VisionCapability } from '../../../shared/schemas/providers';
import { Button, Dialog, Status, Toggle } from '../../design';
import type { RequestState } from './provider-utils';

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
