import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { ProviderError } from './errors';

export const DEFAULT_PI_RPC_LIMITS = Object.freeze({
  maxRecordBytes: 4 * 1024 * 1024,
  maxStdoutBytes: 16 * 1024 * 1024,
  maxStderrBytes: 16 * 1024,
  maxOutboundRecordBytes: 3 * 1024 * 1024,
  maxOutboundBytes: 4 * 1024 * 1024,
  maxRecords: 10_000,
  maxJsonDepth: 32,
  maxJsonValues: 50_000,
  maxObjectKeys: 512,
  maxArrayItems: 5_000,
});

export interface PiRpcLimits {
  readonly maxRecordBytes: number;
  readonly maxStdoutBytes: number;
  readonly maxStderrBytes: number;
  readonly maxOutboundRecordBytes: number;
  readonly maxOutboundBytes: number;
  readonly maxRecords: number;
  readonly maxJsonDepth: number;
  readonly maxJsonValues: number;
  readonly maxObjectKeys: number;
  readonly maxArrayItems: number;
}

export type PiRpcOutboundCommand =
  | { readonly id: string; readonly type: 'get_state' }
  | { readonly id: string; readonly type: 'prompt'; readonly message: string }
  | { readonly id: string; readonly type: 'abort' }
  | {
      readonly type: 'extension_ui_response';
      readonly id: string;
      readonly cancelled: true;
    };

interface PiRpcTransportOptions {
  readonly limits?: Partial<PiRpcLimits>;
  readonly onRecords: (records: readonly Readonly<Record<string, unknown>>[]) => void;
  readonly onFault: (error: ProviderError) => void;
  readonly onStdoutEnd: () => void;
  readonly onClose: () => void;
}

/** Raw-byte, LF-only decoder for Pi's NDJSON stdout protocol. */
export class PiRpcNdjsonDecoder {
  readonly #limits: Pick<
    PiRpcLimits,
    'maxRecordBytes' | 'maxJsonDepth' | 'maxJsonValues' | 'maxObjectKeys' | 'maxArrayItems'
  >;
  readonly #pending: Buffer[] = [];
  #pendingBytes = 0;

  constructor(
    limits: Pick<
      PiRpcLimits,
      'maxRecordBytes' | 'maxJsonDepth' | 'maxJsonValues' | 'maxObjectKeys' | 'maxArrayItems'
    >,
  ) {
    this.#limits = limits;
  }

