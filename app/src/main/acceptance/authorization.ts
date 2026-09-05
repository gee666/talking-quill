import {
  canonicalAcceptanceJson,
  decodeCanonicalEnvelope,
  readP256PublicKey,
  verifyPayload,
} from './authorization-envelope';
export {
  canonicalAcceptanceJson,
  encodeCanonicalAcceptanceEnvelope,
  acceptancePayloadBytes,
} from './authorization-envelope';

import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, renameSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  AcceptanceBuildManifestPayloadSchema,
  AcceptanceBuildManifestSchema,
  AcceptanceRunRequestPayloadSchema,
  AcceptanceRunRequestSchema,
  type AcceptanceRunRequestPayload,
} from './authorization-schema';

export const MAX_ACCEPTANCE_REQUEST_LIFETIME_MS = 5 * 60 * 1_000;
const CLOCK_SKEW_MS = 30_000;
const REQUEST_PREFIX = '--talking-quill-acceptance-request=';

const argumentFields = {
  readinessPipe: '--talking-quill-installed-readiness-pipe=',
  launchCorrelation: '--talking-quill-launch-correlation=',
  automationArmedPipe: '--talking-quill-automation-armed-pipe=',
  automationCase: '--talking-quill-automation-case=',
  lifecycleUserData: '--talking-quill-installed-lifecycle-user-data=',
} as const;

const physicalFlag = '--talking-quill-installed-physical-observation';
const automationFlag = '--talking-quill-installed-automation-validation';
const loginStartFlag = '--talking-quill-login-start';

export const INSTALLED_ACCEPTANCE_ARGUMENT_PREFIXES = Object.freeze([
  REQUEST_PREFIX,
  ...Object.values(argumentFields),
]);
export const INSTALLED_ACCEPTANCE_ARGUMENT_FLAGS = Object.freeze([physicalFlag, automationFlag]);

export interface AcceptanceAuthorizationOptions {
  readonly acceptanceBuild: boolean;
  readonly encodedBuildManifest: string;
  readonly manifestPublicKeySpkiBase64url: string;
  readonly sourceRevision: string;
  readonly argv: readonly string[];
  readonly installed?: {
    readonly resourcesPath: string;
    readonly executablePath: string;
    readonly architecture: string;
    readonly version: string;
  };
  readonly nowMs?: number;
}

export function hasInstalledAcceptanceArguments(argv: readonly string[]): boolean {
  return argv.some(
    (argument) =>
      INSTALLED_ACCEPTANCE_ARGUMENT_FLAGS.some((flag) => argument.startsWith(flag)) ||
      INSTALLED_ACCEPTANCE_ARGUMENT_PREFIXES.some((prefix) => argument.startsWith(prefix)),
  );
}

export function authorizeInstalledAcceptance(
  options: AcceptanceAuthorizationOptions,
): AcceptanceRunRequestPayload {
  const encodedRequest = readSingleValue(options.argv, REQUEST_PREFIX);
  if (encodedRequest === null) throw new Error('Signed installed acceptance request is missing');
  const payload = authorizeInstalledAcceptanceRequest({ ...options, encodedRequest });
  assertArgumentsMatchRequest(options.argv, payload);
  return payload;
}

