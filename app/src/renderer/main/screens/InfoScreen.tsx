/* eslint-disable jsx-a11y/no-noninteractive-tabindex -- the bounded notices document must receive keyboard scroll focus */
import { useEffect, useRef, useState, type RefObject } from 'react';
import type { BootstrapData } from '../../../shared/bridge/api';
import type { InfoStatus, UpdateCheckResult } from '../../../shared/schemas/info';
import { Button, Card, Dialog, Status, Toast } from '../../design';
import { publicErrorMessage } from '../public-error';

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
          setNotice('Talking Quill could not read your permission settings.');
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
      setNotice('Talking Quill could not read your permission settings.');
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
          publicErrorMessage(
            cause,
            'Talking Quill could not check for updates. Check your internet connection and try again.',
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
      setNotice('The list of open-source notices could not be loaded.');
      setNoticesOpen(false);
    }
  };
  return (
    <div className="screen">
      <header className="screen__header">
        <div>
          <p className="eyebrow">About</p>
          <h1 ref={headingRef} tabIndex={-1}>
            About Talking Quill
          </h1>
          <p>Your version, how your words are handled, and where your files live.</p>
        </div>
        <Status tone="info">
          Version {bootstrap.appVersion} · source {bootstrap.sourceRevision ?? 'development'}
        </Status>
      </header>
      <div className="screen__grid">
        <Card
          title="Version and updates"
          description="Talking Quill checks the public GitHub release feed when it starts. You can also check again here."
        >
          <div className="info-actions provider-actions">
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
              <p className="body-copy">You have not checked for updates yet.</p>
            ) : null
          ) : (
            <Status tone={update.status === 'available' ? 'info' : 'success'} live>
              {update.status === 'available'
                ? `Version ${update.latestVersion} is available`
                : `Version ${update.currentVersion} is current`}
            </Status>
          )}
          {update?.status === 'available' ? (
            <div className="info-actions">
              <Button
                variant="secondary"
                onClick={() =>
                  void runAction(
                    () => window.talkingQuill.info.openRelease(update.releaseUrl),
                    'The release page could not be opened in your browser.',
                  )
                }
              >
                Open release page
              </Button>
            </div>
          ) : null}
        </Card>
        <Card title="Raw and Smart dictation" description="Two ways to turn your voice into text.">
          <p className="body-copy">
            <strong>Raw</strong> writes down exactly what you said. It all happens on your computer
            and nothing is sent anywhere.
          </p>
          <p className="body-copy">
            <strong>Smart</strong> asks an AI service to tidy your words up first. Only your
            transcript, your word list, and — if you switch it on — a single screenshot are sent to
            the service you chose.
          </p>
          <p className="body-copy">
            <strong>Free to use — no account, no usage limits.</strong> If you pick a cloud AI
            service for Smart dictation, that cloud provider may charge you for its own service.
          </p>
        </Card>
        <Card
          title="Your privacy"
          description="What Talking Quill does, and does not, do with your words."
        >
          <p className="body-copy">
            Your recordings stay on your computer. Nothing about how you use the app is collected or
            sent anywhere. Extra logging stays off unless you turn it on, and you can switch off or
            clear your dictation history whenever you like.
          </p>
        </Card>
        <Card
          title="Permissions"
          description="Talking Quill needs your permission to hear you and to type for you. Open the right system screen from here if something is missing."
        >
          {permissionError ? (
            <div className="info-alert" role="alert">
              <p>Talking Quill could not read your permission settings.</p>
              <Button variant="secondary" onClick={() => void refreshPermissions()}>
                Try again
              </Button>
            </div>
          ) : permissions === null ? (
            <p className="body-copy" role="status">
              Checking your permissions…
            </p>
          ) : (
            <div className="permission-list">
              <div className="info-actions">
                <Button variant="secondary" onClick={() => void refreshPermissions()}>
                  Check again
                </Button>
              </div>
              <div className="group">
                <PermissionRow
                  label="Microphone"
                  value={permissions.microphone}
                  open={() => window.talkingQuill.info.openPermissionSettings('microphone')}
                  onError={() => setNotice('The microphone settings could not be opened.')}
                />
                {bootstrap.platform === 'darwin' ? (
                  <>
                    <PermissionRow
                      label="Accessibility"
                      value={permissions.helper.permissions.accessibility}
                      open={() => window.talkingQuill.info.openPermissionSettings('accessibility')}
                      onError={() => setNotice('The Accessibility settings could not be opened.')}
                    />
                    <PermissionRow
                      label="Input Monitoring"
                      value={permissions.helper.permissions.inputMonitoring}
                      open={() =>
                        window.talkingQuill.info.openPermissionSettings('input-monitoring')
                      }
                      onError={() =>
                        setNotice('The Input Monitoring settings could not be opened.')
                      }
                    />
                    <PermissionRow
                      label="Screen Recording"
                      value={permissions.screenRecording}
                      open={() =>
                        window.talkingQuill.info.openPermissionSettings('screen-recording')
                      }
                      onError={() =>
                        setNotice('The Screen Recording settings could not be opened.')
                      }
                    />
                  </>
                ) : null}
              </div>
            </div>
          )}
        </Card>
        <Card
          title="Help and your data"
          description="Walk through setup again, or open the folders where Talking Quill keeps your files."
        >
          <div className="info-actions provider-actions">
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
          <p className="hint">
            Your settings, your word lists and your history live in the data folder. The logs folder
            only holds diagnostic files.
          </p>
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
          {notices ?? 'Loading the notices…'}
        </pre>
      </Dialog>
      {notice === null ? null : (
        <Toast tone="error" message={notice} onDismiss={() => setNotice(null)} />
      )}
    </div>
  );
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
      <div className="info-actions provider-actions">
        <Status tone={ready ? 'success' : value === 'denied' ? 'error' : 'warning'}>
          {ready ? 'Allowed' : value === 'denied' ? 'Blocked' : 'Not decided yet'}
        </Status>
        {ready ? null : (
          <Button
            variant="quiet"
            aria-label={`Open ${label} settings`}
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
