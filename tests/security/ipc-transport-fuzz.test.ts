import type { IpcMainInvokeEvent, WebContents } from 'electron';
import { describe, expect, it } from 'vitest';
import { WindowRoleRegistry } from '../../app/src/main/app/window-role-registry';
import {
  registerIpcTransport,
  type IpcMainRegistrar,
  type IpcTransportLifecycle,
} from '../../app/src/main/ipc/transport';
import type { InvokeHandlerMap } from '../../app/src/main/ipc/types';
import {
  eventRegistry,
  invokeRegistry,
  portTransferRegistry,
  type InvokeChannel,
} from '../../app/src/shared/ipc/registry';
import type { WindowRole } from '../../app/src/shared/constants/app';
import { VALID_INVOKE_REQUESTS } from '../fixtures/ipc-valid-requests';

const RESET_ACKNOWLEDGEMENT_TOKEN = '00000000-0000-4000-8000-000000000013';

const ROLE_URL: Readonly<Record<WindowRole, string>> = Object.freeze({
  main: 'talking-quill://app/main/index.html',
  widget: 'talking-quill://app/widget/index.html',
  capture: 'talking-quill://app/capture/index.html',
});

class FakeIpcMain implements IpcMainRegistrar {
  readonly listeners = new Map<
    string,
    (event: IpcMainInvokeEvent, input: unknown) => Promise<unknown>
  >();

  handle(
    channel: string,
    listener: (event: IpcMainInvokeEvent, input: unknown) => Promise<unknown>,
  ): void {
    if (this.listeners.has(channel)) throw new Error(`duplicate channel ${channel}`);
    this.listeners.set(channel, listener);
  }

  removeHandler(channel: string): void {
    this.listeners.delete(channel);
  }
}

function registerRole(registry: WindowRoleRegistry, id: number, role: WindowRole) {
  const destroyedListeners: (() => void)[] = [];
  const webContents = {
    id,
    once: (_event: string, listener: () => void) => destroyedListeners.push(listener),
  } as unknown as WebContents;
  registry.register(webContents, role, ROLE_URL[role]);
}

function eventFor(
  id: number,
  url: string,
  options: {
    readonly subframe?: boolean;
    readonly destroyed?: boolean;
    readonly send?: (channel: string, payload: unknown) => void;
  } = {},
): IpcMainInvokeEvent {
  const frame = { url };
  const sender = {
    id,
    mainFrame: options.subframe === true ? { url } : frame,
    isDestroyed: () => options.destroyed === true,
    once: () => undefined,
    off: () => undefined,
    send: options.send ?? (() => undefined),
  };
  return { sender, senderFrame: frame } as unknown as IpcMainInvokeEvent;
}

function seededMalformedRequests(valid: object, seed: number): readonly unknown[] {
  let state = seed >>> 0;
  const next = () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return state >>> 0;
  };
  const malformed: unknown[] = [null, [], true, false, 0, -1, '', 'invalid', [valid]];
  for (let index = 0; index < 24; index += 1) {
    const suffix = `${next().toString(16)}-${String(index)}`;
    const hostile = [
      null,
      'x'.repeat(4_096 + (next() % 512)),
      Number.MAX_SAFE_INTEGER + 1,
      { nested: { unknown: suffix } },
      [`../${suffix}`, `https://169.254.169.254/${suffix}`],
    ][next() % 5];
    malformed.push({ ...structuredClone(valid), [`__unknown_${suffix}`]: hostile });
  }
  return malformed;
}