export function authorizeInstalledAcceptanceRequest(
  options: Omit<AcceptanceAuthorizationOptions, 'argv'> & { readonly encodedRequest: string },
): AcceptanceRunRequestPayload {
  if (!options.acceptanceBuild)
    throw new Error('Installed acceptance is unavailable in this build');

  const nowMs = options.nowMs ?? Date.now();
  const manifest = decodeCanonicalEnvelope(
    options.encodedBuildManifest,
    (value) => AcceptanceBuildManifestSchema.parse(value),
    'acceptance build manifest',
  );
  const manifestKey = readP256PublicKey(
    options.manifestPublicKeySpkiBase64url,
    'acceptance manifest public key',
  );
  if (!verifyPayload(manifest.payload, manifest.signatureBase64url, manifestKey)) {
    throw new Error('Acceptance build manifest signature is invalid');
  }
  const manifestPayload = AcceptanceBuildManifestPayloadSchema.parse(manifest.payload);
  if (manifestPayload.sourceRevision !== options.sourceRevision) {
    throw new Error('Acceptance build manifest does not match this source revision');
  }
  if (nowMs + CLOCK_SKEW_MS < manifestPayload.validFromMs || nowMs > manifestPayload.validUntilMs) {
    throw new Error('Acceptance build manifest is outside its validity interval');
  }
  if (options.installed !== undefined)
    verifyInstalledAcceptanceBinding(manifestPayload, options.installed);

  const request = decodeCanonicalEnvelope(
    options.encodedRequest,
    (value) => AcceptanceRunRequestSchema.parse(value),
    'acceptance run request',
  );
  const requestKey = readP256PublicKey(
    manifestPayload.requestPublicKeySpkiBase64url,
    'acceptance request public key',
  );
  if (!verifyPayload(request.payload, request.signatureBase64url, requestKey)) {
    throw new Error('Acceptance run request signature is invalid');
  }
  const payload = AcceptanceRunRequestPayloadSchema.parse(request.payload);
  if (payload.buildId !== manifestPayload.buildId) {
    throw new Error('Acceptance run request targets a different build');
  }
  const effectiveRunExpiry = Math.min(
    payload.runWindow.expiresAtMs,
    payload.runWindow.notBeforeMs + payload.runWindow.maxTotalRunMs,
  );
  if (
    payload.expiresAtMs - payload.issuedAtMs > MAX_ACCEPTANCE_REQUEST_LIFETIME_MS ||
    nowMs + CLOCK_SKEW_MS < payload.issuedAtMs ||
    nowMs < payload.runWindow.notBeforeMs ||
    nowMs > payload.expiresAtMs ||
    nowMs > effectiveRunExpiry ||
    nowMs > payload.runWindow.notBeforeMs + payload.latestStartOffsetMs ||
    payload.expiresAtMs < payload.runWindow.notBeforeMs + payload.deadlineOffsetMs ||
    payload.expiresAtMs > payload.runWindow.expiresAtMs ||
    payload.runWindow.notBeforeMs < manifestPayload.validFromMs ||
    payload.runWindow.expiresAtMs > manifestPayload.validUntilMs
  ) {
    throw new Error('Acceptance run request is outside its validity interval');
  }
  return Object.freeze(payload);
}

export function acceptanceNonceReservationRecord(
  payload: AcceptanceRunRequestPayload,
): Readonly<Record<string, unknown>> {
  return Object.freeze({
    version: 1,
    buildId: payload.buildId,
    requestNonce: payload.requestNonce,
    invocationId: payload.invocationId,
    runWindow: payload.runWindow,
    latestStartOffsetMs: payload.latestStartOffsetMs,
    deadlineOffsetMs: payload.deadlineOffsetMs,
    requestExpiresAtMs: payload.expiresAtMs,
  });
}

export function acceptanceNonceLedgerPaths(
  stableTempRoot: string,
  buildId: string,
  requestNonce: string,
): Readonly<{ reserved: string; consumed: string }> {
  const buildRoot = resolve(
    stableTempRoot,
    'TalkingQuillInstalledAcceptanceNonceLedger',
    'v1',
    buildId,
  );
  return Object.freeze({
    reserved: resolve(buildRoot, `${requestNonce}.reserved.json`),
    consumed: resolve(buildRoot, `${requestNonce}.consumed.json`),
  });
}

export function consumeInstalledAcceptanceNonce(
  payload: AcceptanceRunRequestPayload,
  stableTempRoot: string,
  nowMs = Date.now(),
): void {
  const strictExpiry = Math.min(
    payload.expiresAtMs,
    payload.runWindow.expiresAtMs,
    payload.runWindow.notBeforeMs + payload.runWindow.maxTotalRunMs,
    payload.runWindow.notBeforeMs + payload.latestStartOffsetMs,
  );
  if (nowMs > strictExpiry) {
    throw new Error('Acceptance run request expired before nonce consumption');
  }
  const paths = acceptanceNonceLedgerPaths(stableTempRoot, payload.buildId, payload.requestNonce);
  const expected = `${canonicalAcceptanceJson(acceptanceNonceReservationRecord(payload))}\n`;
  try {
    lstatSync(paths.consumed);
    throw new Error('Acceptance run request nonce was already consumed');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
  }
  let reserved: string;
  try {
    reserved = readFileSync(paths.reserved, 'utf8');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      throw new Error('Acceptance run request nonce was not reserved');
    }
    throw error;
  }
  if (reserved !== expected) {
    throw new Error('Acceptance run request reservation binding is invalid');
  }
  try {
    lstatSync(paths.consumed);
    throw new Error('Acceptance run request nonce was already consumed');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
  }
  try {
    renameSync(paths.reserved, paths.consumed);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') {
      throw new Error('Acceptance run request nonce was concurrently consumed');
    }
    throw error;
  }
}

