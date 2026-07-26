import { lookup } from 'node:dns/promises';
import { isIP } from 'node:net';
import type { Destination } from '../../shared/schemas/providers';
import { ProviderError } from '../providers/errors';

const MAX_DNS_ANSWERS = 64;
const MAX_ENDPOINT_LENGTH = 4_096;
const SENSITIVE_QUERY_KEY =
  /^(?:key|api[-_]?key|access[-_]?key|auth|credential|password|secret|token)$/i;

export interface ResolvedAddress {
  readonly address: string;
  readonly family: 4 | 6;
}

export type EndpointResolver = (
  hostname: string,
  signal: AbortSignal,
) => Promise<readonly ResolvedAddress[]>;

export interface EndpointPolicyOptions {
  readonly credentialed: boolean;
  readonly fixedCloud?: boolean;
  readonly signal: AbortSignal;
}

export interface ValidatedEndpoint {
  readonly url: URL;
  readonly destination: Destination;
  readonly addresses: readonly ResolvedAddress[];
  readonly pinnedAddress: ResolvedAddress;
}

export const systemEndpointResolver: EndpointResolver = async (hostname, signal) => {
  const answers = await waitForAbort(lookup(hostname, { all: true, verbatim: true }), signal);
  return answers.map((answer) => {
    if (answer.family !== 4 && answer.family !== 6) throw new ProviderError('SECURITY_BLOCKED');
    return Object.freeze({ address: answer.address, family: answer.family });
  });
};

export async function validateProviderEndpoint(
  value: string | URL,
  resolver: EndpointResolver = systemEndpointResolver,
  options: EndpointPolicyOptions,
): Promise<ValidatedEndpoint> {
  throwIfAborted(options.signal);
  const url = parseProviderUrl(value);
  const hostname = normalizeHostname(url.hostname);
  const literalFamily = isIP(hostname);
  let addresses: readonly ResolvedAddress[];
  try {
    addresses =
      literalFamily === 0
        ? await waitForAbort(resolver(hostname, options.signal), options.signal)
        : [Object.freeze({ address: hostname, family: literalFamily as 4 | 6 })];
  } catch (error: unknown) {
    if (error instanceof ProviderError) throw error;
    if (options.signal.aborted) throw new ProviderError('CANCELLED');
    throw new ProviderError('UNAVAILABLE');
  }
  throwIfAborted(options.signal);
  if (addresses.length === 0 || addresses.length > MAX_DNS_ANSWERS) {
    throw new ProviderError('SECURITY_BLOCKED');
  }

  const checked = Object.freeze([
    ...new Map(
      addresses
        .map((answer) => normalizeResolvedAddress(answer))
        .map((answer) => [`${String(answer.family)}:${answer.address}`, answer] as const),
    ).values(),
  ]);
  const destinations = new Set(checked.map((answer) => classifyIpAddress(answer.address)));
  if (destinations.has('blocked') || destinations.size !== 1) {
    throw new ProviderError('SECURITY_BLOCKED');
  }
  const destination = [...destinations][0];
  if (destination === undefined || destination === 'blocked') {
    throw new ProviderError('SECURITY_BLOCKED');
  }
  assertEndpointPolicy(url, destination, options);

  const pinnedAddress = checked.at(0);
  if (pinnedAddress === undefined) throw new ProviderError('SECURITY_BLOCKED');
  return Object.freeze({
    url,
    destination,
    addresses: checked,
    pinnedAddress,
  });
}

export function assertValidatedEndpointPolicy(
  endpoint: ValidatedEndpoint,
  value: string | URL,
  options: EndpointPolicyOptions,
): void {
  throwIfAborted(options.signal);
  const url = parseProviderUrl(value);
  if (url.origin !== endpoint.url.origin) throw new ProviderError('SECURITY_BLOCKED');
  assertEndpointPolicy(url, endpoint.destination, options);
}

export function parseProviderUrl(value: string | URL): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
  if (url.href.length > MAX_ENDPOINT_LENGTH) throw new ProviderError('INVALID_CONFIG');
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new ProviderError('SECURITY_BLOCKED');
  }
  if (url.username.length > 0 || url.password.length > 0 || url.hash.length > 0) {
    throw new ProviderError('SECURITY_BLOCKED');
  }
  for (const key of url.searchParams.keys()) {
    if (SENSITIVE_QUERY_KEY.test(key)) throw new ProviderError('SECURITY_BLOCKED');
  }
  if (url.hostname.length === 0 || url.hostname.length > 253) {
    throw new ProviderError('INVALID_CONFIG');
  }
  if (url.hostname.endsWith('.') && !url.hostname.startsWith('[')) {
    url.hostname = url.hostname.slice(0, -1);
  }
  return url;
}

