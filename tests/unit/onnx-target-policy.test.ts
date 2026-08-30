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
  files: string[];
  asarUnpack: string[];
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

function targetContext(platform: string, arch: number): BeforePackContext {
  return {
    electronPlatformName: platform,
    arch,
    packager: {
      config: {
        files: ['package.json', onnxPolicy.ONNX_NATIVE_PATTERN],
        asarUnpack: ['native.node', onnxPolicy.ONNX_NATIVE_PATTERN],
      },
    },
  };
}

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
        {
          platform: 'win32',
          architecture: 'x64',
        },
      ),
    ).toThrow('files is missing its ONNX native selector');
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
