import { execFileSync, spawnSync } from 'node:child_process';
import { createHash, X509Certificate } from 'node:crypto';
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

const subprocessEnvironment = sanitizedSubprocessEnvironment();
const hex32 = (value, name) => {
  if (!/^[0-9a-f]{64}$/u.test(value ?? ''))
    throw new Error(`${name} must be 64 lowercase hex characters`);
  return value;
};
const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
const b64 = (bytes) => Buffer.from(bytes).toString('base64url');

/**
 * Derives the release identity before either policy embeds it. The framed,
 * fixed-order transcript binds immutable role identities and release-policy
 * metadata, but excludes the digest itself and CMS signatures, avoiding a hash
 * or signature cycle.
 */
export function deriveReleaseBuildDigest(input) {
  const fields = [
    ['domain', 'talking-quill/macos-local-owner/release-identity/v2'],
    ['policyFormat', 'TQKOPOL1/1'],
    ['packageVersion', requiredText(input.packageVersion, 'package version')],
    ['architecture', input.arch === 'x64' || input.arch === 'arm64' ? input.arch : ''],
    ['signingMode', requiredText(input.signingMode, 'signing mode')],
    ['cmsCertificateSha256', hex32(input.cmsCertificateSha256, 'CMS certificate SHA-256')],
    [
      'codeCertificateSha256',
      optionalHex(input.codeCertificateSha256, 32, 'code certificate SHA-256'),
    ],
    ['codeCertificateSha1', optionalHex(input.codeCertificateSha1, 20, 'code certificate SHA-1')],
    ['gatewaySha256', hex32(input.gatewaySha256, 'gateway SHA-256')],
    ['gatewayIdentifier', requiredText(input.gatewayIdentifier, 'gateway identifier')],
    ['gatewayCdhash', exactHex(input.gatewayCdhash, 20, 'gateway CDHash')],
    ['gatewayRequirement', requiredText(input.gatewayRequirement, 'gateway requirement')],
    ['ownerSha256', hex32(input.ownerSha256, 'owner SHA-256')],
    ['ownerIdentifier', requiredText(input.ownerIdentifier, 'owner identifier')],
    ['ownerCdhash', exactHex(input.ownerCdhash, 20, 'owner CDHash')],
    ['ownerRequirement', requiredText(input.ownerRequirement, 'owner requirement')],
    ['bridgeSha256', hex32(input.bridgeSha256, 'bridge SHA-256')],
    ['bridgeIdentifier', requiredText(input.bridgeIdentifier, 'bridge identifier')],
    ['bridgeCdhash', exactHex(input.bridgeCdhash, 20, 'bridge CDHash')],
    ['bridgeRequirement', requiredText(input.bridgeRequirement, 'bridge requirement')],
    ['predecessorRelease', input.predecessor?.releaseBuildDigest ?? ''],
    ['predecessorGateway', input.predecessor?.gatewaySha256 ?? ''],
    ['predecessorOwner', input.predecessor?.ownerSha256 ?? ''],
  ];
  if (!['x64', 'arm64'].includes(fields[3][1])) throw new Error('invalid release architecture');
  if (input.predecessor !== null && input.predecessor !== undefined) {
    hex32(fields[20][1], 'predecessor release build');
    hex32(fields[21][1], 'predecessor gateway');
    hex32(fields[22][1], 'predecessor owner');
  }
  const transcript = [];
  for (const [name, value] of fields) {
    const nameBytes = Buffer.from(name, 'utf8');
    const valueBytes = Buffer.from(value, 'utf8');
    const frame = Buffer.alloc(6);
    frame.writeUInt16BE(nameBytes.length, 0);
    frame.writeUInt32BE(valueBytes.length, 2);
    transcript.push(frame, nameBytes, valueBytes);
  }
  return digest(Buffer.concat(transcript));
}

function requiredText(value, name) {
  if (typeof value !== 'string' || value.length === 0 || value.includes('\0'))
    throw new Error(`${name} is invalid`);
  return value;
}
function exactHex(value, bytes, name) {
  if (typeof value !== 'string' || !new RegExp(`^[0-9a-f]{${bytes * 2}}$`, 'u').test(value))
    throw new Error(`${name} is invalid`);
  return value;
}
function optionalHex(value, bytes, name) {
  return value === '' || value === undefined ? '' : exactHex(value, bytes, name);
}