export function classifyIpAddress(input: string): Destination | 'blocked' {
  const address = normalizeIpLiteral(input);
  const dottedMapped = /^::ffff:(\d{1,3}(?:\.\d{1,3}){3})$/i.exec(address);
  if (dottedMapped?.[1] !== undefined) return classifyIpv4(dottedMapped[1]);
  const family = isIP(address);
  if (family === 4) return classifyIpv4(address);
  if (family === 6) return classifyIpv6(address);
  return 'blocked';
}

function assertEndpointPolicy(
  url: URL,
  destination: Destination,
  options: Pick<EndpointPolicyOptions, 'credentialed' | 'fixedCloud'>,
): void {
  if (destination === 'cloud' && url.protocol !== 'https:') {
    throw new ProviderError('SECURITY_BLOCKED');
  }
  if (options.credentialed && destination !== 'local' && url.protocol !== 'https:') {
    throw new ProviderError('SECURITY_BLOCKED');
  }
  if (options.fixedCloud && (url.protocol !== 'https:' || destination !== 'cloud')) {
    throw new ProviderError('SECURITY_BLOCKED');
  }
}

function normalizeResolvedAddress(answer: ResolvedAddress): ResolvedAddress {
  const address = normalizeIpLiteral(answer.address);
  const family = isIP(address);
  if (family !== answer.family) throw new ProviderError('SECURITY_BLOCKED');
  return Object.freeze({ address, family: answer.family });
}

function normalizeHostname(hostname: string): string {
  const unwrapped =
    hostname.startsWith('[') && hostname.endsWith(']') ? hostname.slice(1, -1) : hostname;
  if (unwrapped.includes('%')) throw new ProviderError('SECURITY_BLOCKED');
  return unwrapped.toLowerCase();
}

function normalizeIpLiteral(address: string): string {
  return normalizeHostname(address);
}

function classifyIpv4(address: string): Destination | 'blocked' {
  const value = ipv4ToBigInt(address);
  if (value === null) return 'blocked';
  if (inPrefix(value, 0x7f00_0000n, 8, 32)) return 'local';
  if (
    inPrefix(value, 0x0a00_0000n, 8, 32) ||
    inPrefix(value, 0xac10_0000n, 12, 32) ||
    inPrefix(value, 0xc0a8_0000n, 16, 32)
  ) {
    return 'lan';
  }
  if (
    inPrefix(value, 0x0000_0000n, 8, 32) ||
    inPrefix(value, 0x6440_0000n, 10, 32) ||
    inPrefix(value, 0xa9fe_0000n, 16, 32) ||
    value === 0xa83f_8110n || // Azure WireServer/platform virtual IP (168.63.129.16)
    inPrefix(value, 0xc000_0000n, 24, 32) ||
    inPrefix(value, 0xc000_0200n, 24, 32) ||
    inPrefix(value, 0xc01f_c400n, 24, 32) ||
    inPrefix(value, 0xc034_c100n, 24, 32) ||
    inPrefix(value, 0xc058_6300n, 24, 32) ||
    inPrefix(value, 0xc0af_3000n, 24, 32) ||
    inPrefix(value, 0xc612_0000n, 15, 32) ||
    inPrefix(value, 0xc633_6400n, 24, 32) ||
    inPrefix(value, 0xcb00_7100n, 24, 32) ||
    inPrefix(value, 0xe000_0000n, 4, 32) ||
    inPrefix(value, 0xf000_0000n, 4, 32)
  ) {
    return 'blocked';
  }
  return 'cloud';
}

