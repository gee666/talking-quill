import { generateKeyPairSync, randomBytes, sign, type KeyObject } from 'node:crypto';
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  acceptanceNonceLedgerPaths,
  acceptanceNonceReservationRecord,
  acceptancePayloadBytes,
  authorizeInstalledAcceptance,
  canonicalAcceptanceJson,
  consumeInstalledAcceptanceNonce,
  encodeCanonicalAcceptanceEnvelope,
} from '../../app/src/main/acceptance/authorization';
import type {
  AcceptanceBuildManifestPayload,
  AcceptanceRunRequestPayload,
} from '../../app/src/main/acceptance/authorization-schema';

const NOW = 1_800_000_000_000;
const SOURCE_REVISION = '0123456789ab';
const BUILD_ID = '11'.repeat(32);
const readinessPipe = String.raw`\\.\pipe\TalkingQuill.InstalledReadiness.${'22'.repeat(16)}`;
const correlation = '33'.repeat(32);

function publicKeyBase64url(key: KeyObject): string {
  return Buffer.from(key.export({ format: 'der', type: 'spki' })).toString('base64url');
}

function envelope(payload: unknown, privateKey: KeyObject): string {
  return encodeCanonicalAcceptanceEnvelope({
    payload,
    signatureBase64url: sign('sha256', acceptancePayloadBytes(payload), {
      key: privateKey,
      dsaEncoding: 'ieee-p1363',
    }).toString('base64url'),
  });
}

function fixture(overrides: Partial<AcceptanceRunRequestPayload> = {}) {
  const manifestKeys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const requestKeys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const manifestPayload: AcceptanceBuildManifestPayload = {
    version: 1,
    purpose: 'talking-quill/installed-acceptance-build',
    sourceRevision: SOURCE_REVISION,
    buildId: BUILD_ID,
    architecture: 'x64',
    packageVersion: '0.0.69',
    releaseBuildDigest: '55'.repeat(32),
    packageLayoutDigest: '66'.repeat(32),
    ownerManifestSha256: '77'.repeat(32),
    electronSha256: '88'.repeat(32),
    appAsarSha256: '89'.repeat(32),
    gatewaySha256: '99'.repeat(32),
    ownerSha256: 'aa'.repeat(32),
    requestPublicKeySpkiBase64url: publicKeyBase64url(requestKeys.publicKey),
    validationPublicKeySpkiBase64url: publicKeyBase64url(requestKeys.publicKey),
    faultValidationPolicy: {
      schemaVersion: 1,
      phases: [
        'staged',
        'prepared',
        'predecessorMoved',
        'publishing',
        'publishedBeforePersist',
        'published',
        'registered',
        'committed',
        'legacyRetiring',
        'legacyRetired',
      ],
      validatorSha256: 'ab'.repeat(32),
    },
    validFromMs: NOW - 60_000,
    validUntilMs: NOW + 81 * 60_000,
  };
  const requestPayload: AcceptanceRunRequestPayload = {
    version: 1,
    purpose: 'talking-quill/installed-acceptance-run',
    command: 'normal-readiness',
    buildId: BUILD_ID,
    invocationId: 'profile-normal-readiness',
    latestStartOffsetMs: 2 * 60_000,
    deadlineOffsetMs: 3 * 60_000,
    runWindow: {
      notBeforeMs: NOW - 1_000,
      expiresAtMs: NOW - 1_000 + 80 * 60_000,
      maxTotalRunMs: 80 * 60_000,
    },
    requestNonce: '44'.repeat(32),
    issuedAtMs: NOW - 1_000,
    expiresAtMs: NOW - 1_000 + 3 * 60_000,
    readinessPipe,
    launchCorrelation: correlation,
    physicalObservation: false,
    automationValidation: false,
    automationArmedPipe: null,
    automationCase: null,
    lifecycleUserData: null,
    heartbeatDurationMs: 6_250,
    ...overrides,
  };
  const encodedRequest = envelope(requestPayload, requestKeys.privateKey);
  return {
    requestPayload,
    options: {
      acceptanceBuild: true,
      encodedBuildManifest: envelope(manifestPayload, manifestKeys.privateKey),
      manifestPublicKeySpkiBase64url: publicKeyBase64url(manifestKeys.publicKey),
      sourceRevision: SOURCE_REVISION,
      nowMs: NOW,
      argv: [
        `--talking-quill-installed-readiness-pipe=${requestPayload.readinessPipe}`,
        `--talking-quill-launch-correlation=${requestPayload.launchCorrelation}`,
        ...(requestPayload.physicalObservation
          ? ['--talking-quill-installed-physical-observation']
          : []),
        ...(requestPayload.automationValidation
          ? [
              '--talking-quill-installed-automation-validation',
              `--talking-quill-automation-armed-pipe=${String(requestPayload.automationArmedPipe)}`,
              `--talking-quill-automation-case=${String(requestPayload.automationCase)}`,
            ]
          : []),
        ...(requestPayload.lifecycleUserData === null
          ? []
          : [`--talking-quill-installed-lifecycle-user-data=${requestPayload.lifecycleUserData}`]),
        `--talking-quill-acceptance-request=${encodedRequest}`,
      ],
    },
  };
}

