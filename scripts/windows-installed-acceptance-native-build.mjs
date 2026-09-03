import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { ACCEPTANCE_FAULT_PHASES } from './windows-installed-acceptance-schedule.mjs';

const root = resolve(import.meta.dirname, '..');

export async function buildWindowsInstalledAcceptanceArtifacts(
  options,
  context,
  dependencies = {},
) {
  if (process.platform !== 'win32' && dependencies.runStage === undefined) {
    throw new Error('Native installed-acceptance artifacts require Windows');
  }
  const runStage = dependencies.runStage ?? runNativeStage;
  const outputs = {};
  const stages = [
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
  ];
  for (const stage of stages) {
    const result = await runStage(stage, options, context, Object.freeze({ ...outputs }));
    if (result !== undefined) outputs[stage] = result;
  }
  const artifactSet = outputs['seal-artifact-set'];
  if (artifactSet === null || typeof artifactSet !== 'object') {
    throw new Error('Native acceptance build did not seal an artifact set');
  }
  return Object.freeze(artifactSet);
}

function runNativeStage(stage, options, context, outputs) {
  const environment = sanitizedSubprocessEnvironment(process.env, {
    TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD: '1',
    TALKING_QUILL_ACCEPTANCE_BUILD: '1',
    TALKING_QUILL_ACCEPTANCE_BUILD_ID: context.buildId,
    TALKING_QUILL_ACCEPTANCE_UNSIGNED_MANIFEST_PAYLOAD_PATH: resolve(
      context.outputRoot,
      'unsigned-acceptance-manifest.json',
    ),
    ...(outputs['prepare-signing-identities'] === undefined
      ? {}
      : {
          TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL:
            outputs['prepare-signing-identities'].manifestPublicKeySpkiBase64url,
          TALKING_QUILL_ACCEPTANCE_REQUEST_PUBLIC_KEY_SPKI_BASE64URL:
            outputs['prepare-signing-identities'].requestPublicKeySpkiBase64url,
          TALKING_QUILL_ACCEPTANCE_VALIDATION_PUBLIC_KEY_SPKI_BASE64URL:
            outputs['prepare-signing-identities'].validationPublicKeySpkiBase64url,
          TALKING_QUILL_ACCEPTANCE_FAULT_VALIDATOR_SHA256: createHash('sha256')
            .update(
              readFileSync(
                resolve(root, 'scripts/run-windows-installed-acceptance-fault-validation.mjs'),
              ),
            )
            .digest('hex'),
        }),
    TALKING_QUILL_ACCEPTANCE_VALID_FROM_MS: String(context.runWindow.notBeforeMs - 60_000),
    TALKING_QUILL_ACCEPTANCE_VALID_UNTIL_MS: String(context.runWindow.expiresAtMs),
    TALKING_QUILL_RELEASE_COMMIT: context.canonical.descriptor.sourceCommit,
    TALKING_QUILL_RELEASE_TREE: context.canonical.descriptor.sourceTree,
    TALKING_QUILL_SOURCE_COMMIT: context.canonical.descriptor.sourceCommit,
    TALKING_QUILL_SOURCE_TREE: context.canonical.descriptor.sourceTree,
    TALKING_QUILL_CANONICAL_ARTIFACT_JSON: JSON.stringify({
      architecture: 'x64',
      installerPath: context.canonical.installerPath,
      installerSha256: context.canonical.descriptor.sha256,
      unpackedRoot: context.canonicalRoot,
      metadataPath: resolve(context.canonicalRoot, 'resources/keyboard-owner-release-v1.json'),
      metadataSha256: context.canonicalMetadataSha256,
      electronRelativePath: 'Talking Quill.exe',
    }),
    ...(stage === 'acceptance-candidate' || stage === 'pack-candidate'
      ? context.predecessorEnvironment
      : {}),
    ...(stage === 'nonpromotable-repair' || stage.startsWith('fault-')
      ? {
          TALKING_QUILL_PACKAGE_MODE: 'repair',
          TALKING_QUILL_NATIVE_FAULT_PHASE: stage.startsWith('fault-')
            ? stage.slice('fault-'.length)
            : undefined,
        }
      : {}),
  });
  const commands = nativeStageCommands(stage, options, context);
  const secretPaths = {
    manifestPrivateKeyPath: options.manifestPrivateKeyPath,
    requestPrivateKeyPath: options.requestPrivateKeyPath,
    updatePrivateKeyPath: options.updatePrivateKeyPath,
    validationPrivateKeyPath: options.validationPrivateKeyPath,
  };
  const permittedSecrets =
    stage === 'prepare-signing-identities'
      ? secretPaths
      : stage === 'sign-candidate-manifest'
        ? { manifestPrivateKeyPath: secretPaths.manifestPrivateKeyPath }
        : stage.startsWith('validate-')
          ? { validationPrivateKeyPath: secretPaths.validationPrivateKeyPath }
          : stage === 'seal-artifact-set'
            ? { updatePrivateKeyPath: secretPaths.updatePrivateKeyPath }
            : {};
  let stdout = '';
  for (const [command, arguments_] of commands) {
    const result = spawnSync(command, arguments_, {
      cwd: root,
      env: environment,
      encoding: 'utf8',
      windowsHide: true,
      timeout: 30 * 60 * 1_000,
      input: Buffer.from(`${JSON.stringify(permittedSecrets)}\n`),
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    if (result.error !== undefined || result.signal !== null || result.status !== 0) {
      throw new Error(`Native installed-acceptance stage failed: ${stage}`);
    }
    stdout = result.stdout.trim();
  }
  if (stage === 'seal-artifact-set' || stage === 'prepare-signing-identities') {
    if (stdout === '') throw new Error(`${stage} returned no inventory`);
    return JSON.parse(stdout);
  }
  return Object.freeze({ result: 'passed', outputSha256: hashText(stdout) });
}

function hashText(value) {
  return createHash('sha256').update(value).digest('hex');
}

function nativeStageCommands(stage, options, context) {
  const node = process.execPath;
  const pnpm = process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm';
  if (stage === 'prepare-signing-identities') {
    return [
      [
        node,
        [
          'scripts/seal-windows-installed-acceptance-artifacts.mjs',
          'identities',
          '--output',
          context.outputRoot,
        ],
      ],
    ];
  }
  if (stage === 'native-signer') {
    const common = [
      'build',
      '--manifest-path',
      'helper/Cargo.toml',
      '--locked',
      '--release',
      '--target',
      'x86_64-pc-windows-msvc',
    ];
    return [
      ['cargo.exe', [...common, '-p', 'talking-quill-acceptance-signer']],
      [
        'cargo.exe',
        [...common, '-p', 'talking-quill-helper', '--features', 'windows-installed-acceptance'],
      ],
    ];
  }
  if (stage === 'acceptance-candidate') {
    return [
      [node, ['scripts/prepackage-check.mjs']],
      [pnpm, ['build']],
      [node, ['scripts/build-helper.mjs', '--platform', 'win32', '--arch', 'x64']],
      [node, ['scripts/rebuild-electron-native.mjs', 'x64']],
      [node, ['scripts/build-windows-setup.mjs', 'x64']],
      [
        pnpm,
        [
          '--dir',
          'app',
          'exec',
          'electron-builder',
          '--config',
          '../build/electron-builder.installed-acceptance.yml',
          '--config.npmRebuild=false',
          '--win',
          'dir',
          '--x64',
          '--publish',
          'never',
        ],
      ],
    ];
  }
  if (stage === 'sign-candidate-manifest') {
    return [
      [
        node,
        [
          'scripts/seal-windows-installed-acceptance-artifacts.mjs',
          'manifest',
          '--output',
          context.outputRoot,
        ],
      ],
    ];
  }
  if (stage === 'pack-candidate') {
    return [[node, ['scripts/pack-windows-native.mjs', 'x64', 'tmp/installed-acceptance-build']]];
  }
  if (stage === 'nonpromotable-repair') {
    return [
      [node, ['scripts/build-windows-acceptance-repair-setup.mjs', 'x64']],
      [node, ['scripts/pack-windows-native.mjs', 'x64', 'tmp/installed-acceptance-build']],
    ];
  }
  if (stage.startsWith('fault-')) {
    return [
      [node, ['scripts/build-windows-acceptance-fault-setup.mjs', 'x64']],
      [node, ['scripts/pack-windows-native.mjs', 'x64', 'tmp/installed-acceptance-build']],
    ];
  }
  if (stage.startsWith('validate-')) {
    const phase = stage.slice('validate-'.length);
    return [
      [
        node,
        [
          'scripts/run-windows-installed-acceptance-fault-validation.mjs',
          '--phase',
          phase,
          '--output',
          context.outputRoot,
        ],
      ],
    ];
  }
  if (stage === 'source-bound-synthetic-sender') {
    return [
      [
        'cargo.exe',
        [
          'build',
          '--manifest-path',
          'helper/Cargo.toml',
          '--locked',
          '--release',
          '--target',
          'x86_64-pc-windows-msvc',
          '-p',
          'talking-quill-acceptance-signer',
          '--bin',
          'talking-quill-acceptance-synthetic-sender',
        ],
      ],
    ];
  }
  if (stage === 'seal-artifact-set') {
    return [
      [
        node,
        ['scripts/seal-windows-installed-acceptance-artifacts.mjs', '--output', context.outputRoot],
      ],
    ];
  }
  throw new Error(`Unknown native installed-acceptance stage: ${stage}`);
}
