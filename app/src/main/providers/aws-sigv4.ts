import { createHash, createHmac } from 'node:crypto';
import { AwsCredentialsSchema, type AwsCredentials } from '../../shared/schemas/credentials';
import { ProviderError } from './errors';

export function parseAwsCredentials(input: string): AwsCredentials {
  try {
    return AwsCredentialsSchema.parse(JSON.parse(input) as unknown);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
}

export function signAwsRequest(options: {
  readonly method: 'GET' | 'POST';
  readonly url: URL;
  readonly body?: unknown;
  readonly region: string;
  readonly service: 'bedrock';
  readonly credentials: AwsCredentials;
  readonly now?: Date;
}): Readonly<Record<string, string>> {
  const now = options.now ?? new Date();
  if (!Number.isFinite(now.getTime())) throw new ProviderError('INVALID_CONFIG');
  const amzDate = now.toISOString().replace(/[:-]|\.\d{3}/g, '');
  const date = amzDate.slice(0, 8);
  const payload = options.body === undefined ? '' : serialize(options.body);
  const payloadHash = sha256(payload);
  const canonicalHeaders = [
    `host:${options.url.host.toLowerCase()}\n`,
    `x-amz-content-sha256:${payloadHash}\n`,
    `x-amz-date:${amzDate}\n`,
    ...(options.credentials.sessionToken === undefined
      ? []
      : [`x-amz-security-token:${options.credentials.sessionToken.trim()}\n`]),
  ].join('');
  const signedHeaders =
    options.credentials.sessionToken === undefined
      ? 'host;x-amz-content-sha256;x-amz-date'
      : 'host;x-amz-content-sha256;x-amz-date;x-amz-security-token';
  const canonicalRequest = [
    options.method,
    canonicalUri(options.url.pathname),
    canonicalQuery(options.url),
    canonicalHeaders,
    signedHeaders,
    payloadHash,
  ].join('\n');
  const scope = `${date}/${options.region}/${options.service}/aws4_request`;
  const stringToSign = ['AWS4-HMAC-SHA256', amzDate, scope, sha256(canonicalRequest)].join('\n');
  const dateKey = hmac(`AWS4${options.credentials.secretAccessKey}`, date);
  const regionKey = hmac(dateKey, options.region);
  const serviceKey = hmac(regionKey, options.service);
  const signingKey = hmac(serviceKey, 'aws4_request');
  const signature = createHmac('sha256', signingKey).update(stringToSign, 'utf8').digest('hex');
  return Object.freeze({
    authorization: `AWS4-HMAC-SHA256 Credential=${options.credentials.accessKeyId}/${scope}, SignedHeaders=${signedHeaders}, Signature=${signature}`,
    'x-amz-content-sha256': payloadHash,
    'x-amz-date': amzDate,
    ...(options.credentials.sessionToken === undefined
      ? {}
      : { 'x-amz-security-token': options.credentials.sessionToken }),
  });
}

function serialize(input: unknown): string {
  try {
    const value: unknown = JSON.stringify(input);
    if (typeof value !== 'string') throw new ProviderError('INVALID_CONFIG');
    return value;
  } catch (error: unknown) {
    if (error instanceof ProviderError) throw error;
    throw new ProviderError('INVALID_CONFIG');
  }
}

function sha256(input: string): string {
  return createHash('sha256').update(input, 'utf8').digest('hex');
}

function hmac(key: string | Buffer, input: string): Buffer {
  return createHmac('sha256', key).update(input, 'utf8').digest();
}

function canonicalUri(pathname: string): string {
  return pathname
    .split('/')
    .map((segment) => awsEncode(segment))
    .join('/');
}

function canonicalQuery(url: URL): string {
  return [...url.searchParams.entries()]
    .map(([key, value]) => [awsEncode(key), awsEncode(value)] as const)
    .sort(([leftKey, leftValue], [rightKey, rightValue]) => {
      if (leftKey < rightKey) return -1;
      if (leftKey > rightKey) return 1;
      if (leftValue < rightValue) return -1;
      if (leftValue > rightValue) return 1;
      return 0;
    })
    .map(([key, value]) => `${key}=${value}`)
    .join('&');
}

function awsEncode(value: string): string {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}
