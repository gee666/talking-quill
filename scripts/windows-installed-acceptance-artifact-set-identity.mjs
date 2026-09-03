import { createHash } from 'node:crypto';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import { ACCEPTANCE_FAULT_PHASES } from './windows-installed-acceptance-schedule.mjs';

const HEX = /^[0-9a-f]{64}$/u;
const SOURCE = /^[0-9a-f]{40}$/u;
const DOMAIN = Buffer.from('TalkingQuill/windows-installed-acceptance-producer-artifact-set/v1\0');

export function createProducerArtifactSetIdentity(payload) {
  validatePayload(payload);
  return createHash('sha256').update(DOMAIN).update(canonicalAcceptanceJson(payload)).digest('hex');
}

export function producerArtifactSetPayloadFromPlan(plan, payloadEntries) {
  const candidate = plan.artifacts.candidate;
  const repair = plan.artifacts.repair;
  const role = (name) => candidate.metadata.roles.find((entry) => entry.role === name)?.sha256;
  return {
    schemaVersion: 1,
    sourceCommit: plan.sourceCommit,
    sourceTree: plan.sourceTree,
    buildId: plan.acceptance.buildId,
    candidate: {
      electronSha256: candidate.electron.sha256,
      appAsarSha256: candidate.appAsar.sha256,
      metadataSha256: candidate.metadataIdentity.sha256,
      releaseIdentitySha256: candidate.releaseIdentityIdentity.sha256,
      packageLayoutDigest: candidate.metadata.packageLayoutDigest,
      gatewaySha256: role('gateway'),
      ownerSha256: role('owner'),
    },
    update: {
      installerSha256: candidate.installer.sha256,
      packageLayoutDigest: candidate.metadata.packageLayoutDigest,
    },
    repair: {
      installerSha256: repair.installer.sha256,
      packageLayoutDigest: repair.metadata.packageLayoutDigest,
    },
    faults: ACCEPTANCE_FAULT_PHASES.map((phase) => ({
      phase,
      installerSha256: plan.artifacts.faults[phase].installer.sha256,
      packageLayoutDigest: plan.artifacts.faults[phase].metadata.packageLayoutDigest,
    })),
    native: {
      bootstrapSha256: plan.acceptance.acceptanceBootstrap.sha256,
      brokerSha256: plan.acceptance.acceptanceBroker.sha256,
      signerSha256: plan.acceptance.signerSha256,
      senderSha256: plan.acceptance.syntheticSender.sha256,
    },
    embeddedManifest: {
      sha256: plan.acceptance.buildManifestIdentity.sha256,
      validationKeySha256: sha256(
        Buffer.from(plan.acceptance.validationPublicKeySpkiBase64url, 'base64url'),
      ),
    },
    faultChainHeadSha256: plan.acceptance.faultValidation.chainHeadSha256,
    bundlePayloadInventory: payloadEntries.map(({ path, bytes, sha256: digest }) => ({
      path,
      bytes,
      sha256: digest,
    })),
  };
}

function validatePayload(payload) {
  if (
    payload?.schemaVersion !== 1 ||
    !SOURCE.test(payload.sourceCommit ?? '') ||
    !SOURCE.test(payload.sourceTree ?? '') ||
    !HEX.test(payload.buildId ?? '') ||
    !Array.isArray(payload.faults) ||
    canonicalAcceptanceJson(payload.faults.map(({ phase }) => phase)) !==
      canonicalAcceptanceJson(ACCEPTANCE_FAULT_PHASES) ||
    !Array.isArray(payload.bundlePayloadInventory) ||
    payload.bundlePayloadInventory.some(
      (entry, index, entries) =>
        typeof entry.path !== 'string' ||
        entry.path === 'producer-result.json' ||
        entry.path === 'bundle-manifest.json' ||
        !Number.isSafeInteger(entry.bytes) ||
        entry.bytes < 0 ||
        !HEX.test(entry.sha256 ?? '') ||
        (index > 0 && Buffer.from(entries[index - 1].path).compare(Buffer.from(entry.path)) >= 0),
    ) ||
    !allHashes(payload)
  ) {
    throw new Error('Producer artifact-set identity payload is invalid');
  }
}

function allHashes(payload) {
  const values = [
    ...Object.values(payload.candidate ?? {}),
    ...Object.values(payload.update ?? {}),
    ...Object.values(payload.repair ?? {}),
    ...Object.values(payload.native ?? {}),
    ...Object.values(payload.embeddedManifest ?? {}),
    payload.faultChainHeadSha256,
    ...payload.faults.flatMap((identity) =>
      Object.entries(identity)
        .filter(([name]) => name !== 'phase')
        .map(([, value]) => value),
    ),
  ];
  return values.every((value) => HEX.test(value ?? ''));
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