export function encodeReleasePolicy({
  arch,
  buildDigest,
  gatewaySha256,
  ownerSha256,
  gatewayRequirement,
  ownerRequirement,
  predecessor = null,
}) {
  const bytes = Buffer.alloc(328);
  bytes.write('TQKOPOL1', 0, 'ascii');
  bytes.writeUInt16BE(1, 8);
  bytes[10] = 2;
  bytes[11] = arch === 'x64' ? 1 : arch === 'arm64' ? 2 : 0;
  if (bytes[11] === 0) throw new Error('macOS owner policy architecture must be x64 or arm64');
  bytes[12] = 2;
  bytes[13] = predecessor === null ? 0 : 1;
  Buffer.from(buildDigest, 'hex').copy(bytes, 16);
  Buffer.from(gatewaySha256, 'hex').copy(bytes, 48);
  Buffer.from(ownerSha256, 'hex').copy(bytes, 80);
  Buffer.from(digest(gatewayRequirement), 'hex').copy(bytes, 112);
  Buffer.from(digest(ownerRequirement), 'hex').copy(bytes, 144);
  for (const offset of [176, 200]) {
    bytes.writeUInt16BE(1, offset);
    bytes.writeUInt16BE(0, offset + 2);
    bytes.writeUInt32BE(1, offset + 4);
    bytes.writeBigUInt64BE(3n, offset + 8);
    bytes.writeBigUInt64BE(1n, offset + 16);
  }
  if (predecessor !== null) {
    Buffer.from(hex32(predecessor.releaseBuildDigest, 'predecessor release build'), 'hex').copy(
      bytes,
      224,
    );
    Buffer.from(hex32(predecessor.gatewaySha256, 'predecessor gateway'), 'hex').copy(bytes, 256);
    Buffer.from(hex32(predecessor.ownerSha256, 'predecessor owner'), 'hex').copy(bytes, 288);
    bytes[320] = 2;
    bytes[321] = bytes[11];
  }
  return bytes;
}

export function codesignObservation(path, mode, certificate = {}) {
  const result = spawnSync('/usr/bin/codesign', ['-dvvv', '--requirements', '-', path], {
    encoding: 'utf8',
    env: subprocessEnvironment,
  });
  if (result.status !== 0) throw new Error(`codesign inspection failed for ${path}`);
  const combined = `${result.stdout}${result.stderr}`;
  const cdhash = /CDHash=([0-9a-f]{40})/u.exec(combined)?.[1];
  if (cdhash === undefined) throw new Error(`codesign did not report a CDHash for ${path}`);
  const identifier = /Identifier=([^\r\n]+)/u.exec(combined)?.[1];
  if (identifier === undefined)
    throw new Error(`codesign did not report an identifier for ${path}`);
  const signing =
    mode === 'adhoc'
      ? { mode: 'ad_hoc', cdhash }
      : {
          mode: 'locally_trusted_self_signed',
          certificateSha256: hex32(certificate.sha256, 'local certificate SHA-256'),
          certificateRequirementHash: certificate.sha1,
          cdhash,
        };
  if (mode !== 'adhoc' && !/^[0-9a-f]{40}$/u.test(certificate.sha1 ?? ''))
    throw new Error('local certificate SHA-1 requirement hash is invalid');
  return { identifier, cdhash, signing };
}

