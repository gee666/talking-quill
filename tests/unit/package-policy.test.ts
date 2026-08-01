import { spawnSync } from 'node:child_process';
import { access, readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { validateNsisUninstallPolicy } from '../../scripts/nsis-uninstall-policy.mjs';
import {
  discoverFinalArtifactNames,
  finalArtifactNamesForIdentity,
  ONNX_RUNTIME_PATHS,
  PROVIDER_LOGO_BASENAMES,
  validateAsarEntries,
  validateExpectedFinalArtifacts,
  validateFinalArtifactInspection,
  validatePhysicalEntries,
  validatePhysicalPackageEntries,
  validateResourceEntries,
  validateRuntimeContent,
  validateSharedReleaseArtifacts,
} from '../../scripts/package-policy.mjs';

const jpegProviderLogos = new Set(['fireworksai', 'localai', 'mistral', 'openrouter']);

const mergedMasterRendererAssets = [
  'out/renderer/assets/echo-session-valid.js',
  'out/renderer/assets/echo-session-valid.css',
  'out/renderer/assets/logo-light-valid.png',
  'out/renderer/assets/logo-dark-valid.png',
] as const;

const performanceLazyChunks = [
  'out/renderer/assets/Dialog-valid.js',
  'out/renderer/assets/HistoryScreen-valid.js',
  'out/renderer/assets/InfoScreen-valid.js',
  'out/renderer/assets/SettingsScreen-valid.js',
  'out/renderer/assets/SmartProcessingSection-valid.js',
  'out/renderer/assets/UpdateDialog-valid.js',
  'out/renderer/assets/schemas-valid.js',
] as const;

const mergedRendererAssets = [...mergedMasterRendererAssets, ...performanceLazyChunks];

const validAsar = [
  'out',
  'out/main',
  'out/main/index.js',
  'out/main/chunks',
  'out/workers',
  'out/workers/whisper-bootstrap.cjs',
  'out/workers/whisper-payload.cjs',
  'out/preload/main.js',
  'out/preload/widget.js',
  'out/preload/capture.js',
  'out/renderer/main/index.html',
  'out/renderer/widget/index.html',
  'out/renderer/capture/index.html',
  'out/renderer/assets/main-valid.js',
  'out/renderer/assets/main-valid.css',
  'out/renderer/assets/widget-valid.js',
  'out/renderer/assets/widget-valid.css',
  'out/renderer/assets/capture-valid.js',
  'out/renderer/assets/capture.worklet-valid.js',
  'out/renderer/assets/audio-valid.js',
  ...mergedRendererAssets,
  ...PROVIDER_LOGO_BASENAMES.map(
    (name) => `out/renderer/assets/${name}-valid.${jpegProviderLogos.has(name) ? 'jpeg' : 'png'}`,
  ),
  'package.json',
  'node_modules',
  'node_modules/better-sqlite3',
  'node_modules/better-sqlite3/package.json',
  'node_modules/better-sqlite3/lib',
  'node_modules/better-sqlite3/lib/index.js',
  'node_modules/better-sqlite3/build',
  'node_modules/better-sqlite3/build/Release',
  'node_modules/better-sqlite3/build/Release/better_sqlite3.node',
  'node_modules/bindings',
  'node_modules/bindings/package.json',
  'node_modules/bindings/bindings.js',
  'node_modules/file-uri-to-path',
  'node_modules/file-uri-to-path/package.json',
  'node_modules/file-uri-to-path/index.js',
  'node_modules/onnxruntime-node',
  'node_modules/onnxruntime-node/package.json',
  'node_modules/onnxruntime-node/dist',
  'node_modules/onnxruntime-node/dist/index.js',
  'node_modules/onnxruntime-node/bin',
  'node_modules/onnxruntime-node/bin/napi-v3',
  'node_modules/onnxruntime-node/bin/napi-v3/win32',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/x64',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/x64/onnxruntime_binding.node',
  'node_modules/onnxruntime-common',
  'node_modules/onnxruntime-common/package.json',
  'node_modules/onnxruntime-common/dist',
  'node_modules/onnxruntime-common/dist/cjs',
  'node_modules/onnxruntime-common/dist/cjs/package.json',
  'node_modules/onnxruntime-common/dist/cjs/index.js',
  ...ONNX_RUNTIME_PATHS,
];

const asarForTarget = (platform: 'win' | 'mac', architecture: 'x64' | 'arm64') => {
  const binaryRoot = 'node_modules/onnxruntime-node/bin/napi-v3/';
  const platformRoot = `${binaryRoot}${platform === 'mac' ? 'darwin' : 'win32'}`;
  const architectureRoot = `${platformRoot}/${architecture}`;
  return validAsar.filter(
    (entry) =>
      !entry.startsWith(binaryRoot) ||
      entry === platformRoot ||
      entry === architectureRoot ||
      entry.startsWith(`${architectureRoot}/`),
  );
};

const commonResources = [
  'app-update.yml',
  'app.asar',
  'LICENSE',
  'THIRD_PARTY_NOTICES.txt',
  'app.asar.unpacked',
  'app.asar.unpacked/node_modules',
  'app.asar.unpacked/node_modules/better-sqlite3',
  'app.asar.unpacked/node_modules/better-sqlite3/build',
  'app.asar.unpacked/node_modules/better-sqlite3/build/Release',
  'app.asar.unpacked/node_modules/better-sqlite3/build/Release/better_sqlite3.node',
  'app.asar.unpacked/node_modules/onnxruntime-node',
  'app.asar.unpacked/node_modules/onnxruntime-node/bin',
  'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3',
  'helper',
];

const validResources = (target: 'win' | 'mac') => {
  const platform = target === 'mac' ? 'darwin' : 'win32';
  const architectureRoot = `app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3/${platform}/x64`;
  return [
    ...commonResources,
    `app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3/${platform}`,
    architectureRoot,
    ...(target === 'mac'
      ? [`${architectureRoot}/libonnxruntime.1.21.0.dylib`]
      : [`${architectureRoot}/DirectML.dll`, `${architectureRoot}/onnxruntime.dll`]),
    `${architectureRoot}/onnxruntime_binding.node`,
    target === 'win' ? 'helper/talking-quill-helper.exe' : 'helper/talking-quill-helper',
  ];
};

describe('packaged runtime allowlist', () => {
  it('accepts only the expected runtime ASAR and native resource paths', () => {
    expect(validAsar.filter((entry) => entry.startsWith('out/renderer/assets/'))).toHaveLength(56);
    expect(() => validateAsarEntries(validAsar)).not.toThrow();
    expect(() => validateResourceEntries(validResources('win'), 'win')).not.toThrow();
    expect(() => validateResourceEntries(validResources('mac'), 'mac')).not.toThrow();
  });

  it.each([
    ['win', 'x64'],
    ['win', 'arm64'],
    ['mac', 'x64'],
    ['mac', 'arm64'],
  ] as const)('requires only the selected %s/%s ONNX native payload', (platform, architecture) => {
    const entries = asarForTarget(platform, architecture);
    expect(() => validateAsarEntries(entries, { platform, architecture })).not.toThrow();
    const nativeLibrary =
      platform === 'mac'
        ? `node_modules/onnxruntime-node/bin/napi-v3/darwin/${architecture}/libonnxruntime.1.21.0.dylib`
        : `node_modules/onnxruntime-node/bin/napi-v3/win32/${architecture}/onnxruntime.dll`;
    expect(() =>
      validateAsarEntries(
        entries.filter((entry) => entry !== nativeLibrary),
        { platform, architecture },
      ),
    ).toThrow(`Required ONNX runtime path is missing: ${nativeLibrary}`);
  });

  it.each(mergedRendererAssets)('retains the merged renderer allowlist for %s', (asset) => {
    expect(validAsar).toContain(asset);
    expect(() => validateAsarEntries(validAsar)).not.toThrow();
  });

  it('requires guarded worker artifacts, renderer assets, and all provider logos', async () => {
    expect(() =>
      validateAsarEntries(validAsar.filter((entry) => entry !== 'out/workers/whisper-payload.cjs')),
    ).toThrow('Required runtime file is missing');
    for (const requiredOnnxPath of [
      'node_modules/onnxruntime-node/dist/binding.js',
      'node_modules/onnxruntime-common/dist/cjs/tensor.js',
      'node_modules/onnxruntime-node/bin/napi-v3/win32/x64/onnxruntime.dll',
    ]) {
      expect(() =>
        validateAsarEntries(validAsar.filter((entry) => entry !== requiredOnnxPath)),
      ).toThrow('Required ONNX runtime path is missing');
    }
    expect(PROVIDER_LOGO_BASENAMES).toHaveLength(38);
    await Promise.all(
      PROVIDER_LOGO_BASENAMES.map((name) =>
        access(
          resolve(
            'app/assets/provider-logos',
            `${name}${jpegProviderLogos.has(name) ? '.jpeg' : '.png'}`,
          ),
        ),
      ),
    );
    expect(() =>
      validateAsarEntries(
        validAsar.filter((entry) => entry !== 'out/renderer/assets/widget-valid.js'),
      ),
    ).toThrow('Required renderer asset is missing');
    for (const requiredAsset of [
      'out/renderer/assets/audio-valid.js',
      'out/renderer/assets/echo-session-valid.js',
      'out/renderer/assets/echo-session-valid.css',
      'out/renderer/assets/InfoScreen-valid.js',
      'out/renderer/assets/SettingsScreen-valid.js',
      'out/renderer/assets/SmartProcessingSection-valid.js',
      'out/renderer/assets/UpdateDialog-valid.js',
      'out/renderer/assets/schemas-valid.js',
      'out/renderer/assets/logo-light-valid.png',
      'out/renderer/assets/logo-dark-valid.png',
    ]) {
      expect(() =>
        validateAsarEntries(validAsar.filter((entry) => entry !== requiredAsset)),
      ).toThrow('Required renderer asset is missing');
    }
    expect(() =>
      validateAsarEntries(
        validAsar.filter((entry) => entry !== 'out/renderer/assets/openai-valid.png'),
      ),
    ).toThrow('Required provider logo is missing: openai');
  });

  it('requires all 38 provider logos to have matching magic bytes and decode in Electron', () => {
    const electron = createRequire(import.meta.url)('electron') as unknown;
    if (typeof electron !== 'string') throw new Error('Electron executable is unavailable');
    const result = spawnSync(
      electron,
      [resolve('tests/fixtures/validate-provider-logos.cjs'), resolve('app/assets/provider-logos')],
      { encoding: 'utf8', timeout: 30_000 },
    );
    expect(result.status, `${result.stdout}\n${result.stderr}`).toBe(0);
    expect(JSON.parse(result.stdout) as { count: number; failures: unknown[] }).toEqual({
      count: 38,
      failures: [],
    });
  });

  it('rejects unknown dependencies and source or test build artifacts', () => {
    expect(() => validateAsarEntries([...validAsar, 'node_modules/unexpected/index.js'])).toThrow(
      'Unexpected ASAR',
    );
    expect(() => validateAsarEntries([...validAsar, 'node_modules/aws4/aws4.js'])).toThrow(
      'Unexpected ASAR',
    );
    for (const unexpectedOnnxFile of [
      'node_modules/onnxruntime-node/dist/rogue.js',
      'node_modules/onnxruntime-common/dist/cjs/rogue.js',
      'node_modules/onnxruntime-node/bin/napi-v3/win32/x64/rogue.dll',
    ]) {
      expect(() => validateAsarEntries([...validAsar, unexpectedOnnxFile])).toThrow(
        'Unexpected ASAR',
      );
    }
    for (const unexpectedAsset of [
      'out/renderer/assets/rogue.js',
      'out/renderer/assets/status-presentation-valid.js',
      'out/renderer/assets/theme-valid.css',
      'out/renderer/assets/audio-valid.css',
      'out/renderer/assets/InfoScreen-valid.css',
      'out/renderer/assets/app-icon-valid.png',
      'out/renderer/assets/anthropic-valid.jpeg',
    ]) {
      expect(() => validateAsarEntries([...validAsar, unexpectedAsset])).toThrow('Unexpected ASAR');
    }
    expect(() => validateAsarEntries([...validAsar, 'out/renderer/assets/main-stale.js'])).toThrow(
      'Required renderer asset count is not one',
    );
    expect(() =>
      validateAsarEntries([...validAsar, 'out/renderer/assets/anthropic-stale.png']),
    ).toThrow('Required provider logo count is not one');
    expect(() =>
      validateAsarEntries([...validAsar, 'node_modules/better-sqlite3/lib/test_extension.node']),
    ).toThrow('Forbidden packaged files');
    expect(() =>
      validateAsarEntries([...validAsar, 'node_modules/better-sqlite3/lib/addon.cpp']),
    ).toThrow('Forbidden packaged files');
  });

  it('keeps the exact aws4 oracle dev-only at the workspace root and out of artifacts', async () => {
    const [rootSource, appSource, lock] = await Promise.all([
      readFile(resolve('package.json'), 'utf8'),
      readFile(resolve('app/package.json'), 'utf8'),
      readFile(resolve('pnpm-lock.yaml'), 'utf8'),
    ]);
    const root = JSON.parse(rootSource) as {
      dependencies?: Record<string, string>;
      devDependencies?: Record<string, string>;
    };
    const app = JSON.parse(appSource) as { dependencies?: Record<string, string> };

    expect(root.devDependencies?.aws4).toBe('1.13.2');
    expect(root.dependencies?.aws4).toBeUndefined();
    expect(app.dependencies?.aws4).toBeUndefined();
    const appImporter = lock.split('\n  app:\n')[1]?.split('\npackages:\n')[0] ?? '';
    expect(appImporter).not.toMatch(/^\s+aws4:/mu);
    expect(() => validateAsarEntries([...validAsar, 'node_modules/aws4/aws4.js'])).toThrow(
      'Unexpected ASAR',
    );
  });

  it('keeps uninstall data removal explicit and opt-in', async () => {
    const [builder, installer] = await Promise.all([
      readFile(resolve('build/electron-builder.yml'), 'utf8'),
      readFile(resolve('build/installer.nsh'), 'utf8'),
    ]);
    expect(builder).toContain('include: installer.nsh');
    expect(builder).toContain('deleteAppDataOnUninstall: false');
    expect(installer).toContain('!macro customUnWelcomePage');
    expect(installer).toContain('UninstPage custom');
    expect(installer).not.toContain('!macro customUninstallPage');
    expect(installer).toContain('${NSD_Uncheck} $DeleteTalkingQuillDataCheckbox');
    expect(installer).toContain('--talking-quill-reset-owned-data-and-exit="$1"');
    expect(installer).toContain('TALKING_QUILL_UNINSTALL_RESET_CHALLENGE');
    expect(installer).toContain('IfFileExists "$INSTDIR\\${APP_FILENAME}.exe"');
    expect(installer).toMatch(/ExecWait[^\n]+\$0/u);
    expect(installer).toContain('${If} $0 != 0');
    expect(installer).toContain('Abort');
    expect(installer).not.toContain('RMDir /r');
    expect(installer).not.toMatch(/Ollama/i);
    expect(await readFile(resolve('scripts/run-package.mjs'), 'utf8')).toContain(
      "TALKING_QUILL_PACKAGE_INSPECTION_STRICT: '1'",
    );
  });

  it('proves the extracted pinned NSIS page and reset-helper ordering before packaging', async () => {
    const require = createRequire(import.meta.url);
    const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
    const templateRoot = resolve(
      dirname(electronBuilderRequire.resolve('app-builder-lib/package.json')),
      'templates/nsis',
    );
    const [custom, assisted, uninstaller] = await Promise.all([
      readFile(resolve('build/installer.nsh'), 'utf8'),
      readFile(resolve(templateRoot, 'assistedInstaller.nsh'), 'utf8'),
      readFile(resolve(templateRoot, 'uninstaller.nsh'), 'utf8'),
    ]);
    expect(() => validateNsisUninstallPolicy({ custom, assisted, uninstaller })).not.toThrow();
    expect(() =>
      validateNsisUninstallPolicy({
        custom: custom.replace('!macro customUnWelcomePage', '!macro customUninstallPage'),
        assisted,
        uninstaller,
      }),
    ).toThrow(/after InstFiles|pre-InstFiles|Missing NSIS macro/u);
    expect(() =>
      validateNsisUninstallPolicy({
        custom,
        assisted,
        uninstaller: uninstaller.replace(
          '!insertmacro customUnInstall',
          '# custom reset hook removed',
        ),
      }),
    ).toThrow('before install deletion');
  });

  it('requires the physical notices and unpacked native runtime resources', () => {
    expect(() =>
      validateResourceEntries(
        validResources('win').filter((entry) => entry !== 'THIRD_PARTY_NOTICES.txt'),
        'win',
      ),
    ).toThrow('Required packaged resource is missing');
    expect(() => validateResourceEntries(['app.asar', 'LICENSE'], 'win')).toThrow(
      'Required packaged resource is missing',
    );
  });

  it('rejects strict inspection without an explicit artifact requirement', () => {
    const env = { ...process.env };
    delete env.TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED;
    delete env.TALKING_QUILL_PACKAGE_INSPECTION_STRICT;
    const result = spawnSync(
      process.execPath,
      ['scripts/inspect-package.mjs', '--strict', 'release/does-not-exist'],
      { encoding: 'utf8', env },
    );
    expect(result.status).not.toBe(0);
    expect(`${result.stdout}${result.stderr}`).toContain(
      'Strict final-artifact inspection requires --artifacts-required=none|nsis|dmg-zip',
    );
  });

  it('rejects strict inspection without explicit target platform and architecture', () => {
    const env: NodeJS.ProcessEnv = {
      ...process.env,
      TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: 'none',
    };
    delete env.TALKING_QUILL_PACKAGE_TARGET;
    delete env.TALKING_QUILL_PACKAGE_ARCH;
    const result = spawnSync(
      process.execPath,
      ['scripts/inspect-package.mjs', '--strict', 'release/does-not-exist'],
      { encoding: 'utf8', env },
    );
    expect(result.status).not.toBe(0);
    expect(`${result.stdout}${result.stderr}`).toContain(
      'requires TALKING_QUILL_PACKAGE_TARGET=win|mac and TALKING_QUILL_PACKAGE_ARCH=x64|arm64',
    );
  });

  it('discovers every final-artifact extension before validating its name', () => {
    expect(
      discoverFinalArtifactNames([
        'builder-debug.yml',
        'Talking-Quill-1.0.0-win-x64.exe',
        'Talking-Quill-1.0.0-mac-arm64.dmg',
        'Talking-Quill-1.0.0-mac-arm64.zip',
        'unexpected.exe',
      ]),
    ).toEqual([
      'Talking-Quill-1.0.0-win-x64.exe',
      'Talking-Quill-1.0.0-mac-arm64.dmg',
      'Talking-Quill-1.0.0-mac-arm64.zip',
      'unexpected.exe',
    ]);
  });

  it('requires exact declared artifact stems, types, and counts', () => {
    const winX64 = { version: '1.0.0', platform: 'win', arch: 'x64' } as const;
    const macArm64 = { version: '1.0.0', platform: 'mac', arch: 'arm64' } as const;
    expect(() => validateExpectedFinalArtifacts([], 'none', winX64)).not.toThrow();
    expect(() =>
      validateExpectedFinalArtifacts(['Talking-Quill-1.0.0-win-x64.exe'], 'nsis', winX64),
    ).not.toThrow();
    expect(() =>
      validateExpectedFinalArtifacts(
        ['Talking-Quill-1.0.0-mac-arm64.dmg', 'Talking-Quill-1.0.0-mac-arm64.zip'],
        'dmg-zip',
        macArm64,
      ),
    ).not.toThrow();
    expect(() => validateExpectedFinalArtifacts([], 'nsis', winX64)).toThrow('expected exe=1');
    expect(() =>
      validateExpectedFinalArtifacts(
        ['Talking-Quill-1.0.0-win-x64.exe', 'Talking-Quill-1.0.0-win-x64.exe'],
        'nsis',
        winX64,
      ),
    ).toThrow('found exe=2');
    expect(() =>
      validateExpectedFinalArtifacts(['Talking-Quill-1.0.0-mac-arm64.dmg'], 'dmg-zip', macArm64),
    ).toThrow('zip=1');
    for (const names of [
      ['Talking-Quill-1.0.0-mac-arm64.dmg', 'Talking-Quill-1.0.1-mac-arm64.zip'],
      ['Talking-Quill-1.0.0-mac-arm64.dmg', 'Talking-Quill-1.0.0-mac-x64.zip'],
      ['Talking-Quill-1.0.0-mac-x64.dmg', 'Talking-Quill-1.0.0-mac-x64.zip'],
    ]) {
      expect(() => validateExpectedFinalArtifacts(names, 'dmg-zip', macArm64)).toThrow(
        'Unexpected final artifact names',
      );
    }
    expect(() =>
      validateExpectedFinalArtifacts(['Talking-Quill-1.0.0-win-arm64.exe'], 'nsis', winX64),
    ).toThrow('Unexpected final artifact names');
    expect(() => validateExpectedFinalArtifacts(['unexpected.exe'], 'nsis', winX64)).toThrow(
      'Unexpected final artifact names: unexpected.exe',
    );
    expect(() => validateExpectedFinalArtifacts([], 'unknown', winX64)).toThrow(
      'Unknown final-artifact requirement mode',
    );
  });

  it('retains both mac architectures globally but scopes inspection and provenance by identity', () => {
    const names = [
      'Talking-Quill-1.0.1-mac-x64.dmg',
      'Talking-Quill-1.0.1-mac-x64.zip',
      'Talking-Quill-1.0.1-mac-arm64.dmg',
      'Talking-Quill-1.0.1-mac-arm64.zip',
    ];
    const identity = { version: '1.0.1', platform: 'mac', arch: 'arm64' } as const;
    expect(() => validateSharedReleaseArtifacts(names, 'dmg-zip', identity)).not.toThrow();
    expect(finalArtifactNamesForIdentity(names, identity)).toEqual([
      'Talking-Quill-1.0.1-mac-arm64.dmg',
      'Talking-Quill-1.0.1-mac-arm64.zip',
    ]);
  });

  it('requires recursive magic-based architecture probes for unpacked and extracted native images', async () => {
    const inspector = await readFile(resolve('scripts/inspect-package.mjs'), 'utf8');
    for (const native of [
      'better_sqlite3.node',
      'onnxruntime_binding.node',
      'onnxruntime.dll',
      'libonnxruntime.1.21.0.dylib',
    ]) {
      expect(inspector).toContain(native);
    }
    expect(inspector).toMatch(
      /inspectNativeTree\(packageRoot, \{\s*platform: boundPlatform,\s*architecture: boundArch,/u,
    );
    expect(inspector).toMatch(
      /inspectNativeTree\(root, \{\s*platform: mac \? 'mac' : 'win',\s*architecture: expectedArch,/u,
    );
    expect(inspector).toContain('writeArtifactProvenanceManifest');
  });

  it('makes any uninspected expected final artifact fatal only in strict package gates', () => {
    expect(() => validateFinalArtifactInspection(1, 0, false)).not.toThrow();
    expect(() => validateFinalArtifactInspection(1, 0, true)).toThrow(
      'requires every produced artifact',
    );
    expect(() => validateFinalArtifactInspection(2, 2, true)).not.toThrow();
    expect(() => validateFinalArtifactInspection(1, 2, false)).toThrow('counts are invalid');
  });

  it('enforces closed platform-specific physical package allowlists', () => {
    expect(() =>
      validatePhysicalPackageEntries(
        ['Talking Quill.exe', 'locales', 'locales/en-US.pak', 'resources', 'resources/app.asar'],
        'win',
      ),
    ).not.toThrow();
    expect(() =>
      validatePhysicalPackageEntries(['Talking Quill.exe', 'telemetry.dll'], 'win'),
    ).toThrow('Unexpected physical package entries');
    expect(() =>
      validatePhysicalPackageEntries(
        [
          'Applications',
          '.background',
          '.background/background.tiff',
          'Talking Quill.app',
          'Talking Quill.app/Contents',
          'Talking Quill.app/Contents/MacOS',
          'Talking Quill.app/Contents/MacOS/Talking Quill',
          'Talking Quill.app/Contents/Resources',
          'Talking Quill.app/Contents/Resources/app.asar',
          'Talking Quill.app/Contents/Frameworks',
          'Talking Quill.app/Contents/Frameworks/Electron Framework.framework',
        ],
        'mac',
      ),
    ).not.toThrow();
    expect(() =>
      validatePhysicalPackageEntries(
        ['Talking Quill.app', 'Talking Quill.app/Contents/Frameworks/Unknown.framework/payload'],
        'mac',
      ),
    ).toThrow('Unexpected physical package entries');
  });

  it('rejects forbidden physical paths and runtime content with attribution as the only name exception', () => {
    expect(() =>
      validatePhysicalEntries(['Talking Quill.exe', 'resources/app.asar']),
    ).not.toThrow();
    expect(() => validatePhysicalEntries(['resources/reference/legacy.js'])).toThrow(
      'Forbidden packaged files',
    );
    expect(() => validatePhysicalEntries(['AnythingLLM-runtime.dll'])).toThrow(
      'Forbidden packaged files',
    );
    expect(() => validateRuntimeContent('out/main/index.js', 'AnythingLLM runtime')).toThrow(
      'AnythingLLM runtime content',
    );
    expect(() =>
      validateRuntimeContent('THIRD_PARTY_NOTICES.txt', 'AnythingLLM MIT attribution'),
    ).not.toThrow();
    expect(() =>
      validateRuntimeContent(
        'THIRD_PARTY_NOTICES.txt',
        ['-----BEGIN ', 'PRIVATE KEY-----\nnot-attribution'].join(''),
      ),
    ).toThrow('Secret-like content');
    expect(() =>
      validateRuntimeContent('out/main/index.js', `token ghp_${'a'.repeat(24)}`),
    ).toThrow('Secret-like content');
  });

  it('recursively rejects forbidden or unexpected resource paths', () => {
    expect(() =>
      validateResourceEntries(
        [...validResources('win'), 'app.asar.unpacked/reference/secret.js'],
        'win',
      ),
    ).toThrow('Forbidden packaged files');
    expect(() =>
      validateResourceEntries(
        [
          ...validResources('win'),
          'app.asar.unpacked/node_modules/better-sqlite3/build/Release/other.node',
        ],
        'win',
      ),
    ).toThrow('Unexpected packaged resources');
    expect(() =>
      validateResourceEntries([...validResources('win'), 'electron.icns'], 'win'),
    ).toThrow('Unexpected packaged resources');
    expect(() => validateResourceEntries([...validResources('mac'), 'elevate.exe'], 'mac')).toThrow(
      'Unexpected packaged resources',
    );
    expect(() =>
      validateResourceEntries(
        [
          ...validResources('mac'),
          'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3/win32/x64/onnxruntime.dll',
        ],
        'mac',
      ),
    ).toThrow('Unexpected packaged resources');
    expect(() =>
      validateResourceEntries(
        validResources('win').filter((entry) => !entry.endsWith('/DirectML.dll')),
        'win',
      ),
    ).toThrow('Required ONNX resource path is missing');
    for (const rogue of [
      'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3/win32/x64/rogue.dll',
      'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3/win32/arm64/rogue.dat',
    ]) {
      expect(() => validateResourceEntries([...validResources('win'), rogue], 'win')).toThrow(
        'Unexpected packaged resources',
      );
    }
  });
});
