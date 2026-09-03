import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  buildWindowsInstalledAcceptanceInputs,
  installedAcceptanceBuildEnvironment,
} from '../../scripts/build-windows-installed-acceptance-inputs.mjs';
import { buildWindowsInstalledAcceptanceArtifacts } from '../../scripts/windows-installed-acceptance-native-build.mjs';
import {
  ACCEPTANCE_FAULT_PHASES,
  ACCEPTANCE_REQUEST_SCHEDULE,
  MAX_ACCEPTANCE_RUN_MS,
} from '../../scripts/windows-installed-acceptance.mjs';

const roots: string[] = [];
const sourceCommit = '11'.repeat(20);
const sourceTree = '22'.repeat(20);
const hash = '33'.repeat(32);

function metadata() {
  return {
    version: '0.0.69',
    platform: 'win',
    architecture: 'x64',
    packageMode: 'fresh',
    sourceCommit,
    sourceTree,
    freshInstall: true,
    predecessor: null,
    releaseBuildDigest: '44'.repeat(32),
    roles: [
      {
        role: 'gateway',
        path: 'resources/helper/talking-quill-helper.exe',
        sha256: '55'.repeat(32),
      },
      {
        role: 'owner',
        path: 'resources/helper/talking-quill-keyboard-owner.exe',
        sha256: '66'.repeat(32),
      },
    ],
  };
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe('Windows installed-acceptance input producer', () => {
  it('orchestrates all inputs from an empty tmp directory with mocked heavy builds', async () => {
    await mkdir(resolve('tmp'), { recursive: true });
    const root = await mkdtemp(resolve('tmp/acceptance-inputs-integration-'));
    roots.push(root);
    const outputRoot = resolve(root, 'producer');
    const file = async (name: string) => {
      const path = resolve(root, name);
      await writeFile(path, name);
      return path;
    };
    const manifest = await file('manifest.txt');
    const sender = await file('sender.exe');
    const launcher = await file('launcher.exe');
    const validation = await file('validation.json');
    const artifact = { validationEvidencePath: validation };
    const faults = Object.fromEntries(ACCEPTANCE_FAULT_PHASES.map((phase) => [phase, artifact]));
    const produceArtifacts = vi.fn(() =>
      Promise.resolve({
        predecessor: artifact,
        signerPath: launcher,
        signerSha256: hash,
        acceptanceBrokerPath: launcher,
        acceptanceBrokerSha256: hash,
        acceptanceBootstrapPath: launcher,
        acceptanceBootstrapSha256: hash,
        candidate: artifact,
        repair: artifact,
        faults,
        buildManifestPath: manifest,
        manifestPublicKeySpkiBase64url: 'fixture_key',
        validationPublicKeySpkiBase64url: 'fixture_validation_key',
        validationChainHeadSha256: '88'.repeat(32),
        syntheticSenderPath: sender,
        trustedLauncherPath: launcher,
      }),
    );
    const assembler = vi.fn(({ configPath }: { configPath: string }) =>
      Promise.resolve({ configPath }),
    );
    const notBeforeMs = 1_900_000_000_000;
    const result = await buildWindowsInstalledAcceptanceInputs(
      {
        architecture: 'x64',
        descriptorPath: resolve(root, 'RELEASE.json'),
        descriptorSha256: hash,
        provenancePath: resolve(root, 'provenance.json'),
        provenanceSha256: hash,
        sourceRoot: '.',
        requestPrivateKeyPath: resolve(root, 'request.der'),
        manifestPrivateKeyPath: resolve(root, 'manifest.der'),
        updatePrivateKeyPath: resolve(root, 'update.der'),
        validationPrivateKeyPath: resolve(root, 'validation.der'),
        signerPath: resolve(root, 'signer.exe'),
        signerSha256: hash,
        notBeforeMs,
        expiresAtMs: notBeforeMs + MAX_ACCEPTANCE_RUN_MS,
        buildId: '77'.repeat(32),
        outputRoot,
      },
      {
        validateCanonicalRelease: () =>
          Promise.resolve({
            descriptor: {
              version: '0.0.69',
              architecture: 'x64',
              sourceCommit,
              sourceTree,
            },
            installerBytes: Buffer.from('mock installer'),
          }),
        currentSourceIdentity: () => ({ sourceCommit, sourceTree }),
        parseTqpkg2: () => ({
          contents: new Map([
            ['resources/keyboard-owner-release-v1.json', Buffer.from(JSON.stringify(metadata()))],
            ['resources/helper/talking-quill-helper.exe', Buffer.from('gateway')],
            ['resources/helper/talking-quill-keyboard-owner.exe', Buffer.from('owner')],
          ]),
        }),
        produceArtifacts,
        buildInstalledAcceptanceKit: assembler,
      },
    );
    const config = JSON.parse(await readFile(result.configPath as string, 'utf8')) as {
      acceptance: {
        requestNonces: Record<string, string>;
        requestPayloads: Record<string, unknown>;
      };
      artifacts: { faults: Record<string, unknown> };
    };
    expect(produceArtifacts).toHaveBeenCalledOnce();
    expect(assembler).toHaveBeenCalledOnce();
    expect(Object.keys(config.artifacts.faults)).toEqual(ACCEPTANCE_FAULT_PHASES);
    expect(Object.keys(config.acceptance.requestNonces)).toEqual(
      ACCEPTANCE_REQUEST_SCHEDULE.map(({ invocationId }) => invocationId),
    );
    expect(Object.keys(config.acceptance.requestPayloads)).toEqual(
      ACCEPTANCE_REQUEST_SCHEDULE.map(({ invocationId }) => invocationId),
    );
  });

  it('runs every checked-in native build and isolated validation stage in order', async () => {
    const stages: string[] = [];
    const sealed = { candidate: { installerSha256: hash } };
    const result = await buildWindowsInstalledAcceptanceArtifacts(
      {},
      {},
      {
        runStage: (stage: string) => {
          stages.push(stage);
          return Promise.resolve(stage === 'seal-artifact-set' ? sealed : { result: 'passed' });
        },
      },
    );
    expect(stages).toEqual([
      'native-signer',
      'prepare-signing-identities',
      'source-bound-synthetic-sender',
      'acceptance-candidate',
      'sign-candidate-manifest',
      'pack-candidate',
      'nonpromotable-repair',
      ...ACCEPTANCE_FAULT_PHASES.map((phase) => `fault-${phase}`),
      ...ACCEPTANCE_FAULT_PHASES.map((phase) => `validate-${phase}`),
      'seal-artifact-set',
    ]);
    expect(result).toBe(sealed);
  });

  it('does not inherit signing material into build subprocess environments', () => {
    expect(
      installedAcceptanceBuildEnvironment({
        PATH: 'tools',
        TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64: 'secret',
        TALKING_QUILL_ACCEPTANCE_MANIFEST_PRIVATE_KEY: 'secret',
        TALKING_QUILL_ACCEPTANCE_REQUEST_PRIVATE_KEY: 'secret',
      }),
    ).toEqual({ PATH: 'tools' });
  });
});