export function verifyInstalledPolicyIdentities(wireText, { gateway, owner, bridge, mode }) {
  const wire = JSON.parse(wireText);
  const certificate =
    mode === 'adhoc'
      ? {}
      : observedCertificate(requiredEnvironment('TALKING_QUILL_MACOS_LOCAL_IDENTITY'));
  if (mode !== 'adhoc') {
    compareObservedPin(certificate.sha256, 'TALKING_QUILL_MACOS_LOCAL_CERT_SHA256');
    compareObservedPin(certificate.sha1, 'TALKING_QUILL_MACOS_LOCAL_CERT_SHA1');
  }
  const gatewayObservation = codesignObservation(gateway, mode, certificate);
  const ownerObservation = codesignObservation(owner, mode, certificate);
  const bridgeObservation = codesignObservation(bridge, mode, certificate);
  for (const role of [gateway, owner, bridge]) {
    if (mode !== 'adhoc') compareCodesignCertificate(role, certificate);
  }
  const expected = {
    gatewaySha256: digest(readFileSync(gateway)),
    ownerSha256: digest(readFileSync(owner)),
    bridgeSha256: digest(readFileSync(bridge)),
    gatewayIdentifier: gatewayObservation.identifier,
    ownerIdentifier: ownerObservation.identifier,
    bridgeIdentifier: bridgeObservation.identifier,
    gatewayCdhash: gatewayObservation.cdhash,
    ownerCdhash: ownerObservation.cdhash,
    bridgeCdhash: bridgeObservation.cdhash,
  };
  const actual = {
    gatewaySha256: wire.gateway?.executableSha256,
    ownerSha256: wire.owner?.executableSha256,
    bridgeSha256: wire.bridge?.executableSha256,
    gatewayIdentifier: gatewayObservation.identifier,
    ownerIdentifier: ownerObservation.identifier,
    bridgeIdentifier: bridgeObservation.identifier,
    gatewayCdhash: wire.gateway?.signing?.cdhash,
    ownerCdhash: wire.owner?.signing?.cdhash,
    bridgeCdhash: wire.bridge?.signing?.cdhash,
  };
  if (
    actual.gatewayIdentifier !== 'com.talkingquill.app.helper' ||
    actual.ownerIdentifier !== 'com.talkingquill.app.keyboard-owner' ||
    actual.bridgeIdentifier !== 'com.talkingquill.app.service-management' ||
    JSON.stringify(actual) !== JSON.stringify(expected)
  ) {
    throw new Error('Final macOS role identities do not match the sealed release policy');
  }
  return expected;
}

