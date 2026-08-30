import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const require = createRequire(import.meta.url);
interface Target {
  platform: 'win32' | 'darwin';
  architecture: 'x64' | 'arm64';
}

interface OnnxConfig {
  files: unknown;
  asarUnpack: unknown;
}

interface BeforePackContext {
  electronPlatformName: string;
  arch: number;
  packager: { config: OnnxConfig };
}

const onnxPolicy = require('../../app/onnx-target-policy.cjs') as {
  ONNX_NATIVE_PATTERN: string;
  applyTargetNativeOnnxPolicy(config: Record<string, unknown>, target: Target): string;
  electronBuilderTarget(context: { electronPlatformName: string; arch: number }): Target;
  targetNativeOnnxPattern(target: Target): string;
};
const beforePack = require('../../app/before-pack.cjs') as (
  context: BeforePackContext,
) => Promise<void>;

const tuples = [
  ['win32', 'x64', 1],
  ['win32', 'arm64', 3],
  ['darwin', 'x64', 1],
  ['darwin', 'arm64', 3],
] as const;

function targetContext(platform: string, arch: number, config?: OnnxConfig): BeforePackContext {
  return {
    electronPlatformName: platform,
    arch,
    packager: {
      config: config ?? {
        files: ['package.json', onnxPolicy.ONNX_NATIVE_PATTERN],
        asarUnpack: ['native.node', onnxPolicy.ONNX_NATIVE_PATTERN],
      },
    },
  };
}

function attempt12Config(): OnnxConfig {
  // JSON.parse has an `any` return type; this checked-in fixture is consumed as builder input.
  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  return JSON.parse(
    readFileSync('tests/fixtures/electron-builder/attempt-12-before-pack-config.json', 'utf8'),
  );
}

const winX64Target = { platform: 'win32', architecture: 'x64' } as const;
const winX64Pattern = 'node_modules/onnxruntime-node/bin/napi-v3/win32/x64/**/*';

