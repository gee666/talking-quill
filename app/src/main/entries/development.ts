import { app } from 'electron';
import { resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { isStrictPathChild } from '../app/runtime-path-policy';
import { startMain } from '../bootstrap';

const profileArgument = readArgument('--talking-quill-user-data=');
const profile =
  process.env.NODE_ENV === 'test' && profileArgument !== null ? profileArgument : undefined;
const visibleNonce = process.env.TALKING_QUILL_DEV_VISIBLE_NONCE;
if (visibleNonce !== undefined) {
  const visibleProfile = process.env.TALKING_QUILL_DEV_VISIBLE_PROFILE;
  if (
    app.isPackaged ||
    process.env.NODE_ENV !== 'development' ||
    !/^[0-9a-f]{32}$/u.test(visibleNonce) ||
    visibleProfile === undefined ||
    !isStrictPathChild(resolve(tmpdir()), resolve(visibleProfile))
  ) {
    throw new Error('Development visible-window check is unavailable');
  }
  app.on('browser-window-created', (_event, window) => {
    if (window.getTitle() !== 'Talking Quill') return;
    window.once('show', () => {
      setImmediate(() => {
        if (!window.isDestroyed() && window.isVisible()) {
          process.stdout.write(
            `TALKING_QUILL_DEV_VISIBLE:${visibleNonce}:${String(process.pid)}\n`,
          );
          app.quit();
        }
      });
    });
  });
  startMain({ userDataPath: resolve(visibleProfile) });
} else {
  startMain({
    ...(profile === undefined ? {} : { userDataPath: resolve(profile) }),
  });
}

function readArgument(prefix: string): string | null {
  const argument = process.argv.find((value) => value.startsWith(prefix));
  const value = argument?.slice(prefix.length).trim();
  return value === undefined || value.length === 0 ? null : value;
}
