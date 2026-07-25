/* eslint-disable jsx-a11y/no-noninteractive-tabindex -- the bounded notices document must receive keyboard scroll focus */
import { useEffect, useRef, useState, type RefObject } from 'react';
import type { BootstrapData } from '../../../shared/bridge/api';
import type { InfoStatus, UpdateCheckResult } from '../../../shared/schemas/info';
import { Button, Card, Dialog, Status, Toast } from '../../design';

let updateSequence = 0;
export function InfoScreen({
  headingRef,
  bootstrap,
  onOpenWelcome,
}: {
  readonly headingRef: RefObject<HTMLHeadingElement | null>;
  readonly bootstrap: BootstrapData;
  readonly onOpenWelcome: () => void;
}) {
  const [permissions, setPermissions] = useState<InfoStatus | null>(null);
  const [permissionError, setPermissionError] = useState(false);
  const [update, setUpdate] = useState<UpdateCheckResult | null>(null);
  const [updateFeedback, setUpdateFeedback] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [notices, setNotices] = useState<string | null>(null);
  const [noticesOpen, setNoticesOpen] = useState(false);
  const operation = useRef<string | null>(null);
  useEffect(() => {
    let active = true;
    void window.talkingQuill.info.status().then(
      (status) => {
        if (active) setPermissions(status);
      },
      () => {
        if (active) {
          setPermissionError(true);
          setNotice('Permission status could not be refreshed.');
        }
      },
    );
    return () => {
      active = false;
      if (operation.current !== null) void window.talkingQuill.info.cancel(operation.current);
    };
  }, []);
  const refreshPermissions = async () => {
    setPermissionError(false);
    try {
      setPermissions(await window.talkingQuill.info.status());
    } catch {
      setPermissionError(true);
      setNotice('Permission status could not be refreshed.');
    }
  };
  const runAction = async (action: () => Promise<void>, failure: string) => {
    try {
      await action();
    } catch {
      setNotice(failure);
    }
  };
  const check = async () => {
    const id = `info-update-${String(++updateSequence)}`;
    operation.current = id;
    setChecking(true);
    setNotice(null);
    setUpdate(null);
    setUpdateFeedback(null);
    try {
      const result = await window.talkingQuill.info.checkForUpdates(id);
      if (operation.current === id) setUpdate(result);
    } catch (cause: unknown) {
      if (operation.current === id) {
        setUpdate(null);
        setNotice(
          actionableMessage(
            cause,
            'Updates could not be checked. Verify the network connection and try again.',
          ),
        );
      }
    } finally {
      if (operation.current === id) {
        operation.current = null;
        setChecking(false);
      }
    }
  };
  const showNotices = async () => {
    setNoticesOpen(true);
    if (notices !== null) return;
    try {
      setNotices(await window.talkingQuill.info.notices());
    } catch {
      setNotice('Third-party notices could not be loaded.');
      setNoticesOpen(false);
    }
  };
  return (
    <div className="screen">
      <header className="screen__header">
        <div>
          <p className="eyebrow">Info</p>
          <h1 ref={headingRef} tabIndex={-1}>
            About Talking Quill
          </h1>
          <p>Version, privacy, permissions, updates, and local application data.</p>
        </div>
        <Status tone="info">
          Version {bootstrap.appVersion} · source {bootstrap.sourceRevision ?? 'development'}
        </Status>
      </header>
      <div className="screen__grid">
        <Card title="Updates" description="Checks only when you ask; no background update traffic.">
          <div className="provider-actions">
            <Button busy={checking} onClick={() => void check()}>
              Check for updates
            </Button>
            {checking ? (
              <Button
                variant="secondary"
                onClick={() => {
                  const id = operation.current;
                  operation.current = null;
                  if (id !== null) void window.talkingQuill.info.cancel(id);
                  setChecking(false);
                  setUpdate(null);
                  setUpdateFeedback('Update check cancelled');
                }}
              >
                Cancel
              </Button>
            ) : null}
          </div>
          {updateFeedback === null ? null : (
            <Status tone="neutral" live>
              {updateFeedback}
            </Status>
          )}
          {update === null ? (
            updateFeedback === null ? (
              <p className="body-copy">No update check has been made.</p>
            ) : null
          ) : (
            <Status tone={update.status === 'available' ? 'info' : 'success'} live>
              {update.status === 'available'
                ? `Version ${update.latestVersion} is available`
                : `Version ${update.currentVersion} is current`}
            </Status>
          )}
          {update?.status === 'available' ? (
            <Button
              variant="secondary"
              onClick={() =>
                void runAction(
                  () => window.talkingQuill.info.openRelease(update.releaseUrl),
                  'The release page could not be opened.',
                )
              }
            >
              Open release page
            </Button>
          ) : null}
        </Card>
        <Card title="Raw and Smart transcription">
          <p className="body-copy">
            <strong>Raw</strong> transcribes locally and performs no provider request.{' '}
            <strong>Smart</strong> sends the transcript, custom vocabulary, and an optional
            one-session screenshot only to the provider you configure.
          </p>
          <p className="body-copy">
            Free to use — no account required and no usage limits. A cloud provider may charge for
            its own service.
          </p>
        </Card>
        <Card title="Privacy">
          <p className="body-copy">
            Audio stays on this device. There is no telemetry. Diagnostic logging is off by default
            and Past Echoes can be disabled or deleted.
          </p>
        </Card>
        <Card
          title="Permissions"
          description="Open the relevant operating-system pane when access needs attention."
        >
          {permissionError ? (
            <div role="alert">
              <p>Permission status is unavailable.</p>
              <Button variant="secondary" onClick={() => void refreshPermissions()}>
                Retry permission check
              </Button>
            </div>
          ) : permissions === null ? (
            <p role="status">Checking permissions…</p>
          ) : (
            <div className="permission-list">
              <Button variant="secondary" onClick={() => void refreshPermissions()}>
                Refresh permission status
              </Button>
              <PermissionRow
                label="Microphone"
                value={permissions.microphone}
                open={() => window.talkingQuill.info.openPermissionSettings('microphone')}
                onError={() => setNotice('Microphone settings could not be opened.')}
              />
              {bootstrap.platform === 'darwin' ? (
                <>
                  <PermissionRow
                    label="Accessibility"
                    value={permissions.helper.permissions.accessibility}
                    open={() => window.talkingQuill.info.openPermissionSettings('accessibility')}
                    onError={() => setNotice('Accessibility settings could not be opened.')}
                  />
                  <PermissionRow
                    label="Input Monitoring"
                    value={permissions.helper.permissions.inputMonitoring}
                    open={() => window.talkingQuill.info.openPermissionSettings('input-monitoring')}
                    onError={() => setNotice('Input Monitoring settings could not be opened.')}
                  />
                  <PermissionRow
                    label="Screen Recording"
                    value={permissions.screenRecording}
                    open={() => window.talkingQuill.info.openPermissionSettings('screen-recording')}
                    onError={() => setNotice('Screen Recording settings could not be opened.')}
                  />
                </>
              ) : null}
            </div>
          )}
        </Card>
        <Card title="Help and local data">
          <div className="provider-actions">
            <Button id="reopen-welcome" variant="secondary" onClick={onOpenWelcome}>
              Reopen Welcome
            </Button>
            <Button
              variant="secondary"
              onClick={() =>
                void runAction(
                  () => window.talkingQuill.info.openLocation('data'),
                  'The data folder could not be opened.',
                )
              }
            >
              Open data folder
            </Button>
            <Button
              variant="secondary"
              onClick={() =>
                void runAction(
                  () => window.talkingQuill.info.openLocation('logs'),
                  'The logs folder could not be opened.',
                )
              }
            >
              Open logs folder
            </Button>
            <Button variant="secondary" onClick={() => void showNotices()}>
              Third-party notices
            </Button>
          </div>
        </Card>
      </div>
      <Dialog
        open={noticesOpen}
        title="Third-party notices"
        onClose={() => setNoticesOpen(false)}
        actions={<Button onClick={() => setNoticesOpen(false)}>Close</Button>}
      >
        <pre
          className="notices-text"
          role="document"
          tabIndex={0}
          aria-label="Third-party notices text"
        >
          {notices ?? 'Loading notices…'}
        </pre>
      </Dialog>
      {notice === null ? null : (
        <Toast tone="error" message={notice} onDismiss={() => setNotice(null)} />
      )}
    </div>
  );
}
function actionableMessage(cause: unknown, fallback: string): string {
  if (cause instanceof Error && cause.message.length > 0 && cause.message.length <= 240) {
    return cause.message;
  }
  return fallback;
}

function PermissionRow({
  label,
  value,
  open,
  onError,
}: {
  readonly label: string;
  readonly value: string;
  readonly open: () => Promise<void>;
  readonly onError: () => void;
}) {
  const ready = value === 'granted' || value === 'not_applicable';
  return (
    <div className="readiness-row">
      <span>{label}</span>
      <div className="provider-actions">
        <Status tone={ready ? 'success' : value === 'denied' ? 'error' : 'warning'}>
          {ready ? 'Allowed' : value === 'denied' ? 'Denied' : 'Needs review'}
        </Status>
        {ready ? null : (
          <Button
            variant="quiet"
            onClick={() => {
              void open().catch(onError);
            }}
          >
            Open settings
          </Button>
        )}
      </div>
    </div>
  );
}
