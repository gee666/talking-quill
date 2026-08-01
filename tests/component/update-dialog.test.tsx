// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { UpdateDialog } from '../../app/src/renderer/main/updates/UpdateDialog';
import type { ApplicationUpdateState } from '../../app/src/shared/schemas/info';

const available: ApplicationUpdateState = {
  phase: 'available',
  currentVersion: '1.0.0',
  availableVersion: '1.1.0',
  releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
  percent: null,
  message: null,
  revision: 1,
};

afterEach(() => cleanup());

describe('global update prompt', () => {
  it('requires consent before starting the Windows update', async () => {
    const applyUpdate = vi.fn(() =>
      Promise.resolve({ ...available, phase: 'downloading' as const, percent: 0, revision: 2 }),
    );
    installInfoApi({ updateState: () => Promise.resolve(available), applyUpdate });
    render(<UpdateDialog />);
    await screen.findByRole('heading', { name: 'Update available' });
    await userEvent.click(screen.getByRole('button', { name: 'Update now' }));
    expect(applyUpdate).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.getByText('Downloading update')).toBeInTheDocument());
  });

  it('offers the release page instead of automatic installation for unsigned macOS', async () => {
    const openRelease = vi.fn(() => Promise.resolve());
    installInfoApi({
      updateState: () =>
        Promise.resolve({
          ...available,
          phase: 'unsupported',
          message: 'Install this release manually from its GitHub release page.',
        }),
      openRelease,
    });
    render(<UpdateDialog />);
    await screen.findByText('Version 1.1.0 is available for manual installation.');
    expect(screen.queryByRole('button', { name: 'Update now' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Open release page' }));
    expect(openRelease).toHaveBeenCalledWith(available.releaseUrl);
  });
});

function installInfoApi(
  overrides: Partial<{
    updateState: () => Promise<ApplicationUpdateState>;
    applyUpdate: () => Promise<ApplicationUpdateState>;
    openRelease: (url: string) => Promise<void>;
  }>,
): void {
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: {
      info: {
        updateState: overrides.updateState ?? (() => Promise.resolve(available)),
        applyUpdate: overrides.applyUpdate ?? (() => Promise.resolve(available)),
        openRelease: overrides.openRelease ?? (() => Promise.resolve()),
        onUpdateChanged: () => () => undefined,
      },
    },
  });
}
