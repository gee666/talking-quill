import { describe, expect, it, vi } from 'vitest';
import { createHandlers, type HandlerDependencies } from '../../app/src/main/ipc/handlers';
import { invokeRegistry } from '../../app/src/shared/ipc/registry';
import { ShortcutCaptureLeaseIdSchema } from '../../app/src/shared/schemas/shortcut-capture';

const LEASE_ID = ShortcutCaptureLeaseIdSchema.parse('11111111-1111-4111-8111-111111111111');

const context = {
  webContentsId: 42,
  onDestroyed: () => () => undefined,
};

describe('shortcut capture lease IPC', () => {
  it('accepts only opaque lease capabilities on start responses and stop requests', () => {
    expect(invokeRegistry['shortcut-capture:start'].response.parse({ leaseId: LEASE_ID })).toEqual({
      leaseId: LEASE_ID,
    });
    expect(
      invokeRegistry['shortcut-capture:start'].response.safeParse({ accepted: true }).success,
    ).toBe(false);
    expect(invokeRegistry['shortcut-capture:stop'].request.parse({ leaseId: LEASE_ID })).toEqual({
      leaseId: LEASE_ID,
    });
    for (const request of [{}, { leaseId: 'predictable' }, { leaseId: LEASE_ID, extra: true }]) {
      expect(invokeRegistry['shortcut-capture:stop'].request.safeParse(request).success).toBe(
        false,
      );
    }
  });

  it('returns the main-minted lease and binds stop to its requesting renderer', async () => {
    const startShortcutCapture = vi.fn().mockResolvedValue(LEASE_ID);
    const stopShortcutCapture = vi.fn().mockResolvedValue(undefined);
    const handlers = createHandlers({
      echo: { startShortcutCapture, stopShortcutCapture },
    } as unknown as HandlerDependencies);

    await expect(handlers['shortcut-capture:start']({}, context)).resolves.toEqual({
      leaseId: LEASE_ID,
    });
    await expect(
      handlers['shortcut-capture:stop']({ leaseId: LEASE_ID }, context),
    ).resolves.toEqual({ accepted: true });

    expect(startShortcutCapture).toHaveBeenCalledWith(42, context.onDestroyed);
    expect(stopShortcutCapture).toHaveBeenCalledWith(42, LEASE_ID);
  });
});