function classifyIpv6(address: string): Destination | 'blocked' {
  const value = ipv6ToBigInt(address);
  if (value === null) return 'blocked';

  const mappedPrefix = value >> 32n;
  if (mappedPrefix === 0xffffn) return classifyIpv4(bigIntToIpv4(value & 0xffff_ffffn));
  if (value === 1n) return 'local';
  // IPv4-compatible and other IPv4-embedded forms in ::/96 are never accepted.
  if (value >> 32n === 0n) return 'blocked';

  // AWS EC2 Instance Metadata Service IPv6 endpoints must be checked before ULA.
  if (value === (0xfd00_0ec2n << 96n) + 0x254n || value === (0xfd00_0ec2n << 96n) + 0x23n) {
    return 'blocked';
  }
  if (inPrefix(value, 0xfc00n << 112n, 7, 128)) return 'lan';

  // Only globally routed unicast can be classified as cloud. The explicit exclusions
  // below are special-purpose subranges within 2000::/3.
  if (!inPrefix(value, 0x2000n << 112n, 3, 128)) return 'blocked';
  if (
    ((value >> 32n) & 0xffff_ffffn) === 0x0000_5efen || // ISATAP IPv4 embedding
    value === (0x2001_0001n << 96n) + 1n ||
    value === (0x2001_0001n << 96n) + 2n ||
    value === (0x2001_0001n << 96n) + 3n ||
    inPrefix(value, 0x2001_0000n << 96n, 32, 128) || // Teredo
    inPrefix(value, 0x2001_0002n << 96n, 48, 128) || // benchmarking
    inPrefix(value, 0x2001_0003n << 96n, 32, 128) || // AMT
    inPrefix(value, 0x2001_0004_0112n << 80n, 48, 128) || // AS112
    inPrefix(value, 0x2001_0010n << 96n, 28, 128) || // ORCHID
    inPrefix(value, 0x2001_0020n << 96n, 28, 128) || // ORCHIDv2
    inPrefix(value, 0x2001_0030n << 96n, 28, 128) ||
    inPrefix(value, 0x2001_0100n << 96n, 64, 128) || // discard-only
    inPrefix(value, 0x2001_0db8n << 96n, 32, 128) || // documentation
    inPrefix(value, 0x2002n << 112n, 16, 128) || // 6to4
    inPrefix(value, 0x3fffn << 112n, 20, 128) || // documentation
    inPrefix(value, 0x2620_004f_8000n << 80n, 48, 128) // AS112 direct delegation
  ) {
    return 'blocked';
  }
  return 'cloud';
}

function ipv4ToBigInt(address: string): bigint | null {
  const parts = address.split('.').map(Number);
  if (
    parts.length !== 4 ||
    parts.some((part) => !Number.isInteger(part) || part < 0 || part > 255)
  ) {
    return null;
  }
  return parts.reduce((result, part) => (result << 8n) | BigInt(part), 0n);
}

function bigIntToIpv4(value: bigint): string {
  return [
    Number((value >> 24n) & 255n),
    Number((value >> 16n) & 255n),
    Number((value >> 8n) & 255n),
    Number(value & 255n),
  ].join('.');
}

function inPrefix(value: bigint, prefix: bigint, bits: number, width: number): boolean {
  const shift = BigInt(width - bits);
  return value >> shift === prefix >> shift;
}

function ipv6ToBigInt(address: string): bigint | null {
  if (isIP(address) !== 6) return null;
  const pieces = address.split('::');
  if (pieces.length > 2) return null;
  const left = parseIpv6Side(pieces[0] ?? '');
  const right = parseIpv6Side(pieces[1] ?? '');
  if (left === null || right === null) return null;
  const missing = 8 - left.length - right.length;
  if ((pieces.length === 2 && missing < 1) || (pieces.length === 1 && missing !== 0)) return null;
  const groups = [...left, ...Array<number>(missing).fill(0), ...right];
  return groups.reduce((result, group) => (result << 16n) | BigInt(group), 0n);
}

function parseIpv6Side(side: string): number[] | null {
  if (side.length === 0) return [];
  const groups: number[] = [];
  for (const part of side.split(':')) {
    if (part.includes('.')) {
      const value = ipv4ToBigInt(part);
      if (value === null) return null;
      groups.push(Number((value >> 16n) & 0xffffn), Number(value & 0xffffn));
      continue;
    }
    if (!/^[a-f0-9]{1,4}$/i.test(part)) return null;
    groups.push(Number.parseInt(part, 16));
  }
  return groups;
}

function throwIfAborted(signal: AbortSignal): void {
  if (signal.aborted) throw new ProviderError('CANCELLED');
}

function waitForAbort<Result>(operation: Promise<Result>, signal: AbortSignal): Promise<Result> {
  throwIfAborted(signal);
  return new Promise<Result>((resolve, reject) => {
    const abort = (): void => reject(new ProviderError('CANCELLED'));
    signal.addEventListener('abort', abort, { once: true });
    void operation.then(
      (result) => {
        signal.removeEventListener('abort', abort);
        resolve(result);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        reject(error instanceof Error ? error : new ProviderError('UNAVAILABLE'));
      },
    );
  });
}