  push(input: Buffer): readonly Readonly<Record<string, unknown>>[] {
    const records: Readonly<Record<string, unknown>>[] = [];
    let offset = 0;
    while (offset < input.length) {
      const newline = input.indexOf(0x0a, offset);
      const end = newline === -1 ? input.length : newline;
      const fragment = input.subarray(offset, end);
      if (this.#pendingBytes + fragment.length > this.#limits.maxRecordBytes) {
        throw new ProviderError('RESPONSE_TOO_LARGE');
      }
      if (fragment.length > 0) {
        this.#pending.push(fragment);
        this.#pendingBytes += fragment.length;
      }
      if (newline === -1) break;
      records.push(this.#decodeRecord(Buffer.concat(this.#pending, this.#pendingBytes)));
      this.#pending.length = 0;
      this.#pendingBytes = 0;
      offset = newline + 1;
    }
    return records;
  }

  finish(): void {
    if (this.#pendingBytes !== 0) throw new ProviderError('INVALID_RESPONSE');
  }

  discardPending(): void {
    this.#pending.length = 0;
    this.#pendingBytes = 0;
  }

  #decodeRecord(input: Buffer): Readonly<Record<string, unknown>> {
    const record =
      input.length > 0 && input[input.length - 1] === 0x0d ? input.subarray(0, -1) : input;
    if (record.length === 0) throw new ProviderError('INVALID_RESPONSE');
    if (record.length >= 3 && record[0] === 0xef && record[1] === 0xbb && record[2] === 0xbf) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    let text: string;
    try {
      text = new TextDecoder('utf-8', { fatal: true }).decode(record);
    } catch {
      throw new ProviderError('INVALID_RESPONSE');
    }
    return parseStrictJsonRecord(text, this.#limits);
  }
}

/** Bounded process-stream transport. It never retains or reports Pi content or stderr. */
export class PiRpcTransport {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly #options: PiRpcTransportOptions;
  readonly #limits: PiRpcLimits;
  readonly #decoder: PiRpcNdjsonDecoder;
  #stdoutBytes = 0;
  #stderrBytes = 0;
  #outboundBytes = 0;
  #records = 0;
  #ended = false;
  #faulted = false;
  #protocolSealed = false;
  #writeTail: Promise<void> = Promise.resolve();

  constructor(child: ChildProcessWithoutNullStreams, options: PiRpcTransportOptions) {
    this.#child = child;
    this.#options = options;
    this.#limits = validatePiRpcLimits(options.limits);
    this.#decoder = new PiRpcNdjsonDecoder(this.#limits);
    this.#attach();
  }

  write(command: PiRpcOutboundCommand): Promise<void> {
    if (this.#ended) {
      return Promise.reject(new ProviderError('PI_LAUNCH_FAILED', { fallbackEligible: true }));
    }
    assertOutboundCommand(command);
    const frame = Buffer.from(`${JSON.stringify(command)}\n`, 'utf8');
    if (frame.length > this.#limits.maxOutboundRecordBytes) {
      return Promise.reject(new ProviderError('REQUEST_TOO_LARGE', { fallbackEligible: true }));
    }
    this.#outboundBytes += frame.length;
    if (this.#outboundBytes > this.#limits.maxOutboundBytes) {
      return Promise.reject(new ProviderError('REQUEST_TOO_LARGE', { fallbackEligible: true }));
    }
    const write = this.#writeTail.then(() => this.#writeFrame(frame));
    this.#writeTail = write.catch(() => undefined);
    return write;
  }

  /** Stops application-protocol decoding while retaining bounded stdout/stderr draining. */
  sealProtocol(): void {
    if (this.#protocolSealed) return;
    this.#protocolSealed = true;
    this.#decoder.discardPending();
  }

  end(): void {
    if (this.#ended) return;
    this.#ended = true;
    try {
      this.#child.stdin.end();
    } catch {
      this.#fault(new ProviderError('PI_LAUNCH_FAILED'));
    }
  }

  #attach(): void {
    this.#child.stdout.on('data', (chunk: unknown) => {
      if (this.#faulted) return;
      if (!Buffer.isBuffer(chunk)) {
        this.#fault(new ProviderError('INVALID_RESPONSE'));
        return;
      }
      const bytes = chunk;
      this.#stdoutBytes += bytes.length;
      if (this.#stdoutBytes > this.#limits.maxStdoutBytes) {
        this.#fault(new ProviderError('RESPONSE_TOO_LARGE'));
        return;
      }
      if (this.#protocolSealed) return;
      try {
        const records = this.#decoder.push(bytes);
        if (this.#records + records.length > this.#limits.maxRecords) {
          this.#fault(new ProviderError('RESPONSE_TOO_LARGE'));
          return;
        }
        this.#records += records.length;
        this.#options.onRecords(records);
      } catch (error: unknown) {
        this.#fault(asProviderError(error));
      }
    });
    this.#child.stdout.once('end', () => {
      if (this.#faulted) return;
      if (!this.#protocolSealed) {
        try {
          this.#decoder.finish();
        } catch (error: unknown) {
          this.#fault(asProviderError(error));
          return;
        }
      }
      this.#options.onStdoutEnd();
    });
    this.#child.stdout.once('error', () => this.#fault(new ProviderError('PI_LAUNCH_FAILED')));
    this.#child.stderr.on('data', (chunk: Buffer | string) => {
      if (this.#faulted) return;
      this.#stderrBytes +=
        typeof chunk === 'string' ? Buffer.byteLength(chunk, 'utf8') : chunk.byteLength;
      if (this.#stderrBytes > this.#limits.maxStderrBytes) {
        this.#fault(new ProviderError('RESPONSE_TOO_LARGE'));
      }
    });
    this.#child.stderr.once('error', () => this.#fault(new ProviderError('PI_LAUNCH_FAILED')));
    this.#child.stdin.once('error', () => this.#fault(new ProviderError('PI_LAUNCH_FAILED')));
    this.#child.once('error', () => this.#fault(new ProviderError('PI_LAUNCH_FAILED')));
    this.#child.once('close', () => this.#options.onClose());
  }

  #writeFrame(frame: Buffer): Promise<void> {
    if (
      this.#ended ||
      this.#child.stdin.destroyed ||
      !this.#child.stdin.writable ||
      this.#child.exitCode !== null ||
      this.#child.signalCode !== null
    ) {
      return Promise.reject(new ProviderError('PI_LAUNCH_FAILED', { fallbackEligible: true }));
    }
    return new Promise<void>((resolveWrite, rejectWrite) => {
      try {
        this.#child.stdin.write(frame, (error?: Error | null) => {
          if (error === null || error === undefined) resolveWrite();
          else rejectWrite(new ProviderError('PI_LAUNCH_FAILED'));
        });
      } catch {
        rejectWrite(new ProviderError('PI_LAUNCH_FAILED', { fallbackEligible: true }));
      }
    });
  }

  #fault(error: ProviderError): void {
    if (this.#faulted) return;
    this.#faulted = true;
    this.#options.onFault(error);
  }
}

