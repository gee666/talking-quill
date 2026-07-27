import { describe, expect, it } from 'vitest';
import {
  HelperFrameDecoder,
  decodeHelperJson,
  encodeHelperFrame,
} from '../../app/src/main/helper/framing';
import {
  HELPER_MAX_FRAME_BYTES,
  HelperNotificationSchema,
  HelperRequestIdSchema,
  HelperRpcResponseSchema,
  helperParamsSchemas,
  helperResultSchemas,
} from '../../app/src/shared/helper/protocol';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
} from '../../app/src/shared/schemas/dictation-profiles';
import { shortcutFromLegacyActivation } from '../../app/src/shared/schemas/shortcut';

const binding = (profileId: string, shortcut: unknown) => ({ profileId, shortcut });
const customProfileId = (index: number) =>
  `00000000-0000-4000-8000-${String(index).padStart(12, '0')}`;

describe('native helper framing', () => {
  it('decodes partial and concatenated big-endian frames', () => {
    const first = encodeHelperFrame({ one: 1 });
    const second = encodeHelperFrame({ two: 2 });
    const decoder = new HelperFrameDecoder();
    expect(decoder.push(first.subarray(0, 2))).toEqual([]);
    const frames = decoder.push(Buffer.concat([first.subarray(2), second]));
    expect(frames.map(decodeHelperJson)).toEqual([{ one: 1 }, { two: 2 }]);
    expect(() => decoder.finish()).not.toThrow();
  });

  it('decodes a maximum-size frame fragmented one byte at a time', () => {
    const payload = Buffer.alloc(HELPER_MAX_FRAME_BYTES, 0x61);
    const framed = Buffer.allocUnsafe(payload.length + 4);
    framed.writeUInt32BE(payload.length, 0);
    payload.copy(framed, 4);
    const decoder = new HelperFrameDecoder();
    const frames: Buffer[] = [];

    for (const byte of framed) frames.push(...decoder.push(Buffer.of(byte)));

    expect(frames).toEqual([payload]);
    expect(() => decoder.finish()).not.toThrow();
  });

  it('rejects zero, oversized, truncated, and invalid UTF-8 frames', () => {
    const zero = Buffer.alloc(4);
    expect(() => new HelperFrameDecoder().push(zero)).toThrow('Invalid helper frame length');
    const oversized = Buffer.alloc(4);
    oversized.writeUInt32BE(HELPER_MAX_FRAME_BYTES + 1);
    expect(() => new HelperFrameDecoder().push(oversized)).toThrow('Invalid helper frame length');

    const truncated = new HelperFrameDecoder();
    expect(truncated.push(encodeHelperFrame({ ok: true }).subarray(0, 7))).toEqual([]);
    expect(() => truncated.finish()).toThrow('truncated frame');
    expect(() => decodeHelperJson(Buffer.from([0xff]))).toThrow('invalid UTF-8 JSON');
    expect(() => encodeHelperFrame('x'.repeat(HELPER_MAX_FRAME_BYTES))).toThrow('Outbound helper');
  });
});

