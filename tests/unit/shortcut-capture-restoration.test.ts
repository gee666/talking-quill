// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import {
  restoreShortcutCaptureLease,
  retryShortcutCaptureRestorations,
} from '../../app/src/renderer/main/settings/shortcut-capture-restoration';
import {
  ShortcutCaptureLeaseIdSchema,
  type ShortcutCaptureLeaseId,
} from '../../app/src/shared/schemas/shortcut-capture';

const FIRST_LEASE = ShortcutCaptureLeaseIdSchema.parse('11111111-1111-4111-8111-111111111111');
const SECOND_LEASE = ShortcutCaptureLeaseIdSchema.parse('22222222-2222-4222-8222-222222222222');

describe('shortcut capture restoration manager', () => {
  it('does not wait on a restoration whose acquisition depends on the current retry barrier', async () => {
    let resolveFirst!: (leaseId: ShortcutCaptureLeaseId) => void;
    const firstAcquisition = new Promise<ShortcutCaptureLeaseId>((resolve) => {
      resolveFirst = resolve;
    });
    const stop = vi.fn<(leaseId: ShortcutCaptureLeaseId) => Promise<void>>(() => Promise.resolve());
    Object.defineProperty(window, 'talkingQuill', {
      configurable: true,
      value: { shortcutCapture: { stop } },
    });

    const firstRestoration = restoreShortcutCaptureLease(firstAcquisition);
    const secondAcquisition = retryShortcutCaptureRestorations().then(() => SECOND_LEASE);
    const secondRestoration = restoreShortcutCaptureLease(secondAcquisition);

    resolveFirst(FIRST_LEASE);
    await Promise.all([firstRestoration, secondRestoration]);

    expect(stop.mock.calls).toEqual([[FIRST_LEASE], [SECOND_LEASE]]);
  });
});