function assertOutboundCommand(command: PiRpcOutboundCommand): void {
  const keys = Object.keys(command).sort();
  if (command.type === 'get_state' || command.type === 'abort') {
    if (!sameKeys(keys, ['id', 'type']) || !validId(command.id)) {
      throw new ProviderError('INVALID_CONFIG');
    }
    return;
  }
  if (command.type === 'prompt') {
    if (
      !sameKeys(keys, ['id', 'message', 'type']) ||
      !validId(command.id) ||
      typeof command.message !== 'string'
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    return;
  }
  if (!sameKeys(keys, ['cancelled', 'id', 'type']) || !validId(command.id)) {
    throw new ProviderError('INVALID_CONFIG');
  }
}

function parseStrictJsonRecord(
  text: string,
  limits: Pick<PiRpcLimits, 'maxJsonDepth' | 'maxJsonValues' | 'maxObjectKeys' | 'maxArrayItems'>,
): Readonly<Record<string, unknown>> {
  try {
    new JsonStructureValidator(text, limits).validate();
    const value: unknown = JSON.parse(text);
    if (!isRecord(value)) throw new Error('not an object');
    return value;
  } catch {
    throw new ProviderError('INVALID_RESPONSE');
  }
}

/** Validates JSON grammar while detecting duplicate keys and structural floods. */
class JsonStructureValidator {
  readonly #text: string;
  readonly #limits: Pick<
    PiRpcLimits,
    'maxJsonDepth' | 'maxJsonValues' | 'maxObjectKeys' | 'maxArrayItems'
  >;
  #offset = 0;
  #values = 0;

  constructor(
    text: string,
    limits: Pick<PiRpcLimits, 'maxJsonDepth' | 'maxJsonValues' | 'maxObjectKeys' | 'maxArrayItems'>,
  ) {
    this.#text = text;
    this.#limits = limits;
  }

  validate(): void {
    this.#value(0);
    this.#whitespace();
    if (this.#offset !== this.#text.length) throw new Error('trailing JSON');
  }

  #value(depth: number): void {
    this.#values += 1;
    if (this.#values > this.#limits.maxJsonValues || depth > this.#limits.maxJsonDepth) {
      throw new Error('JSON structure limit');
    }
    this.#whitespace();
    const token = this.#text[this.#offset];
    if (token === '{') this.#object(depth + 1);
    else if (token === '[') this.#array(depth + 1);
    else if (token === '"') void this.#string();
    else if (token === 't') this.#literal('true');
    else if (token === 'f') this.#literal('false');
    else if (token === 'n') this.#literal('null');
    else this.#number();
  }

  #object(depth: number): void {
    this.#offset += 1;
    this.#whitespace();
    if (this.#text[this.#offset] === '}') {
      this.#offset += 1;
      return;
    }
    const keys = new Set<string>();
    for (;;) {
      this.#whitespace();
      const key = this.#string();
      if (keys.has(key) || keys.size >= this.#limits.maxObjectKeys) {
        throw new Error('duplicate or excessive JSON key');
      }
      keys.add(key);
      this.#whitespace();
      this.#consume(':');
      this.#value(depth);
      this.#whitespace();
      const token = this.#text[this.#offset];
      if (token === '}') {
        this.#offset += 1;
        return;
      }
      this.#consume(',');
    }
  }

  #array(depth: number): void {
    this.#offset += 1;
    this.#whitespace();
    if (this.#text[this.#offset] === ']') {
      this.#offset += 1;
      return;
    }
    let items = 0;
    for (;;) {
      items += 1;
      if (items > this.#limits.maxArrayItems) throw new Error('excessive JSON array');
      this.#value(depth);
      this.#whitespace();
      const token = this.#text[this.#offset];
      if (token === ']') {
        this.#offset += 1;
        return;
      }
      this.#consume(',');
    }
  }

  #string(): string {
    const start = this.#offset;
    this.#consume('"');
    while (this.#offset < this.#text.length) {
      const token = this.#text[this.#offset];
      if (token === '"') {
        this.#offset += 1;
        return JSON.parse(this.#text.slice(start, this.#offset)) as string;
      }
      if (token === '\\') {
        this.#offset += 1;
        const escape = this.#text[this.#offset];
        if (escape === 'u') {
          const hex = this.#text.slice(this.#offset + 1, this.#offset + 5);
          if (!/^[0-9a-fA-F]{4}$/u.test(hex)) throw new Error('invalid JSON escape');
          this.#offset += 5;
          continue;
        }
        if (escape === undefined || !'"\\/bfnrt'.includes(escape)) {
          throw new Error('invalid JSON escape');
        }
        this.#offset += 1;
        continue;
      }
      if (token === undefined || token.charCodeAt(0) < 0x20) throw new Error('invalid JSON string');
      this.#offset += 1;
    }
    throw new Error('unterminated JSON string');
  }

  #number(): void {
    const match = /-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/uy;
    match.lastIndex = this.#offset;
    const result = match.exec(this.#text);
    if (result === null) throw new Error('invalid JSON value');
    this.#offset = match.lastIndex;
  }

  #literal(value: string): void {
    if (!this.#text.startsWith(value, this.#offset)) throw new Error('invalid JSON literal');
    this.#offset += value.length;
  }

  #consume(value: string): void {
    if (this.#text[this.#offset] !== value) throw new Error('invalid JSON grammar');
    this.#offset += 1;
  }

  #whitespace(): void {
    while (/^[\t\n\r ]$/u.test(this.#text[this.#offset] ?? '')) this.#offset += 1;
  }
}

export function validatePiRpcLimits(overrides: Partial<PiRpcLimits> | undefined): PiRpcLimits {
  const limits = { ...DEFAULT_PI_RPC_LIMITS, ...overrides };
  for (const value of Object.values(limits)) {
    if (!Number.isSafeInteger(value) || value < 1) throw new ProviderError('INVALID_CONFIG');
  }
  return Object.freeze(limits);
}

function asProviderError(error: unknown): ProviderError {
  return error instanceof ProviderError ? error : new ProviderError('INVALID_RESPONSE');
}

function validId(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 128 && noControls(value);
}

function noControls(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x20 || code === 0x7f) return false;
  }
  return true;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function sameKeys(actual: readonly string[], expected: readonly string[]): boolean {
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}