function verifyInstalledAcceptanceBinding(
  manifest: ReturnType<typeof AcceptanceBuildManifestPayloadSchema.parse>,
  installed: NonNullable<AcceptanceAuthorizationOptions['installed']>,
): void {
  if (
    manifest.architecture !== installed.architecture ||
    manifest.packageVersion !== installed.version
  ) {
    throw new Error('Acceptance manifest architecture/version binding is invalid');
  }
  const ownerManifestPath = resolve(installed.resourcesPath, 'keyboard-owner-release-v1.json');
  const installedRoot = resolve(installed.resourcesPath, '..');
  const ownerManifestBytes = readFileSync(ownerManifestPath);
  const ownerManifest = JSON.parse(ownerManifestBytes.toString('utf8')) as {
    releaseBuildDigest?: unknown;
    packageLayoutDigest?: unknown;
    roles?: { role?: unknown; path?: unknown; sha256?: unknown }[];
  };
  const role = (name: string) => ownerManifest.roles?.find((value) => value.role === name);
  const gateway = role('gateway');
  const owner = role('owner');
  const bindings = [
    [sha256(ownerManifestBytes), manifest.ownerManifestSha256],
    [ownerManifest.releaseBuildDigest, manifest.releaseBuildDigest],
    [ownerManifest.packageLayoutDigest, manifest.packageLayoutDigest],
    [sha256(readFileSync(installed.executablePath)), manifest.electronSha256],
    [sha256(readFileSync(resolve(installed.resourcesPath, 'app.asar'))), manifest.appAsarSha256],
    [gateway?.sha256, manifest.gatewaySha256],
    [owner?.sha256, manifest.ownerSha256],
    [
      typeof gateway?.path === 'string'
        ? sha256(readFileSync(resolve(installedRoot, gateway.path)))
        : null,
      manifest.gatewaySha256,
    ],
    [
      typeof owner?.path === 'string'
        ? sha256(readFileSync(resolve(installedRoot, owner.path)))
        : null,
      manifest.ownerSha256,
    ],
  ];
  if (bindings.some(([observed, expected]) => observed !== expected)) {
    throw new Error('Acceptance manifest does not bind the exact installed runtime');
  }
}

function sha256(bytes: Buffer): string {
  return createHash('sha256').update(bytes).digest('hex');
}

function assertArgumentsMatchRequest(
  argv: readonly string[],
  payload: AcceptanceRunRequestPayload,
): void {
  for (const [field, prefix] of Object.entries(argumentFields) as [
    keyof typeof argumentFields,
    string,
  ][]) {
    const expected = payload[field];
    if (readSingleValue(argv, prefix) !== expected) {
      throw new Error(`Installed acceptance argument does not match signed field ${field}`);
    }
  }
  if (argv.some((argument) => argument.startsWith(physicalFlag) && argument !== physicalFlag)) {
    throw new Error('Installed physical observation flag is malformed');
  }
  if (argv.some((argument) => argument.startsWith(automationFlag) && argument !== automationFlag)) {
    throw new Error('Installed automation validation flag is malformed');
  }
  if (countExact(argv, physicalFlag) !== Number(payload.physicalObservation)) {
    throw new Error('Installed physical observation flag does not match signed request');
  }
  if (countExact(argv, automationFlag) !== Number(payload.automationValidation)) {
    throw new Error('Installed automation validation flag does not match signed request');
  }
  if (countExact(argv, loginStartFlag) !== Number(payload.command === 'login-marker')) {
    throw new Error('Installed login-start flag does not match signed request');
  }
}

function readSingleValue(argv: readonly string[], prefix: string): string | null {
  const values = argv.filter((argument) => argument.startsWith(prefix));
  if (values.length > 1) throw new Error(`Installed acceptance argument is duplicated: ${prefix}`);
  const [argument] = values;
  if (argument === undefined) return null;
  const value = argument.slice(prefix.length);
  if (value.length === 0) throw new Error(`Installed acceptance argument is empty: ${prefix}`);
  return value;
}

function countExact(argv: readonly string[], value: string): number {
  return argv.filter((argument) => argument === value).length;
}
