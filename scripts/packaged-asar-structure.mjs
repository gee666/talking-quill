import { listPackage, statFile } from '@electron/asar';
import { resolve, sep } from 'node:path';
import { extractRegularAsarFiles } from './asar-entry-inspection.mjs';
import { normalizePackagePath, validateAsarEntries } from './package-policy.mjs';
import { readNativeArchitectures } from './native-architecture.mjs';

export async function verifyPackagedAsarStructure(context) {
  const target = packageTarget(context);
  const resources =
    target.platform === 'mac'
      ? resolve(
          context.appOutDir,
          `${context.packager.appInfo.productFilename}.app/Contents/Resources`,
        )
      : resolve(context.appOutDir, 'resources');
  const archive = resolve(resources, 'app.asar');
  const entries = listPackage(archive).map(normalizePackagePath);
  validateAsarEntries(entries, target);
  // Full consumption enforces every metadata-to-physical check.
  Array.from(extractRegularAsarFiles(archive, entries, 'Post-pack ASAR'));
  await verifyTargetNativeOnnxArchitecture(archive, target);
}

export async function verifyTargetNativeOnnxArchitecture(archive, target) {
  const expectedFormat = target.platform === 'win' ? 'pe' : 'mach-o';
  const nativePrefix = `node_modules/onnxruntime-node/bin/napi-v3/${target.platform === 'win' ? 'win32' : 'darwin'}/${target.architecture}/`;
  for (const entry of listPackage(archive).map(normalizePackagePath)) {
    if (!entry.startsWith(nativePrefix)) continue;
    const metadata = statFile(archive, entry.split('/').join(sep), false);
    if (!Object.hasOwn(metadata, 'size')) continue;
    if (metadata.unpacked !== true) {
      throw new Error(`Post-pack ONNX native is not unpacked: ${entry}`);
    }
    const native = await readNativeArchitectures(resolve(`${archive}.unpacked`, entry));
    if (
      native === null ||
      (target.platform === 'win') !== (native.format === 'pe') ||
      native.architectures.length !== 1 ||
      native.architectures[0] !== target.architecture
    ) {
      throw new Error(
        `Post-pack ONNX native mismatch: ${entry} is not ${expectedFormat}/${target.architecture}`,
      );
    }
  }
}

function packageTarget(context) {
  const platform = context.electronPlatformName === 'win32' ? 'win' : 'mac';
  const architecture = context.arch === 1 ? 'x64' : context.arch === 3 ? 'arm64' : null;
  if (!['win32', 'darwin'].includes(context.electronPlatformName) || architecture === null) {
    throw new Error(
      `Unsupported post-pack target: ${String(context.electronPlatformName)}/${String(context.arch)}`,
    );
  }
  return { platform, architecture };
}
