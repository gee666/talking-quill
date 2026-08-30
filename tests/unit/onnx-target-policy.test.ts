import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const require = createRequire(import.meta.url);
interface Target {
  platform: 'win32' | 'darwin';
  architecture: 'x64' | 'arm64';
}

const onnxPolicy = require('../../app/onnx-target-policy.cjs') as {
  ONNX_NATIVE_PATTERN: string;
  applyTargetNativeOnnxPolicy(config: Record<string, unknown>, target: Target): string;
  electronBuilderTarget(context: { electronPlatformName: string; arch: number }): Target;
  targetNativeOnnxPattern(target: Target): string;
};

const tuples = [
  ['win32', 'x64', 1],
  ['win32', 'arm64', 3],
  ['darwin', 'x64', 1],
  ['darwin', 'arm64', 3],
] as const;

describe('electron-builder target-native ONNX policy', () => {
  it.each(tuples)('selects only %s/%s before ASAR creation', (platform, architecture, arch) => {
    const target = onnxPolicy.electronBuilderTarget({ electronPlatformName: platform, arch });
    const config = {
      files: ['package.json', onnxPolicy.ONNX_NATIVE_PATTERN],
      asarUnpack: ['native.node', onnxPolicy.ONNX_NATIVE_PATTERN],
    };

    const selected = onnxPolicy.applyTargetNativeOnnxPolicy(config, target);

    expect(selected).toBe(onnxPolicy.targetNativeOnnxPattern({ platform, architecture }));
    expect(config.files).toEqual(['package.json', selected]);
    expect(config.asarUnpack).toEqual(['native.node', selected]);
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
        {
          platform: 'win32',
          architecture: 'x64',
        },
      ),
    ).toThrow('files is missing its ONNX native selector');
  });

  it('pins package hooks around ASAR creation in the installed builder', () => {
    const source = readFileSync(require.resolve('app-builder-lib/out/platformPackager.js'), 'utf8');
    expect(source.indexOf('emitBeforePack')).toBeGreaterThanOrEqual(0);
    expect(source.indexOf('emitBeforePack')).toBeLessThan(source.indexOf('getFileMatchersOptions'));
    expect(source.indexOf('emitBeforePack')).toBeLessThan(source.indexOf('computeAsarOptions'));
    expect(source.indexOf('emitAfterPack')).toBeGreaterThan(source.indexOf('copyAppFiles'));
    expect(source.indexOf('emitAfterPack')).toBeLessThan(source.indexOf('sanityCheckPackage'));
  });

  it('keeps afterPack as a read-only ONNX structural gate', () => {
    const source = readFileSync('app/after-pack.cjs', 'utf8');
    expect(source).toContain('await verifyPackagedStructure(context)');
    expect(source).not.toContain('pruneOnnxRuntime');
    expect(source).not.toContain('rmSync');
  });
});
