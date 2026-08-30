'use strict';

const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { mkdirSync, readFileSync, writeFileSync } = require('node:fs');
const { join, resolve } = require('node:path');
const signWithLeastPrivilege = require('./mac-sign.cjs');

module.exports = async function signMacosOwner(configuration) {
  const app = configuration.app;
  const gateway = join(app, 'Contents', 'Resources', 'helper', 'talking-quill-helper');
  const nested = join(app, 'Contents', 'Library', 'LoginItems', 'Talking Quill Keyboard Owner.app');
  const owner = join(nested, 'Contents', 'MacOS', 'talking-quill-keyboard-owner');
  const bridge = join(app, 'Contents', 'MacOS', 'talking-quill-macos-service-bridge');
  const outerResources = join(app, 'Contents', 'Resources');
  const mode = process.env.TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE ?? 'self-signed';
  if (!['self-signed', 'adhoc'].includes(mode)) throw new Error('Unknown local macOS signing mode');
  const identity =
    mode === 'adhoc' ? '-' : resolveUniqueIdentity(required('TALKING_QUILL_MACOS_LOCAL_IDENTITY'));
  const cmsIdentity = required('TALKING_QUILL_MACOS_POLICY_CMS_IDENTITY');
  const ownerEntitlements = resolve(__dirname, 'entitlements.keyboard-owner.mac.plist');

  // First sign Electron's complete ordinary graph. Policy-bound roles receive
  // their final explicit identities only after this traversal has finished.
  await signWithLeastPrivilege({
    ...configuration,
    identity,
    strictVerify: false,
    preAutoEntitlements: false,
    preEmbedProvisioningProfile: false,
    optionsForFile(filePath) {
      return configuration.optionsForFile?.(filePath) ?? {};
    },
  });
  // Signing the nested app assigns its fixed CFBundleIdentifier to the owner
  // main executable. The gateway is not a bundle and needs an explicit ID.
  signBundle(nested, identity, ownerEntitlements);
  signRole(gateway, identity, 'com.talkingquill.app.helper');
  signRole(bridge, identity, 'com.talkingquill.app.service-management');

  const before = roleIdentity(gateway, owner, bridge);
  const { inspectMacosOuterIdentity } = await import('../scripts/macos-outer-identity.mjs');
  const outerIdentity = inspectMacosOuterIdentity(app);
  const arch = process.env.TALKING_QUILL_PACKAGE_ARCH;
  if (!['x64', 'arm64'].includes(arch)) throw new Error('TALKING_QUILL_PACKAGE_ARCH is required');
  const { createInstalledPolicy, verifyInstalledPolicyIdentities } =
    await import('../scripts/macos-owner-policy.mjs');
  const policy = createInstalledPolicy({
    gateway,
    owner,
    bridge,
    arch,
    mode: mode === 'adhoc' ? 'adhoc' : 'self-signed',
    cmsIdentity,
    installationIdentity: required('TALKING_QUILL_MACOS_INSTALLATION_ID'),
    releaseVersion: require('../app/package.json').version,
    predecessor: predecessor(),
  });
  mkdirSync(outerResources, { recursive: true });
  // The policy cannot live inside the owner bundle: sealing it would mutate the
  // owner's embedded CodeDirectory and create a hash/signature cycle. Both roles
  // read this one outer sealed resource.
  writeFileSync(join(outerResources, 'keyboard-owner-r5m.json'), policy, { mode: 0o600 });
  const policyWire = JSON.parse(policy);
  const { RELEASE_PACKAGE_METADATA_NAME, writePackageReleaseMetadata } =
    await import('../scripts/release-package-metadata.mjs');
  await writePackageReleaseMetadata(join(outerResources, RELEASE_PACKAGE_METADATA_NAME), {
    version: require('../app/package.json').version,
    platform: 'mac',
    architecture: arch,
    packageRoot: resolve(app, '..'),
    predecessor: packagePredecessor(predecessor()),
    releaseBuildDigest: policyWire.releaseBuildDigest,
    outerIdentity: mode === 'adhoc' ? null : outerIdentity,
  });

  // Adding policy and immutable package metadata invalidates only the outer
  // resource envelope. Re-sign the outer bundle directly, never either
  // policy-bound role file.
  signBundle(app, identity, resolve(__dirname, 'entitlements.mac.plist'));
  const sealedOuterIdentity = inspectMacosOuterIdentity(app);
  if (mode !== 'adhoc' && JSON.stringify(sealedOuterIdentity) !== JSON.stringify(outerIdentity)) {
    throw new Error('The outer macOS designated requirement changed while sealing metadata');
  }
  const after = roleIdentity(gateway, owner, bridge);
  if (JSON.stringify(after) !== JSON.stringify(before)) {
    throw new Error('A policy-bound macOS role changed while sealing its containing bundles');
  }
  verifyInstalledPolicyIdentities(policy, { gateway, owner, bridge, mode });
  execFileSync('/usr/bin/codesign', ['--verify', '--deep', '--strict', app], { stdio: 'inherit' });
};