function fieldLevelMutations(valid: object, seed: number): readonly unknown[] {
  const mutations: unknown[] = [];
  const paths: (readonly string[])[] = [];
  const collect = (value: unknown, path: readonly string[]) => {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return;
    const entries = Object.entries(value);
    if (entries.length === 0) paths.push(path);
    for (const [key, nested] of entries) {
      const child = [...path, key];
      paths.push(child);
      collect(nested, child);
    }
  };
  collect(valid, []);
  const replace = (path: readonly string[], replacement: unknown, remove = false) => {
    if (path.length === 0) return replacement;
    const copy = structuredClone(valid) as Record<string, unknown>;
    let parent = copy;
    for (const segment of path.slice(0, -1)) {
      parent = parent[segment] as Record<string, unknown>;
    }
    const key = path.at(-1);
    if (key !== undefined) {
      if (remove) Reflect.deleteProperty(parent, key);
      else parent[key] = replacement;
    }
    return copy;
  };
  for (const path of paths) {
    mutations.push(
      replace(path, undefined, true),
      replace(path, { wrongType: true }),
      replace(path, null),
      replace(path, []),
      replace(path, 'x'.repeat(8_192)),
      replace(path, Number.MAX_SAFE_INTEGER + 1),
    );
    const current = path.reduce<unknown>(
      (value, segment) => (value as Record<string, unknown>)[segment],
      valid,
    );
    if (typeof current === 'object' && current !== null && !Array.isArray(current)) {
      mutations.push(replace(path, { ...current, __nestedUnknown: seed }));
    }
  }
  let state = seed >>> 0;
  return mutations
    .map((value) => {
      state = (state * 1_664_525 + 1_013_904_223) >>> 0;
      return { value, order: state };
    })
    .sort((left, right) => left.order - right.order)
    .map(({ value }) => value);
}

function alternateRole(allowed: readonly WindowRole[]): WindowRole {
  return (
    (['main', 'widget', 'capture'] as const).find((role) => !allowed.includes(role)) ??
    (allowed[0] === 'main' ? 'widget' : 'main')
  );
}

