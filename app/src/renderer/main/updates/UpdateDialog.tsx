import { useEffect, useState } from 'react';
import type { ApplicationUpdateState } from '../../../shared/schemas/info';
import { Button, Dialog, Progress, Toast } from '../../design';

export function UpdateDialog() {
  const [update, setUpdate] = useState<ApplicationUpdateState | null>(null);
  const [dismissedRevision, setDismissedRevision] = useState(-1);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let newestRevision = -1;
    const apply = (state: ApplicationUpdateState) => {
      if (!active || state.revision < newestRevision) return;
      newestRevision = state.revision;
      setUpdate(state);
    };
    const unsubscribe = window.talkingQuill.info.onUpdateChanged(apply);
    void window.talkingQuill.info.updateState().then(apply, () => undefined);
    return () => {
      active = false;
      unsubscribe();
    };
  }, []);

  if (update === null) return null;
  const availableVersion = update.availableVersion;
  if (availableVersion === null) return null;
  const visible =
    update.revision > dismissedRevision &&
    ['available', 'downloading', 'installing', 'error', 'unsupported'].includes(update.phase);
  const close = () => setDismissedRevision(update.revision);
  const openRelease = async () => {
    if (update.releaseUrl === null) return;
    try {
      await window.talkingQuill.info.openRelease(update.releaseUrl);
    } catch {
      setNotice('The release page could not be opened in your browser.');
    }
  };
  const applyUpdate = async () => {
    try {
      setUpdate(await window.talkingQuill.info.applyUpdate());
    } catch {
      setNotice('Talking Quill could not start the update.');
    }
  };

  return (
    <>
      <Dialog
        open={visible}
        title={update.phase === 'error' ? 'Update could not be installed' : 'Update available'}
        description={
          update.phase === 'unsupported'
            ? `Version ${availableVersion} is available for manual installation.`
            : `Talking Quill ${availableVersion} is ready to install.`
        }
        onClose={close}
        actions={
          <>
            {update.phase === 'available' ? (
              <Button data-autofocus onClick={() => void applyUpdate()}>
                Update now
              </Button>
            ) : null}
            {update.releaseUrl === null ||
            update.phase === 'downloading' ||
            update.phase === 'installing' ? null : (
              <Button variant="secondary" onClick={() => void openRelease()}>
                Open release page
              </Button>
            )}
            {update.phase === 'downloading' || update.phase === 'installing' ? null : (
              <Button variant="quiet" onClick={close}>
                Later
              </Button>
            )}
          </>
        }
      >
        {update.phase === 'downloading' ? (
          <Progress label="Downloading update" value={update.percent ?? 0} />
        ) : update.phase === 'installing' ? (
          <p role="status">The download is complete. Talking Quill is restarting to install it…</p>
        ) : (
          <p>
            {update.message ??
              'The update will download in the background, then Talking Quill will restart and install it.'}
          </p>
        )}
      </Dialog>
      {notice === null ? null : (
        <Toast tone="error" message={notice} onDismiss={() => setNotice(null)} />
      )}
    </>
  );
}