describe('electron-builder target-native ONNX policy', () => {
  it.each(tuples)(
    'selects only %s/%s through the beforePack hook',
    async (platform, architecture, arch) => {
      const context = targetContext(platform, arch);
      const selected = onnxPolicy.targetNativeOnnxPattern({ platform, architecture });

      await beforePack(context);

      expect(context.packager.config.files).toEqual(['package.json', selected]);
      expect(context.packager.config.asarUnpack).toEqual(['native.node', selected]);
    },
  );

  it('handles the normalized FileSet shape emitted in release attempt 12', async () => {
    const context = targetContext('win32', 1, attempt12Config());

    await beforePack(context);

    const files = context.packager.config.files as { filter: string[] }[];
    expect(files).toHaveLength(1);
    expect(files[0]?.filter).toContain('package.json');
    expect(files[0]?.filter).toContain(winX64Pattern);
    expect(JSON.stringify(files)).not.toContain(onnxPolicy.ONNX_NATIVE_PATTERN);
    expect(context.packager.config.asarUnpack).toEqual([
      'node_modules/better-sqlite3/build/Release/better_sqlite3.node',
      winX64Pattern,
    ]);
  });

  it.each([
    ['string', onnxPolicy.ONNX_NATIVE_PATTERN, winX64Pattern],
    [
      'object with string filter',
      { filter: onnxPolicy.ONNX_NATIVE_PATTERN },
      { filter: winX64Pattern },
    ],
    [
      'FileSet array',
      [{ filter: ['package.json', onnxPolicy.ONNX_NATIVE_PATTERN] }],
      [{ filter: ['package.json', winX64Pattern] }],
    ],
  ])('supports electron-builder files in %s form', (_name, files, expected) => {
    const config = { files: structuredClone(files), asarUnpack: onnxPolicy.ONNX_NATIVE_PATTERN };

    onnxPolicy.applyTargetNativeOnnxPolicy(config, winX64Target);

    expect(config.files).toEqual(expected);
    expect(config.asarUnpack).toBe(winX64Pattern);
  });

  it('uses the builder target tuple during a cross-build instead of the host tuple', () => {
    const crossTarget = process.platform === 'win32' ? ['darwin', 3] : ['win32', 1];
    const target = onnxPolicy.electronBuilderTarget({
      electronPlatformName: crossTarget[0] as string,
      arch: crossTarget[1] as number,
    });

    expect(target).toEqual(
      process.platform === 'win32'
        ? { platform: 'darwin', architecture: 'arm64' }
        : { platform: 'win32', architecture: 'x64' },
    );
  });

  it('rejects unsupported tuples and missing native selectors', () => {
    expect(() =>
      onnxPolicy.electronBuilderTarget({ electronPlatformName: 'linux', arch: 1 }),
    ).toThrow('Unsupported ONNX package target');
    expect(() =>
      onnxPolicy.applyTargetNativeOnnxPolicy(
        { files: ['package.json'], asarUnpack: [onnxPolicy.ONNX_NATIVE_PATTERN] },
        winX64Target,
      ),
    ).toThrow('files is missing its ONNX native selector');
  });

  it.each([
    [
      'multiple selectors',
      [onnxPolicy.ONNX_NATIVE_PATTERN, onnxPolicy.ONNX_NATIVE_PATTERN],
      'exactly one ONNX native selector',
    ],
    ['an exclusion', [`!${onnxPolicy.ONNX_NATIVE_PATTERN}`], 'unsupported ONNX native selector'],
    ['an already narrowed selector', [winX64Pattern], 'unsupported ONNX native selector'],
    [
      'an ONNX FileSet source',
      [{ from: 'node_modules/onnxruntime-node/bin/napi-v3', filter: '**/*' }],
      'unsupported ONNX file set',
    ],
    [
      'an ONNX selector under a remapped FileSet',
      [{ from: 'vendor', filter: onnxPolicy.ONNX_NATIVE_PATTERN }],
      'ambiguous ONNX file set',
    ],
  ])('fails closed when files contains %s', (_name, files, message) => {
    expect(() =>
      onnxPolicy.applyTargetNativeOnnxPolicy(
        { files, asarUnpack: onnxPolicy.ONNX_NATIVE_PATTERN },
        winX64Target,
      ),
    ).toThrow(message);
  });

  it('rejects multiple asarUnpack selectors without partly rewriting files', () => {
    const config = {
      files: [onnxPolicy.ONNX_NATIVE_PATTERN],
      asarUnpack: [onnxPolicy.ONNX_NATIVE_PATTERN, onnxPolicy.ONNX_NATIVE_PATTERN],
    };

    expect(() => onnxPolicy.applyTargetNativeOnnxPolicy(config, winX64Target)).toThrow(
      'asarUnpack must contain exactly one ONNX native selector',
    );
    expect(config.files).toEqual([onnxPolicy.ONNX_NATIVE_PATTERN]);
  });

  it('rejects object forms for asarUnpack', () => {
    expect(() =>
      onnxPolicy.applyTargetNativeOnnxPolicy(
        {
          files: onnxPolicy.ONNX_NATIVE_PATTERN,
          asarUnpack: { filter: onnxPolicy.ONNX_NATIVE_PATTERN },
        },
        winX64Target,
      ),
    ).toThrow('asarUnpack must be a string or an array of strings');
  });

  it('keeps the beforePack hook wired in electron-builder configuration', () => {
    const builder = readFileSync('build/electron-builder.yml', 'utf8');
    const configuredPath = /^beforePack: (.+)$/mu.exec(builder)?.[1];

    expect(configuredPath).toBe('before-pack.cjs');
    expect(existsSync(resolve('app', configuredPath ?? 'missing'))).toBe(true);
  });

  it('keeps afterPack as a read-only ONNX structural gate', () => {
    const source = readFileSync('app/after-pack.cjs', 'utf8');
    expect(source).toContain('await verifyPackagedStructure(context)');
    expect(source).not.toContain('pruneOnnxRuntime');
    expect(source).not.toContain('rmSync');
  });
});