export function createInstalledPolicy({
  gateway,
  owner,
  bridge,
  arch,
  mode,
  cmsIdentity,
  installationIdentity,
  releaseVersion,
  predecessor,
}) {
  const localCertificate =
    mode === 'adhoc'
      ? {}
      : observedCertificate(requiredEnvironment('TALKING_QUILL_MACOS_LOCAL_IDENTITY'));
  if (mode !== 'adhoc') {
    compareObservedPin(localCertificate.sha256, 'TALKING_QUILL_MACOS_LOCAL_CERT_SHA256');
    compareObservedPin(localCertificate.sha1, 'TALKING_QUILL_MACOS_LOCAL_CERT_SHA1');
  }
  const cmsCertificate = observedCertificate(cmsIdentity);
  compareObservedPin(cmsCertificate.sha256, 'TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256');
  const gatewayObservation = codesignObservation(gateway, mode, localCertificate);
  const ownerObservation = codesignObservation(owner, mode, localCertificate);
  const bridgeObservation = codesignObservation(bridge, mode, localCertificate);
  if (
    gatewayObservation.identifier !== 'com.talkingquill.app.helper' ||
    ownerObservation.identifier !== 'com.talkingquill.app.keyboard-owner' ||
    bridgeObservation.identifier !== 'com.talkingquill.app.service-management'
  )
    throw new Error('macOS role signing identifiers are not exact');
  const gatewaySha256 = digest(readFileSync(gateway));
  const ownerSha256 = digest(readFileSync(owner));
  const bridgeSha256 = digest(readFileSync(bridge));
  const gatewayRequirement =
    mode === 'adhoc'
      ? `identifier "com.talkingquill.app.helper" and cdhash H"${gatewayObservation.cdhash}" and not anchor apple`
      : `identifier "com.talkingquill.app.helper" and anchor trusted and certificate leaf = H"${localCertificate.sha1}" and certificate root = H"${localCertificate.sha1}" and not anchor apple`;
  const ownerRequirement =
    mode === 'adhoc'
      ? `identifier "com.talkingquill.app.keyboard-owner" and cdhash H"${ownerObservation.cdhash}" and not anchor apple`
      : `identifier "com.talkingquill.app.keyboard-owner" and anchor trusted and certificate leaf = H"${localCertificate.sha1}" and certificate root = H"${localCertificate.sha1}" and not anchor apple`;
  const bridgeRequirement =
    mode === 'adhoc'
      ? `identifier "com.talkingquill.app.service-management" and cdhash H"${bridgeObservation.cdhash}" and not anchor apple`
      : `identifier "com.talkingquill.app.service-management" and anchor trusted and certificate leaf = H"${localCertificate.sha1}" and certificate root = H"${localCertificate.sha1}" and not anchor apple`;
  const buildDigest = deriveReleaseBuildDigest({
    packageVersion: requiredText(releaseVersion, 'release version'),
    arch,
    signingMode: mode,
    cmsCertificateSha256: cmsCertificate.sha256,
    codeCertificateSha256: localCertificate.sha256 ?? '',
    codeCertificateSha1: localCertificate.sha1 ?? '',
    gatewaySha256,
    gatewayIdentifier: gatewayObservation.identifier,
    gatewayCdhash: gatewayObservation.cdhash,
    gatewayRequirement,
    ownerSha256,
    ownerIdentifier: ownerObservation.identifier,
    ownerCdhash: ownerObservation.cdhash,
    ownerRequirement,
    bridgeSha256,
    bridgeIdentifier: bridgeObservation.identifier,
    bridgeCdhash: bridgeObservation.cdhash,
    bridgeRequirement,
    predecessor,
  });
  const policy = encodeReleasePolicy({
    arch,
    buildDigest,
    gatewaySha256,
    ownerSha256,
    gatewayRequirement,
    ownerRequirement,
    predecessor,
  });
  const bridgePolicy = encodeReleasePolicy({
    arch,
    buildDigest,
    gatewaySha256: bridgeSha256,
    ownerSha256: bridgeSha256,
    gatewayRequirement: bridgeRequirement,
    ownerRequirement: bridgeRequirement,
    predecessor: null,
  });
  const temporary = mkdtempSync(join(process.cwd(), 'tmp/macos-policy-'));
  try {
    const signature = signAndVerifyCms(
      temporary,
      'roles',
      policy,
      cmsCertificate.selector,
      cmsCertificate,
    );
    const bridgeSignature = signAndVerifyCms(
      temporary,
      'bridge',
      bridgePolicy,
      cmsCertificate.selector,
      cmsCertificate,
    );
    const installedApp =
      process.env.TALKING_QUILL_MACOS_INSTALLED_APP_PATH ?? '/Applications/Talking Quill.app';
    if (!installedApp.startsWith('/') || installedApp.includes('/../'))
      throw new Error('Installed app path must be absolute and normalized');
    const wire = {
      releaseBuildDigest: buildDigest,
      installationIdentityDigest: hex32(installationIdentity, 'installation identity'),
      gateway: {
        canonicalExecutablePath: `${installedApp}/Contents/Resources/helper/talking-quill-helper`,
        executableSha256: gatewaySha256,
        captureAuthorized: true,
        signing: gatewayObservation.signing,
      },
      owner: {
        canonicalExecutablePath: `${installedApp}/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner`,
        executableSha256: ownerSha256,
        captureAuthorized: true,
        signing: ownerObservation.signing,
      },
      bridge: {
        canonicalExecutablePath: `${installedApp}/Contents/MacOS/talking-quill-macos-service-bridge`,
        executableSha256: bridgeSha256,
        captureAuthorized: false,
        signing: bridgeObservation.signing,
      },
      gatewayReleasePolicy: b64(policy),
      gatewayReleasePolicySignature: b64(signature),
      ownerReleasePolicy: b64(policy),
      ownerReleasePolicySignature: b64(signature),
      bridgeReleasePolicy: b64(bridgePolicy),
      bridgeReleasePolicySignature: b64(bridgeSignature),
    };
    return `${JSON.stringify(wire)}\n`;
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}

function observedCertificate(identity) {
  const identities = execFileSync('/usr/bin/security', ['find-identity', '-v'], {
    encoding: 'utf8',
    env: subprocessEnvironment,
  })
    .split(/\r?\n/u)
    .map((line) => /^\s*\d+\)\s+([0-9A-F]{40})\s+"([^"]+)"/u.exec(line))
    .filter((match) => match !== null && match[2] === identity);
  if (identities.length !== 1)
    throw new Error(`Signing identity must resolve uniquely: ${identity}`);
  const selector = identities[0][1].toLowerCase();
  const pem = execFileSync('/usr/bin/security', ['find-certificate', '-a', '-p'], {
    encoding: 'utf8',
    env: subprocessEnvironment,
  });
  const certificates =
    pem.match(/-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/gu) ?? [];
  const matches = certificates
    .map((value) => new X509Certificate(value).raw)
    .filter((raw) => createHash('sha1').update(raw).digest('hex') === selector);
  if (matches.length !== 1)
    throw new Error(`Signing certificate must resolve uniquely: ${selector}`);
  return {
    selector,
    raw: matches[0],
    sha256: digest(matches[0]),
    sha1: selector,
  };
}