describe('installed acceptance authorization', () => {
  it('accepts a canonical P-256 manifest and a build-bound per-run request', () => {
    const value = fixture();
    expect(authorizeInstalledAcceptance(value.options)).toEqual(value.requestPayload);
  });

  it('atomically consumes a build-bound nonce in the stable acceptance temp ledger', () => {
    const value = fixture();
    const root = resolve('tmp', `acceptance-ledger-${randomBytes(8).toString('hex')}`);
    mkdirSync(root, { recursive: true });
    try {
      const paths = acceptanceNonceLedgerPaths(
        root,
        value.requestPayload.buildId,
        value.requestPayload.requestNonce,
      );
      mkdirSync(resolve(paths.reserved, '..'), { recursive: true });
      writeFileSync(
        paths.reserved,
        `${canonicalAcceptanceJson(acceptanceNonceReservationRecord(value.requestPayload))}\n`,
        { flag: 'wx' },
      );
      consumeInstalledAcceptanceNonce(value.requestPayload, root, NOW);
      expect(existsSync(paths.reserved)).toBe(false);
      expect(existsSync(paths.consumed)).toBe(true);
      expect(() => consumeInstalledAcceptanceNonce(value.requestPayload, root, NOW)).toThrow(
        'already consumed',
      );
      expect(() =>
        consumeInstalledAcceptanceNonce(
          { ...value.requestPayload, requestNonce: 'ab'.repeat(32), expiresAtMs: NOW - 1 },
          root,
          NOW,
        ),
      ).toThrow('expired before nonce consumption');
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('rejects a reserved nonce whose invocation binding differs', () => {
    const value = fixture({ requestNonce: 'ab'.repeat(32) });
    const root = resolve('tmp', `acceptance-ledger-${randomBytes(8).toString('hex')}`);
    const paths = acceptanceNonceLedgerPaths(
      root,
      value.requestPayload.buildId,
      value.requestPayload.requestNonce,
    );
    mkdirSync(resolve(paths.reserved, '..'), { recursive: true });
    try {
      writeFileSync(
        paths.reserved,
        `${canonicalAcceptanceJson({
          ...acceptanceNonceReservationRecord(value.requestPayload),
          invocationId: 'different-invocation',
        })}\n`,
      );
      expect(() => consumeInstalledAcceptanceNonce(value.requestPayload, root, NOW)).toThrow(
        'reservation binding is invalid',
      );
      expect(existsSync(paths.reserved)).toBe(true);
      expect(existsSync(paths.consumed)).toBe(false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('rejects protected readiness arguments in a normal production build', () => {
    const value = fixture();
    expect(() =>
      authorizeInstalledAcceptance({ ...value.options, acceptanceBuild: false }),
    ).toThrow('unavailable in this build');
  });

  it('binds the Windows login-start marker to the signed command', () => {
    const value = fixture();
    expect(() =>
      authorizeInstalledAcceptance({
        ...value.options,
        argv: [...value.options.argv, '--talking-quill-login-start'],
      }),
    ).toThrow('login-start flag does not match signed request');
  });

  it('rejects an argv field that differs from the signed request', () => {
    const value = fixture();
    const argv = value.options.argv.map((argument) =>
      argument.startsWith('--talking-quill-launch-correlation=')
        ? `--talking-quill-launch-correlation=${'55'.repeat(32)}`
        : argument,
    );
    expect(() => authorizeInstalledAcceptance({ ...value.options, argv })).toThrow(
      'does not match signed field launchCorrelation',
    );
  });

  it('rejects expired, overlong, and noncanonical requests', () => {
    const expired = fixture({ issuedAtMs: NOW - 120_000, expiresAtMs: NOW - 1 });
    expect(() => authorizeInstalledAcceptance(expired.options)).toThrow('validity interval');

    const overlong = fixture({
      issuedAtMs: NOW - 2_000,
      expiresAtMs: NOW - 2_000 + 5 * 60_000 + 1,
    });
    expect(() => authorizeInstalledAcceptance(overlong.options)).toThrow('validity interval');

    const noncanonical = fixture();
    const requestIndex = noncanonical.options.argv.findIndex((value) =>
      value.startsWith('--talking-quill-acceptance-request='),
    );
    const requestArgument = noncanonical.options.argv[requestIndex];
    const encoded = requestArgument?.split('=', 2)[1];
    if (encoded === undefined) throw new Error('Test request argument is missing');
    const spaced = Buffer.from(
      JSON.stringify(JSON.parse(Buffer.from(encoded, 'base64url').toString('utf8')), null, 2),
    ).toString('base64url');
    const argv = [...noncanonical.options.argv];
    argv[requestIndex] = `--talking-quill-acceptance-request=${spaced}`;
    expect(() => authorizeInstalledAcceptance({ ...noncanonical.options, argv })).toThrow(
      'not canonical',
    );
  });

  it('rejects a request signature made by a key outside the signed manifest', () => {
    const value = fixture();
    const unrelated = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const forged = envelope(value.requestPayload, unrelated.privateKey);
    const argv = value.options.argv.map((argument) =>
      argument.startsWith('--talking-quill-acceptance-request=')
        ? `--talking-quill-acceptance-request=${forged}`
        : argument,
    );
    expect(() => authorizeInstalledAcceptance({ ...value.options, argv })).toThrow(
      'signature is invalid',
    );
  });
});