describe('registry-complete IPC transport fuzzing', () => {
  it('seed-fuzzes every event payload and port descriptor contract and inventories roles', () => {
    const arbitrary = [
      null,
      undefined,
      true,
      13,
      'x'.repeat(8_192),
      [],
      { unknown: true },
      { protocolVersion: 2 },
      { path: 'C:/renderer-controlled' },
    ];
    for (const [channel, contract] of Object.entries(eventRegistry)) {
      expect(contract.roles.length, channel).toBeGreaterThan(0);
      for (const [caseIndex, value] of arbitrary.entries()) {
        expect(
          contract.payload.safeParse(value).success,
          `${channel}:case-${String(caseIndex)}`,
        ).toBe(false);
      }
    }
    for (const [channel, contract] of Object.entries(portTransferRegistry)) {
      expect(contract.roles, channel).toEqual(['capture']);
      expect(contract.descriptor.safeParse({ protocolVersion: 1 }).success, channel).toBe(true);
      for (const [caseIndex, value] of arbitrary.entries()) {
        expect(
          contract.descriptor.safeParse(value).success,
          `${channel}:case-${String(caseIndex)}`,
        ).toBe(false);
      }
    }
  });

  it('has one compile-time fixture that validates for every invoke channel', () => {
    expect(Object.keys(VALID_INVOKE_REQUESTS).sort()).toEqual(Object.keys(invokeRegistry).sort());
    for (const channel of Object.keys(invokeRegistry) as InvokeChannel[]) {
      expect(
        invokeRegistry[channel].request.safeParse(VALID_INVOKE_REQUESTS[channel]).success,
        channel,
      ).toBe(true);
    }
  });

  it('accepts a valid fixture through the actual registered transport success path', async () => {
    const registrar = new FakeIpcMain();
    const roles = new WindowRoleRegistry();
    registerRole(roles, 7, 'main');
    const handlers = new Proxy({}, { get: () => () => ({ accepted: true }) }) as InvokeHandlerMap;
    const lifecycle = registerIpcTransport(roles, handlers, registrar);
    await expect(
      registrar.listeners.get('window:minimize')?.(
        eventFor(7, ROLE_URL.main),
        VALID_INVOKE_REQUESTS['window:minimize'],
      ),
    ).resolves.toEqual({ ok: true, data: { accepted: true } });
    lifecycle.dispose();
  });

  it('keeps durable reset acceptance successful when its advisory renderer event cannot be sent', async () => {
    const registrar = new FakeIpcMain();
    const roles = new WindowRoleRegistry();
    registerRole(roles, 9, 'main');
    const handlers = new Proxy(
      {},
      {
        get: (_target, channel: string) => () =>
          channel === 'data:reset-all'
            ? { accepted: true, acknowledgementToken: RESET_ACKNOWLEDGEMENT_TOKEN }
            : { accepted: true },
      },
    ) as InvokeHandlerMap;
    const lifecycle = registerIpcTransport(roles, handlers, registrar);
    const response = registrar.listeners.get('data:reset-all')?.(
      eventFor(9, ROLE_URL.main, {
        send: () => {
          throw new Error('renderer gone');
        },
      }),
      VALID_INVOKE_REQUESTS['data:reset-all'],
    );
    await expect(response).resolves.toEqual({
      ok: true,
      data: { accepted: true, acknowledgementToken: RESET_ACKNOWLEDGEMENT_TOKEN },
    });
    lifecycle.dispose();
  });

  it('can drain concurrent IPC during reset without waiting on the reset invocation itself', async () => {
    const registrar = new FakeIpcMain();
    const roles = new WindowRoleRegistry();
    registerRole(roles, 8, 'main');
    let releaseConcurrent: () => void = () => undefined;
    const concurrentGate = new Promise<void>((resolve) => {
      releaseConcurrent = resolve;
    });
    const handlers = new Proxy(
      {},
      {
        get: (_target, channel: string) => {
          if (channel === 'data:reset-all') {
            return async () => {
              lifecycle.stopAccepting();
              await lifecycle.drain(['data:reset-all']);
              return {
                accepted: true,
                acknowledgementToken: RESET_ACKNOWLEDGEMENT_TOKEN,
              };
            };
          }
          if (channel === 'window:minimize') {
            return async () => {
              await concurrentGate;
              return { accepted: true };
            };
          }
          return () => ({ accepted: true });
        },
      },
    ) as InvokeHandlerMap;
    const lifecycle: IpcTransportLifecycle = registerIpcTransport(roles, handlers, registrar);
    const minimizeListener = registrar.listeners.get('window:minimize');
    const resetListener = registrar.listeners.get('data:reset-all');
    const concurrentInvocation = minimizeListener?.(
      eventFor(8, ROLE_URL.main),
      VALID_INVOKE_REQUESTS['window:minimize'],
    );
    const resetInvocation = resetListener?.(
      eventFor(8, ROLE_URL.main),
      VALID_INVOKE_REQUESTS['data:reset-all'],
    );
    let resetSettled = false;
    void resetInvocation?.then(() => {
      resetSettled = true;
    });
    await Promise.resolve();
    expect(resetSettled).toBe(false);

    releaseConcurrent();
    await expect(concurrentInvocation).resolves.toEqual({ ok: true, data: { accepted: true } });
    await expect(resetInvocation).resolves.toEqual({
      ok: true,
      data: { accepted: true, acknowledgementToken: RESET_ACKNOWLEDGEMENT_TOKEN },
    });
    lifecycle.dispose();
  });

  it('rejects seeded missing, wrong, null, array, bounds, and nested field mutations for every fixture', async () => {
    const registrar = new FakeIpcMain();
    const roles = new WindowRoleRegistry();
    const calls: string[] = [];
    const handlers = new Proxy(
      {},
      { get: (_target, channel: string) => () => calls.push(channel) },
    ) as InvokeHandlerMap;
    const lifecycle = registerIpcTransport(roles, handlers, registrar);
    let senderId = 500;
    for (const [index, channel] of (Object.keys(invokeRegistry) as InvokeChannel[]).entries()) {
      const role = invokeRegistry[channel].roles[0];
      senderId += 1;
      registerRole(roles, senderId, role);
      const invalid = fieldLevelMutations(
        VALID_INVOKE_REQUESTS[channel],
        0x13f1_3d00 + index,
      ).filter((input) => !invokeRegistry[channel].request.safeParse(input).success);
      expect(invalid.length, channel).toBeGreaterThan(0);
      for (const input of invalid) {
        await expect(
          registrar.listeners.get(channel)?.(eventFor(senderId, ROLE_URL[role]), input),
          channel,
        ).resolves.toEqual({
          ok: false,
          error: { code: 'BAD_REQUEST', message: 'The request was invalid.' },
        });
      }
    }
    expect(calls).toEqual([]);
    lifecycle.dispose();
  });

  it('rejects a deterministic seeded malformed corpus before any handler runs', async () => {
    const registrar = new FakeIpcMain();
    const roles = new WindowRoleRegistry();
    const calls: string[] = [];
    const handlers = new Proxy(
      {},
      {
        get: (_target, channel: string) => () => {
          calls.push(channel);
          return { deliberately: 'invalid response' };
        },
      },
    ) as InvokeHandlerMap;
    const lifecycle = registerIpcTransport(roles, handlers, registrar);
    let senderId = 10;
    let cases = 0;

    for (const [index, channel] of (Object.keys(invokeRegistry) as InvokeChannel[]).entries()) {
      const role = invokeRegistry[channel].roles[0];
      senderId += 1;
      registerRole(roles, senderId, role);
      const listener = registrar.listeners.get(channel);
      expect(listener, channel).toBeDefined();
      for (const malformed of seededMalformedRequests(
        VALID_INVOKE_REQUESTS[channel],
        0x5eed_1300 + index,
      )) {
        const response = await listener?.(eventFor(senderId, ROLE_URL[role]), malformed);
        expect(response, `${channel} malformed case ${String(cases)}`).toEqual({
          ok: false,
          error: { code: 'BAD_REQUEST', message: 'The request was invalid.' },
        });
        cases += 1;
      }
    }

    expect(cases).toBe(Object.keys(invokeRegistry).length * 33);
    expect(calls).toEqual([]);
    lifecycle.dispose();
    expect(registrar.listeners.size).toBe(0);
  });

  it('enforces every channel role and converts invalid handler responses safely', async () => {
    const registrar = new FakeIpcMain();
    const roles = new WindowRoleRegistry();
    const calls: string[] = [];
    const handlers = new Proxy(
      {},
      {
        get: (_target, channel: string) => () => {
          calls.push(channel);
          return { secret: 'must not cross the response schema' };
        },
      },
    ) as InvokeHandlerMap;
    const lifecycle = registerIpcTransport(roles, handlers, registrar);
    let senderId = 10_000;

    for (const channel of Object.keys(invokeRegistry) as InvokeChannel[]) {
      const listener = registrar.listeners.get(channel);
      const allowedRole = invokeRegistry[channel].roles[0];
      senderId += 1;
      registerRole(roles, senderId, allowedRole);
      expect(
        await listener?.(eventFor(senderId, ROLE_URL[allowedRole]), VALID_INVOKE_REQUESTS[channel]),
        channel,
      ).toEqual({
        ok: false,
        error: { code: 'INTERNAL', message: 'The operation could not be completed.' },
      });

      const deniedRole = alternateRole(invokeRegistry[channel].roles);
      senderId += 1;
      registerRole(roles, senderId, deniedRole);
      expect(
        await listener?.(eventFor(senderId, ROLE_URL[deniedRole]), VALID_INVOKE_REQUESTS[channel]),
        `${channel} wrong role`,
      ).toEqual({
        ok: false,
        error: { code: 'FORBIDDEN', message: 'This window is not authorized.' },
      });

      const allowedSenderId = senderId - 1;
      for (const [name, event] of [
        ['wrong URL', eventFor(allowedSenderId, 'talking-quill://app/other/index.html')],
        ['subframe', eventFor(allowedSenderId, ROLE_URL[allowedRole], { subframe: true })],
        ['destroyed sender', eventFor(allowedSenderId, ROLE_URL[allowedRole], { destroyed: true })],
      ] as const) {
        expect(
          await listener?.(event, VALID_INVOKE_REQUESTS[channel]),
          `${channel} ${name}`,
        ).toEqual({
          ok: false,
          error: { code: 'FORBIDDEN', message: 'This window is not authorized.' },
        });
      }
    }

    expect(calls).toHaveLength(Object.keys(invokeRegistry).length);
    lifecycle.dispose();
  });
});
