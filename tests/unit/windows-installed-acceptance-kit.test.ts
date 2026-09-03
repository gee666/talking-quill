import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { verifyAcceptanceBundleArchive } from '../../scripts/windows-installed-acceptance-bundle.mjs';
import { signAcceptancePayload } from '../../scripts/windows-installed-acceptance-signer.mjs';
import {
  ACCEPTANCE_FAULT_PHASES,
  ACCEPTANCE_REQUEST_SCHEDULE,
} from '../../scripts/windows-installed-acceptance.mjs';
import {
  buildInstalledAcceptanceKit,
  sanitizedBuildEnvironment,
} from '../../tmp/build-windows-installed-acceptance-kit.mjs';

const builderSource = readFileSync('tmp/build-windows-installed-acceptance-kit.mjs', 'utf8');
const signerSource = readFileSync('scripts/windows-installed-acceptance-signer.mjs', 'utf8');
const fixtureRoots: string[] = [];
const sha256 = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');

afterEach(async () => {
  await Promise.all(
    fixtureRoots.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

describe('Windows installed-acceptance kit', () => {
  it('keeps every private signing value out of broad build environments', () => {
    expect(
      sanitizedBuildEnvironment({
        PATH: 'tools',
        TALKING_QUILL_ACCEPTANCE_MANIFEST_PRIVATE_KEY_PEM: 'manifest-secret',
        TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64: 'update-secret',
        TALKING_QUILL_ACCEPTANCE_REQUEST_PRIVATE_KEY: 'request-secret',
      }),
    ).toEqual({ PATH: 'tools' });
    expect(builderSource).not.toContain('readRegular(options.requestPrivateKeyPath)');
    expect(builderSource).not.toContain('privateKeyPkcs8Base64');
    expect(signerSource).not.toContain('readFileSync(resolve(privateKeyPath');
    expect(signerSource).not.toContain('createPrivateKey');
  });

  it('passes only the protected key path and payload to the minimal native signer', () => {
    const spawnProcess = vi.fn((...arguments_: unknown[]) => {
      void arguments_;
      return {
        status: 0,
        stderr: '',
        stdout: `${'11'.repeat(64)}\n04${'22'.repeat(64)}\n`,
      };
    });
    const payloadBytes = Buffer.from('fixed canonical payload');
    const signerPath = 'scripts/windows-installed-acceptance-signer.mjs';
    const signerSha256 = createHash('sha256').update(readFileSync(signerPath)).digest('hex');
    const result = signAcceptancePayload({
      signerPath,
      signerSha256,
      privateKeyPath: 'tmp/protected-request-key.der',
      payloadBytes,
      spawnProcess,
    });
    expect(result.signatureBase64url).toBe(
      Buffer.from('11'.repeat(64), 'hex').toString('base64url'),
    );
    const call = spawnProcess.mock.calls[0];
    if (call === undefined) throw new Error('Native signer was not spawned');
    const arguments_ = call[1] as string[];
    const options = call[2] as { input: Buffer; env: Record<string, string> };
    expect(arguments_).toEqual([
      '--private-key',
      expect.stringMatching(/protected-request-key\.der$/u),
    ]);
    expect(options.input).toBe(payloadBytes);
    expect(options.env).not.toHaveProperty('PATH');
    expect(JSON.stringify(options)).not.toContain('private-key-material');
    expect(() =>
      signAcceptancePayload({
        signerPath,
        signerSha256: '00'.repeat(32),
        privateKeyPath: 'tmp/protected-request-key.der',
        payloadBytes,
        spawnProcess,
      }),
    ).toThrow('signer identity is invalid');
    expect(spawnProcess).toHaveBeenCalledOnce();
  });

  it('builds and verifies a fixture kit with independent shared artifact entries', async () => {
    await mkdir(resolve('tmp'), { recursive: true });
    const fixtureRoot = await mkdtemp(resolve('tmp/kit-builder-test-'));
    fixtureRoots.push(fixtureRoot);
    const inputs = resolve(fixtureRoot, 'inputs');
    const unpackedRoot = resolve(inputs, 'unpacked');
    await mkdir(resolve(unpackedRoot, 'resources'), { recursive: true });
    await writeFile(resolve(unpackedRoot, 'resources/app.asar'), 'fixture app');
    await writeFile(resolve(unpackedRoot, 'resources/acceptance-manifest.json'), '{}\n');
    const installerBytes = Buffer.from('canonical installer fixture');
    const installerPath = resolve(inputs, 'installer.exe');
    const metadataPath = resolve(inputs, 'metadata.json');
    const releaseIdentityPath = resolve(inputs, 'release-identity.json');
    const validationEvidencePath = resolve(inputs, 'validation.json');
    const syntheticSenderPath = resolve(inputs, 'synthetic-sender.exe');
    const trustedLauncherPath = resolve(inputs, 'trusted-launcher.exe');
    await Promise.all([
      writeFile(installerPath, installerBytes),
      writeFile(metadataPath, '{}\n'),
      writeFile(releaseIdentityPath, '{}\n'),
      writeFile(validationEvidencePath, '{}\n'),
      writeFile(syntheticSenderPath, 'sender'),
      writeFile(trustedLauncherPath, 'launcher'),
    ]);
    const artifact = () => ({
      architecture: 'x64',
      installerPath,
      installerSha256: sha256(installerBytes),
      metadataPath,
      metadataSha256: sha256(Buffer.from('{}\n')),
      releaseIdentityPath,
      releaseIdentitySha256: sha256(Buffer.from('{}\n')),
      validationEvidencePath,
      validationEvidenceSha256: sha256(Buffer.from('{}\n')),
      unpackedRoot,
      electronPath: resolve(unpackedRoot, 'resources/app.asar'),
      appAsarPath: resolve(unpackedRoot, 'resources/app.asar'),
      electronRelativePath: 'resources/app.asar',
    });
    const runWindow = {
      notBeforeMs: 1_800_000_000_000,
      expiresAtMs: 1_800_004_800_000,
      maxTotalRunMs: 4_800_000,
    };
    const requestPayloads = Object.fromEntries(
      ACCEPTANCE_REQUEST_SCHEDULE.map(({ invocationId }) => [invocationId, {}]),
    );
    const requestNonces = Object.fromEntries(
      ACCEPTANCE_REQUEST_SCHEDULE.map(({ invocationId }, index) => [
        invocationId,
        index.toString(16).padStart(64, '0'),
      ]),
    );
    const faults = Object.fromEntries(ACCEPTANCE_FAULT_PHASES.map((phase) => [phase, artifact()]));
    const config = {
      architecture: 'x64',
      artifacts: {
        predecessor: artifact(),
        candidate: artifact(),
        fresh: artifact(),
        repair: artifact(),
        fault: artifact(),
        faults,
      },
      acceptance: {
        buildId: '12'.repeat(32),
        sourceRevision: 'a'.repeat(12),
        runWindow,
        requestPayloads,
        requestNonces,
        buildManifestPath: resolve(unpackedRoot, 'resources/acceptance-manifest.json'),
        syntheticSenderPath,
        trustedLauncherPath,
      },
    };
    const configPath = resolve(inputs, 'config.json');
    await writeFile(configPath, JSON.stringify(config));
    const sourceCommit = 'a'.repeat(40);
    const sourceTree = 'b'.repeat(40);
    const descriptor = {
      architecture: 'x64',
      installer: 'canonical.exe',
      sha256: sha256(installerBytes),
      sourceCommit,
      sourceTree,
    };
    const descriptorBytes = Buffer.from(`${JSON.stringify(descriptor)}\n`);
    const outputRoot = resolve(fixtureRoot, 'kit');
    const bundlePath = resolve(fixtureRoot, 'kit.zip');
    const result = await buildInstalledAcceptanceKit(
      {
        descriptorPath: resolve(inputs, 'RELEASE.json'),
        descriptorSha256: sha256(descriptorBytes),
        sourceRoot: fixtureRoot,
        configPath,
        requestPrivateKeyPath: resolve(inputs, 'request-key.der'),
        signerPath: resolve(inputs, 'signer.exe'),
        signerSha256: '34'.repeat(32),
        outputRoot,
        bundlePath,
      },
      {
        validateCanonicalRelease: () =>
          Promise.resolve({
            descriptor,
            descriptorBytes,
            installerPath,
            installerBytes,
          }),
        signAcceptancePayload: () => ({
          signatureBase64url: 'A'.repeat(86),
          publicKeySpkiBase64url: Buffer.from('fixture-public-key').toString('base64url'),
        }),
        createInstalledAcceptancePlan: (input: Record<string, unknown>) =>
          Promise.resolve({
            ...input,
            acceptance: { ...(input.acceptance as object), runWindow },
          }),
        validateAcceptanceRunSequence: () => ({ requests: [] }),
        verifyAcceptancePreflight: () => Promise.resolve({ result: 'passed' }),
      },
    );
    expect(result.bundlePath).toBe(bundlePath);
    const verified = await verifyAcceptanceBundleArchive(bundlePath, {
      architecture: 'x64',
      sourceCommit,
      sourceTree,
    });
    if (verified.entries === undefined) throw new Error('Fixture ZIP entries are missing');
    const evidenceEntry = verified.entries.find(({ path }) => path === 'evidence-input.json');
    if (evidenceEntry === undefined) throw new Error('Fixture evidence entry is missing');
    const evidence = JSON.parse(evidenceEntry.bytes.toString('utf8')) as {
      artifacts: {
        predecessor: Record<string, string>;
        fresh: Record<string, string>;
        fault: Record<string, string>;
        faults: Record<string, Record<string, string>>;
      };
    };
    expect(evidence.artifacts.fresh).toEqual(evidence.artifacts.predecessor);
    expect(evidence.artifacts.fault).toEqual(evidence.artifacts.faults.published);
    expect(evidence.artifacts.fresh.installerPath).toBe('artifacts/predecessor/installer.exe');
    expect(evidence.artifacts.fault.installerPath).toBe('artifacts/fault-published/installer.exe');
  }, 30_000);

  it('binds RELEASE buffers, ordered nonces, deterministic ZIP, and self-verification', () => {
    for (const contract of [
      "descriptor.version !== '0.0.69'",
      "descriptor.packageMode !== 'fresh'",
      "descriptor.variant !== 'canonical'",
      'parseTqpkg2(installerBytes, descriptor.architecture)',
      'ACCEPTANCE_FAULT_PHASES',
      'requestNonces',
      'writeFile(releaseCopy, imported.descriptorBytes',
      'writeFile(installerCopy, imported.installerBytes',
      'createDeterministicAcceptanceZip',
      'extractVerifiedAcceptanceBundle',
      'verifyAcceptanceBundleTree(selfCheckRoot',
    ]) {
      expect(builderSource).toContain(contract);
    }
  });
});