function signBundle(path, identity, entitlements) {
  execFileSync(
    '/usr/bin/codesign',
    [
      '--force',
      '--sign',
      identity,
      '--timestamp=none',
      '--options',
      'runtime',
      '--entitlements',
      entitlements,
      path,
    ],
    { stdio: 'inherit' },
  );
}
function signRole(path, identity, identifier) {
  execFileSync(
    '/usr/bin/codesign',
    [
      '--force',
      '--sign',
      identity,
      '--identifier',
      identifier,
      '--timestamp=none',
      '--options',
      'runtime',
      path,
    ],
    { stdio: 'inherit' },
  );
}
function roleIdentity(gateway, owner, bridge) {
  return {
    gatewaySha256: sha256(gateway),
    ownerSha256: sha256(owner),
    bridgeSha256: sha256(bridge),
    gatewayCode: execFileSync('/usr/bin/codesign', ['-dvvv', gateway], { encoding: 'utf8' }),
    ownerCode: execFileSync('/usr/bin/codesign', ['-dvvv', owner], { encoding: 'utf8' }),
    bridgeCode: execFileSync('/usr/bin/codesign', ['-dvvv', bridge], { encoding: 'utf8' }),
  };
}
function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}
function required(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required`);
  return value;
}
function resolveUniqueIdentity(name) {
  const output = execFileSync('/usr/bin/security', ['find-identity', '-v', '-p', 'codesigning'], {
    encoding: 'utf8',
  });
  const matches = output
    .split(/\r?\n/u)
    .map((line) => /^\s*\d+\)\s+([0-9A-F]{40})\s+"([^"]+)"/u.exec(line))
    .filter((match) => match !== null && match[2] === name);
  if (matches.length !== 1) throw new Error(`Code-signing identity must resolve uniquely: ${name}`);
  return matches[0][1].toLowerCase();
}

function packagePredecessor(value) {
  if (value === null) return null;
  const version = process.env.TALKING_QUILL_PREDECESSOR_VERSION;
  if (!/^\d+\.\d+\.\d+$/u.test(version ?? '')) {
    throw new Error('The package predecessor version must be explicit');
  }
  return {
    platform: 'mac',
    architecture: process.env.TALKING_QUILL_PACKAGE_ARCH,
    version,
    ...value,
  };
}

function predecessor() {
  const build = process.env.TALKING_QUILL_MACOS_PREDECESSOR_BUILD;
  const gateway = process.env.TALKING_QUILL_MACOS_PREDECESSOR_GATEWAY_SHA256;
  const owner = process.env.TALKING_QUILL_MACOS_PREDECESSOR_OWNER_SHA256;
  if ([build, gateway, owner].every((value) => value === undefined)) return null;
  if (![build, gateway, owner].every((value) => /^[0-9a-f]{64}$/u.test(value ?? '')))
    throw new Error('Predecessor enrollment must be complete and exact');
  return { releaseBuildDigest: build, gatewaySha256: gateway, ownerSha256: owner };
}