function signAndVerifyCms(directory, name, policy, selector, expectedCertificate) {
  const policyPath = join(directory, `${name}.bin`);
  const signaturePath = join(directory, `${name}.cms`);
  const certificatePath = join(directory, `${name}-signers.pem`);
  const exactSignerPath = join(directory, `${name}-exact-signer.pem`);
  writeFileSync(policyPath, policy);
  execFileSync(
    '/usr/bin/security',
    ['cms', '-S', '-N', selector, '-i', policyPath, '-o', signaturePath],
    { stdio: 'inherit', env: subprocessEnvironment },
  );
  execFileSync('/usr/bin/security', ['cms', '-D', '-i', signaturePath, '-c', policyPath], {
    stdio: 'ignore',
    env: subprocessEnvironment,
  });
  execFileSync(
    '/usr/bin/openssl',
    [
      'cms',
      '-verify',
      '-binary',
      '-inform',
      'DER',
      '-in',
      signaturePath,
      '-content',
      policyPath,
      '-noverify',
      '-certsout',
      certificatePath,
      '-out',
      '/dev/null',
    ],
    { stdio: 'ignore', env: subprocessEnvironment },
  );
  writeFileSync(exactSignerPath, new X509Certificate(expectedCertificate.raw).toString());
  // `-nointern` ignores every embedded certificate. Verification can succeed
  // only when the supplied exact leaf matches SignerInfo SID and signature.
  execFileSync(
    '/usr/bin/openssl',
    [
      'cms',
      '-verify',
      '-binary',
      '-inform',
      'DER',
      '-in',
      signaturePath,
      '-content',
      policyPath,
      '-nointern',
      '-certfile',
      exactSignerPath,
      '-noverify',
      '-out',
      '/dev/null',
    ],
    { stdio: 'ignore', env: subprocessEnvironment },
  );
  const signerPem = readFileSync(certificatePath, 'utf8');
  const signerCertificates =
    signerPem.match(/-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/gu) ?? [];
  const exactSignerCount = signerCertificates.filter(
    (value) => digest(new X509Certificate(value).raw) === expectedCertificate.sha256,
  ).length;
  if (exactSignerCount !== 1)
    throw new Error('Actual CMS signer certificate does not match the unique pinned identity');
  return readFileSync(signaturePath);
}

function compareCodesignCertificate(path, expectedCertificate) {
  const directory = mkdtempSync(join(process.cwd(), 'tmp/macos-codesign-cert-'));
  try {
    const prefix = join(directory, 'certificate');
    execFileSync('/usr/bin/codesign', ['-d', '--extract-certificates', prefix, path], {
      stdio: 'ignore',
      env: subprocessEnvironment,
    });
    const leaf = readFileSync(`${prefix}0`);
    if (
      digest(leaf) !== expectedCertificate.sha256 ||
      createHash('sha1').update(leaf).digest('hex') !== expectedCertificate.sha1
    )
      throw new Error(`Actual codesign leaf certificate does not match pins: ${path}`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function requiredEnvironment(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}

function compareObservedPin(actual, name) {
  const declared = process.env[name]?.trim();
  if (declared === undefined || declared !== actual)
    throw new Error(`${name} does not match the observed signing certificate`);
}
