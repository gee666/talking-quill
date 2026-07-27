import { describe, expect, it, vi } from 'vitest';
import { createHandlers, type HandlerDependencies } from '../../app/src/main/ipc/handlers';
import { ProviderError } from '../../app/src/main/providers/errors';
import { ModelManagerError } from '../../app/src/main/transcription/errors';
import { authorizeIpc } from '../../app/src/main/security/ipc-authorization';
import { toPublicError } from '../../app/src/main/security/public-error';
import { shortcutFromLegacyActivation } from '../../app/src/shared/schemas/shortcut';
import {
  eventRegistry,
  invokeRegistry,
  portTransferRegistry,
  successResponseSchema,
} from '../../app/src/shared/ipc/registry';

const BINDING_TOKEN = '11111111-1111-4111-8111-111111111111';
const RESET_ACKNOWLEDGEMENT_TOKEN = '00000000-0000-4000-8000-000000000013';

describe('typed IPC registry', () => {
  it('requires strict request and response schemas for every channel', () => {
    expect(Object.keys(invokeRegistry)).toHaveLength(73);
    for (const [channel, contract] of Object.entries(invokeRegistry)) {
      expect(contract.roles.length, channel).toBeGreaterThan(0);
      expect(contract.request.safeParse({ unknown: true }).success, channel).toBe(false);
    }
    expect(invokeRegistry['app:set-enabled'].request.safeParse({ enabled: 'yes' }).success).toBe(
      false,
    );
    expect(
      invokeRegistry['profile:create'].request.safeParse({
        name: 'Reserved',
        shortcut: shortcutFromLegacyActivation('Z', false),
        processingMode: 'raw',
        smartPrompt: null,
      }).success,
    ).toBe(false);
    const profileUpdate = invokeRegistry['profile:update'].request;
    expect(
      profileUpdate.safeParse({
        id: '11111111-1111-4111-8111-111111111111',
        patch: { shortcut: { keys: ['Q'] } },
      }).success,
    ).toBe(false);
    expect(
      profileUpdate.safeParse({
        id: 'general',
        patch: { shortcut: { modifiers: { ctrl: false, alt: true, shift: true, meta: false } } },
      }).success,
    ).toBe(false);
    expect(
      profileUpdate.safeParse({
        id: 'general',
        patch: { shortcut: shortcutFromLegacyActivation('Z', true) },
      }).success,
    ).toBe(false);
    expect(
      profileUpdate.safeParse({
        id: 'general',
        patch: { shortcut: shortcutFromLegacyActivation('Q', true), name: 'Moved General' },
      }).success,
    ).toBe(true);
    expect(
      profileUpdate.safeParse({
        id: 'general',
        patch: { name: 'Renamed', smartPrompt: null },
      }).success,
    ).toBe(true);
    expect(
      successResponseSchema('app:set-enabled').safeParse({
        ok: true,
        data: { enabled: true, status: 'invalid' },
      }).success,
    ).toBe(false);
    expect(
      invokeRegistry['provider:list-models'].request.safeParse({
        providerId: 'anthropic',
        operationId: 'operation-123',
        refresh: true,
      }).success,
    ).toBe(true);
    expect(
      invokeRegistry['provider:config-save'].request.safeParse({
        config: { providerId: 'anthropic', modelId: 'claude-sonnet-4-6' },
      }).success,
    ).toBe(true);
    expect(
      invokeRegistry['history:list'].request.safeParse({ limit: 50, cursor: null }).success,
    ).toBe(true);
    expect(invokeRegistry['history:list'].request.safeParse({ limit: 51 }).success).toBe(false);
    expect(
      invokeRegistry['history:delete'].request.safeParse({ id: '../history.db' }).success,
    ).toBe(false);
    expect(invokeRegistry['data:reset-all'].roles).toEqual(['main']);
    expect(
      invokeRegistry['data:reset-all'].request.safeParse({ confirmation: 'RESET TALKING QUILL' })
        .success,
    ).toBe(true);
    expect(
      invokeRegistry['data:reset-all'].request.safeParse({ confirmation: 'reset', extra: true })
        .success,
    ).toBe(false);
    expect(invokeRegistry['history:delete-all'].roles).toEqual(['main']);
    expect(invokeRegistry['history:copy'].roles).toEqual(['main']);
    expect(
      invokeRegistry['commands:create'].request.safeParse({
        trigger: 'go',
        snippet: 'text',
        path: 'C:/secret',
      }).success,
    ).toBe(false);
    expect(
      invokeRegistry['vocabulary:import-file'].request.safeParse({ path: 'C:/secret.txt' }).success,
    ).toBe(false);
    expect(
      invokeRegistry['provider:secret-set'].request.safeParse({
        providerId: 'openai',
        expectedBindingToken: BINDING_TOKEN,
        secret: 'safe-input',
        vaultId: 'renderer-controlled',
      }).success,
    ).toBe(false);
    for (const secret of ['', ' ', ' padded', 'padded ', 'line\rbreak', 'line\nbreak']) {
      expect(
        invokeRegistry['provider:secret-set'].request.safeParse({
          providerId: 'openai',
          expectedBindingToken: BINDING_TOKEN,
          secret,
        }).success,
        secret,
      ).toBe(false);
    }
    expect(
      invokeRegistry['provider:secret-set'].request.safeParse({
        providerId: 'openai',
        expectedBindingToken: BINDING_TOKEN,
        secret: 'trimmed-nonblank',
      }).success,
    ).toBe(true);
    expect(
      invokeRegistry['provider:secret-set'].request.safeParse({
        providerId: 'openai',
        secret: 'trimmed-nonblank',
      }).success,
    ).toBe(false);
    expect(
      invokeRegistry['provider:secret-delete'].request.safeParse({
        providerId: 'openai',
        expectedBindingToken: 'not-a-token',
      }).success,
    ).toBe(false);
    expect(
      invokeRegistry['provider:secret-status'].response.safeParse({
        providerId: 'openai',
        configured: false,
        updatedAt: null,
        bindingToken: BINDING_TOKEN,
        secret: 'must-not-cross-ipc',
      }).success,
    ).toBe(false);
    expect(Object.keys(eventRegistry)).toEqual([
      'data:reset-accepted',
      'activation-test:changed',
      'app:state-changed',
      'settings:changed',
      'history:changed',
      'model:progress',
      'window:maximized-changed',
      'recording:devices-changed',
      'recording:test-level',
      'recording:test-state-changed',
      'echo:session-changed',
    ]);
    expect(portTransferRegistry['capture:port'].roles).toEqual(['capture']);
    expect(
      portTransferRegistry['capture:port'].descriptor.safeParse({ protocolVersion: 2 }).success,
    ).toBe(false);
  });

  it('prepares reset recovery before acknowledging the destructive request', async () => {
    const requestDataReset = vi.fn().mockResolvedValue(RESET_ACKNOWLEDGEMENT_TOKEN);
    const acknowledgeDataReset = vi.fn();
    const handlers = createHandlers({
      requestDataReset,
      acknowledgeDataReset,
    } as unknown as HandlerDependencies);
    await expect(
      handlers['data:reset-all'](
        { confirmation: 'RESET TALKING QUILL' },
        { webContentsId: 1, onDestroyed: () => () => undefined },
      ),
    ).resolves.toEqual({
      accepted: true,
      acknowledgementToken: RESET_ACKNOWLEDGEMENT_TOKEN,
    });
    expect(
      handlers['data:reset-renderer-ack'](
        { acknowledgementToken: RESET_ACKNOWLEDGEMENT_TOKEN },
        { webContentsId: 1, onDestroyed: () => () => undefined },
      ),
    ).toEqual({ accepted: true });
    expect(requestDataReset).toHaveBeenCalledOnce();
    expect(acknowledgeDataReset).toHaveBeenCalledWith(RESET_ACKNOWLEDGEMENT_TOKEN);
  });

  it('forwards renderer binding tokens through credential mutation handlers', async () => {
    const setSecret = vi.fn().mockResolvedValue({
      providerId: 'openai',
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    });
    const deleteSecret = vi.fn().mockResolvedValue({
      providerId: 'openai',
      configured: false,
      updatedAt: null,
      bindingToken: BINDING_TOKEN,
    });
    const handlers = createHandlers({
      providerMutations: { setSecret, deleteSecret },
    } as unknown as HandlerDependencies);
    const context = {
      webContentsId: 1,
      onDestroyed: () => () => undefined,
    };

    await handlers['provider:secret-set'](
      {
        providerId: 'openai',
        expectedBindingToken: BINDING_TOKEN,
        secret: 'write-only-secret',
      },
      context,
    );
    await handlers['provider:secret-delete'](
      { providerId: 'openai', expectedBindingToken: BINDING_TOKEN },
      context,
    );

    expect(setSecret).toHaveBeenCalledWith('openai', BINDING_TOKEN, 'write-only-secret');
    expect(deleteSecret).toHaveBeenCalledWith('openai', BINDING_TOKEN);
  });

  it('returns a typed in-use outcome instead of deleting the active transcription model', async () => {
    const status = {
      modelId: 'Xenova/whisper-small' as const,
      state: 'ready' as const,
      downloadedBytes: 10,
      totalBytes: 10,
      detail: null,
      repairable: false,
    };
    const remove = vi.fn().mockResolvedValue({ outcome: 'in-use', status });
    const handlers = createHandlers({
      models: { deleteIfIdle: remove },
    } as unknown as HandlerDependencies);

    await expect(
      handlers['model:delete'](
        { modelId: 'Xenova/whisper-small' },
        { webContentsId: 1, onDestroyed: () => () => undefined },
      ),
    ).resolves.toEqual({ outcome: 'in-use', status });
    expect(remove).toHaveBeenCalledWith('Xenova/whisper-small');
  });

  it.each([
    {
      registeredRole: null,
      allowedRoles: ['main'] as const,
      isMainFrame: true,
      frameUrl: 'talking-quill://app/main/index.html',
      expectedUrl: null,
    },
    {
      registeredRole: 'widget' as const,
      allowedRoles: ['main'] as const,
      isMainFrame: true,
      frameUrl: 'talking-quill://app/widget/index.html',
      expectedUrl: 'talking-quill://app/widget/index.html',
    },
    {
      registeredRole: 'main' as const,
      allowedRoles: ['main'] as const,
      isMainFrame: false,
      frameUrl: 'talking-quill://app/main/index.html',
      expectedUrl: 'talking-quill://app/main/index.html',
    },
    {
      registeredRole: 'main' as const,
      allowedRoles: ['main'] as const,
      isMainFrame: true,
      frameUrl: 'https://evil.invalid',
      expectedUrl: 'talking-quill://app/main/index.html',
    },
  ])('denies unknown, wrong-role, subframe, and URL-mismatched senders', (input) => {
    expect(() => authorizeIpc(input)).toThrow('not authorized');
  });

  it('authorizes only the registered main frame and sanitizes failures', () => {
    expect(
      authorizeIpc({
        registeredRole: 'main',
        allowedRoles: ['main'],
        isMainFrame: true,
        frameUrl: 'talking-quill://app/main/index.html',
        expectedUrl: 'talking-quill://app/main/index.html',
      }),
    ).toBe('main');
    const error = toPublicError(new Error('secret stack and token'));
    expect(error).toEqual({ code: 'INTERNAL', message: 'The operation could not be completed.' });
    expect(JSON.stringify(error)).not.toContain('secret');
    expect(toPublicError(new ProviderError('AUTHENTICATION_FAILED'))).toEqual({
      code: 'AUTHENTICATION_FAILED',
      message: 'The provider rejected the credential.',
    });
    expect(
      toPublicError(new ModelManagerError('CANCELLED', 'private cancellation detail')),
    ).toEqual({
      code: 'CANCELLED',
      message: 'The operation was cancelled.',
    });
  });
});
