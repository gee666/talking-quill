import { describe, expect, it } from 'vitest';
import {
  HelperFrameDecoder,
  decodeHelperJson,
  encodeHelperFrame,
} from '../../app/src/main/helper/framing';
import {
  HELPER_MAX_FRAME_BYTES,
  HelperNotificationSchema,
  HelperRpcResponseSchema,
  helperParamsSchemas,
  helperResultSchemas,
} from '../../app/src/shared/helper/protocol';

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
        bindings: [{ key: 'Z', shift: false }],
      }).success,
    ).toBe(true);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({
        enabled: true,
        bindings: [{ key: 'F1', shift: false }],
      }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['activation.configure'].safeParse({ enabled: true, key: 'Z' }).success,
    ).toBe(false);
    expect(
      helperParamsSchemas['paste.inject'].safeParse({ key: 'A', shell: 'whoami' }).success,
    ).toBe(false);
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
        params: { phase: 'down', key: 'Z', shift: true },
      }).success,
    ).toBe(true);
    expect(
      HelperNotificationSchema.safeParse({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: { phase: 'down', alternate: true },
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
