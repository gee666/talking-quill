import { createHash } from 'node:crypto';
import aws4, { type Request as Aws4Request } from 'aws4';
import { describe, expect, it } from 'vitest';
import { parseAwsCredentials, signAwsRequest } from '../../app/src/main/providers/aws-sigv4';
import { serializeAwsCredentials } from '../../app/src/shared/schemas/credentials';

const credentials = {
  accessKeyId: 'AKIDEXAMPLE123456',
  secretAccessKey: 'wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY',
  sessionToken: 'session-token-example-123456',
} as const;

describe('AWS Signature Version 4', () => {
  it('produces a deterministic Bedrock signature over path, query, headers, and payload', () => {
    const body = {
      messages: [{ role: 'user', content: [{ text: 'hello' }] }],
      inferenceConfig: { temperature: 0.2, maxTokens: 64 },
    };
    const headers = signAwsRequest({
      method: 'POST',
      url: new URL(
        'https://bedrock-runtime.us-west-2.amazonaws.com/model/profile%3Aexample/converse?z=last&a=first',
      ),
      body,
      region: 'us-west-2',
      service: 'bedrock',
      credentials,
      now: new Date('2025-01-02T03:04:05.000Z'),
    });

    expect(headers).toEqual({
      authorization:
        'AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE123456/20250102/us-west-2/bedrock/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token, Signature=83ada18c414e67982f84b27749c70f60d9616cda4b84a236361554e51b5ff2a2',
      'x-amz-content-sha256': createHash('sha256')
        .update(JSON.stringify(body), 'utf8')
        .digest('hex'),
      'x-amz-date': '20250102T030405Z',
      'x-amz-security-token': 'session-token-example-123456',
    });
  });

  it.each([
    {
      name: 'control-plane GET with repeated canonical query parameters',
      method: 'GET' as const,
      url: new URL(
        'https://bedrock.us-west-2.amazonaws.com/foundation-models?nextToken=%CE%B2&byOutputModality=TEXT&byOutputModality=IMAGE',
      ),
      body: undefined,
    },
    {
      name: 'runtime POST with encoded profile ARN and multibyte raw payload',
      method: 'POST' as const,
      url: new URL(
        'https://bedrock-runtime.us-west-2.amazonaws.com/model/arn%3Aaws%3Abedrock%3Aus-west-2%3A123456789012%3Aapplication-inference-profile%2Fprofile-id/converse',
      ),
      body: {
        messages: [{ role: 'user', content: [{ text: 'Grüße 世界 😀' }] }],
        inferenceConfig: { maxTokens: 64 },
      },
    },
  ])('matches the independent aws4 oracle for $name', ({ method, url, body }) => {
    const actual = signAwsRequest({
      method,
      url,
      ...(body === undefined ? {} : { body }),
      region: 'us-west-2',
      service: 'bedrock',
      credentials,
      now: new Date('2025-01-02T03:04:05.000Z'),
    });
    const payload = body === undefined ? '' : JSON.stringify(body);
    const payloadHash = createHash('sha256').update(payload, 'utf8').digest('hex');
    const oracleInput: Aws4Request & {
      extraHeadersToIgnore: Readonly<Record<string, boolean>>;
    } = {
      host: url.host,
      path: `${url.pathname}${url.search}`,
      method,
      service: 'bedrock',
      region: 'us-west-2',
      body: payload,
      headers: {
        'X-Amz-Date': '20250102T030405Z',
        'X-Amz-Content-Sha256': payloadHash,
        'X-Amz-Security-Token': credentials.sessionToken,
      },
      // aws4 supplies transport headers for a body; Bedrock's signing contract here
      // intentionally signs only the host and x-amz-* headers.
      extraHeadersToIgnore: { 'content-length': true, 'content-type': true },
    };
    const oracleRequest = aws4.sign(oracleInput, credentials);
    const oracleHeaders = lowerCaseHeaders(oracleRequest.headers ?? {});

    expect(actual.authorization).toBe(oracleHeaders.authorization);
    expect(actual['x-amz-content-sha256']).toBe(payloadHash);
    expect(actual['x-amz-date']).toBe(oracleHeaders['x-amz-date']);
    expect(actual['x-amz-security-token']).toBe(oracleHeaders['x-amz-security-token']);
  });

  it('round-trips strict structured credentials and rejects legacy bearer strings', () => {
    const serialized = serializeAwsCredentials(credentials);
    expect(parseAwsCredentials(serialized)).toEqual(credentials);
    expect(() => parseAwsCredentials('legacy-bedrock-bearer-key')).toThrow(
      expect.objectContaining({ code: 'INVALID_CONFIG' }),
    );
    expect(serialized).not.toContain('Bearer');
  });
});

function lowerCaseHeaders(
  headers: Readonly<Record<string, string | string[] | number | undefined>>,
): Readonly<Record<string, string>> {
  const normalized: Record<string, string> = {};
  for (const [name, value] of Object.entries(headers)) {
    if (value === undefined) continue;
    normalized[name.toLowerCase()] = Array.isArray(value) ? value.join(',') : String(value);
  }
  return normalized;
}