describe('native helper JSON-RPC schemas', () => {
  it('requires protocol v5 and has no lossy default activation key handshake field', () => {
    expect(helperParamsSchemas.initialize.safeParse({ protocolVersion: 5 }).success).toBe(true);
    expect(helperParamsSchemas.initialize.safeParse({ protocolVersion: 4 }).success).toBe(false);
    const initialized = {
      protocolVersion: 5,
      helperVersion: '1.0.0',
      platform: 'windows',
      architecture: 'x86_64',
      hookStatus: 'ready',
      permissions: {
        accessibility: 'not_applicable',
        inputMonitoring: 'not_applicable',
        eventPost: 'not_applicable',
      },
    };
    expect(helperResultSchemas.initialize.safeParse(initialized).success).toBe(true);
    expect(
      helperResultSchemas.initialize.safeParse({ ...initialized, defaultActivationKey: 'Z' })
        .success,
    ).toBe(false);
  });

  it('contains only the fixed command allowlist with strict params', () => {
    expect(Object.keys(helperParamsSchemas)).toEqual([
      'initialize',
      'activation.configure',
      'session.set_capture',
      'paste.inject',
      'front_app.get',
      'permissions.get',
      'ping',
      'shutdown',
    ]);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({
        enabled: true,
        bindings: [binding('general', shortcutFromLegacyActivation('Z', false))],
      }).success,
    ).toBe(true);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({
        enabled: true,
        bindings: [
          binding('prompt', {
            modifiers: { ctrl: true, alt: false, shift: true, meta: false },
            keys: ['Q', 'P'],
          }),
        ],
      }).success,
    ).toBe(true);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: true, shift: false, meta: false },
            keys: ['F1'],
          }),
        ],
      }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({ enabled: true, key: 'Z' }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['paste.inject'].safeParse({ key: 'A', shell: 'whoami' }).success,
    ).toBe(false);
  });

  it('allows only the canonical built-in prefix family and rejects unrelated conflicts', () => {
    const schema = helperParamsSchemas['activation.configure'];
    const altA = shortcutFromLegacyActivation('A', false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', DEFAULT_GENERAL_PROFILE.shortcut),
          binding('prompt', DEFAULT_PROMPT_PROFILE.shortcut),
          binding('markdown', DEFAULT_MARKDOWN_PROFILE.shortcut),
          binding('translate-to-english', DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.shortcut),
        ],
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: true, shift: false, meta: false },
            keys: ['X', 'Q'],
          }),
        ],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', DEFAULT_PROMPT_PROFILE.shortcut)],
      }).success,
    ).toBe(false);
    expect(schema.safeParse({ enabled: true, bindings: [] }).success).toBe(false);
    expect(schema.safeParse({ enabled: false, bindings: [] }).success).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: false, shift: false, meta: false },
            keys: ['A'],
          }),
        ],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', {
            modifiers: { ctrl: false, alt: false, shift: true, meta: false },
            keys: ['A'],
          }),
        ],
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', altA), binding('prompt', altA)],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', altA), binding('prompt', { ...altA, keys: ['A', 'B'] })],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', altA),
          binding('general', shortcutFromLegacyActivation('B', false)),
        ],
      }).success,
    ).toBe(false);
    expect(schema.safeParse({ enabled: true, bindings: [binding('custom', altA)] }).success).toBe(
      false,
    );
    for (const profileId of [
      '00000000-0000-0000-0000-000000000000',
      'ffffffff-ffff-ffff-ffff-ffffffffffff',
    ]) {
      expect(
        schema.safeParse({ enabled: true, bindings: [binding(profileId, altA)] }).success,
      ).toBe(true);
    }
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF', altA)],
      }).success,
    ).toBe(false);
    expect(schema.safeParse({ enabled: true, bindings: [altA] }).success).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [
          binding('general', altA),
          binding('prompt', {
            ...altA,
            modifiers: { ...altA.modifiers, shift: true },
            keys: ['A', 'B'],
          }),
        ],
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: [binding('general', { ...altA, keys: ['A', 'A'] })],
      }).success,
    ).toBe(false);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: Array.from({ length: 12 }, (_, index) =>
          binding(customProfileId(index), {
            ...altA,
            keys: [String.fromCharCode(65 + index)],
          }),
        ),
      }).success,
    ).toBe(true);
    expect(
      schema.safeParse({
        enabled: true,
        bindings: Array.from({ length: 14 }, (_, index) =>
          binding(customProfileId(index), {
            ...altA,
            keys: [String.fromCharCode(65 + index)],
          }),
        ),
      }).success,
    ).toBe(false);
  });

  it('accepts bounded numeric and v2-compatible string request IDs', () => {
    for (const id of [0, Number.MAX_SAFE_INTEGER, 'request-id', 'é'.repeat(32)]) {
      expect(HelperRequestIdSchema.safeParse(id).success).toBe(true);
    }
    for (const id of [-1, Number.MAX_SAFE_INTEGER + 1, '', 'a'.repeat(65), 'é'.repeat(33)]) {
      expect(HelperRequestIdSchema.safeParse(id).success).toBe(false);
    }
  });

  it('strictly validates responses, method results, and notifications', () => {
    expect(
      HelperRpcResponseSchema.safeParse({
        jsonrpc: '2.0',
        id: 1,
        result: { submitted: true },
      }).success,
    ).toBe(true);
    expect(
      HelperRpcResponseSchema.safeParse({
        jsonrpc: '2.0',
        id: 'request-id',
        result: {},
      }).success,
    ).toBe(true);
    expect(
      HelperRpcResponseSchema.safeParse({
        jsonrpc: '2.0',
        id: 1,
        result: {},
        extra: true,
      }).success,
    ).toBe(false);
    expect(helperResultSchemas['paste.inject'].safeParse({ submitted: false }).success).toBe(false);
    expect(
      helperResultSchemas['paste.inject'].safeParse({
        submitted: false,
        reason: 'secure_input',
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          profileId: 'translate-to-english',
          shortcut: DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.shortcut,
        },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: 'general',
          shortcut: DEFAULT_GENERAL_PROFILE.shortcut,
          heldMs: 599,
        },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: 'general',
          shortcut: DEFAULT_GENERAL_PROFILE.shortcut,
        },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: 'prompt',
          shortcut: DEFAULT_PROMPT_PROFILE.shortcut,
          heldMs: 100,
        },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: { phase: 'down', profileId: 'general', shortcut: { keys: ['Z'] } },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: { phase: 'down', shortcut: shortcutFromLegacyActivation('Z', false) },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: { phase: 'down', profileId: 'general', key: 'Z', shift: false },
      }).success,
    ).toBe(false);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'key.inject',
        params: { key: 'A' },
      }).success,
    ).toBe(false);
  });
});
