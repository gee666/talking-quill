import { createHash } from 'node:crypto';
import { mkdir, readFile, readdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { basename, dirname, join, resolve } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { load, dump } from 'js-yaml';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const require = createRequire(import.meta.url);
const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
const appBuilderRoot = dirname(electronBuilderRequire.resolve('app-builder-lib/package.json'));
const { buildBlockMap } = electronBuilderRequire(
  join(appBuilderRoot, 'out', 'targets', 'blockmap', 'blockmap.js'),
);
const architecture = process.argv[2];
if (!['x64', 'arm64'].includes(architecture ?? '')) {
  throw new Error('Usage: wrap-windows-nsis.mjs <x64|arm64>');
}
const packageJson = JSON.parse(await readFile(resolve(repositoryRoot, 'package.json'), 'utf8'));
const outputDirectory = resolve(repositoryRoot, process.argv[3] ?? 'release');
const artifact = resolve(
  outputDirectory,
  `Talking-Quill-${packageJson.version}-win-${architecture}.exe`,
);
const stub = resolve(
  repositoryRoot,
  'tmp',
  'windows-bootstrap',
  architecture,
  'talking-quill-windows-bootstrap.exe',
);
const [stubBytes, innerBytes] = await Promise.all([readFile(stub), readFile(artifact)]);
requireGuiPe(stubBytes, architecture, 'native bootstrap');
requireGuiPe(innerBytes, architecture, 'inner NSIS installer');
if (innerBytes.subarray(-64, -56).toString('ascii') === 'TQNSIS01') {
  throw new Error('refusing to nest a native bootstrap around an already wrapped artifact');
}
const footer = Buffer.alloc(64);
footer.write('TQNSIS01', 0, 'ascii');
footer.writeUInt32LE(1, 8);
footer.writeBigUInt64LE(BigInt(stubBytes.length), 16);
footer.writeBigUInt64LE(BigInt(innerBytes.length), 24);
createHash('sha256').update(innerBytes).digest().copy(footer, 32);
const pending = `${artifact}.pending-native-bootstrap`;
const retainedDirectory = resolve(repositoryRoot, 'tmp', 'windows-inner-nsis', architecture);
const retained = resolve(retainedDirectory, basename(artifact).replace(/\.exe$/u, '-inner.exe'));
await mkdir(retainedDirectory, { recursive: true });
await rm(pending, { force: true });
await writeFile(pending, Buffer.concat([stubBytes, innerBytes, footer]), { flag: 'wx' });
await rm(retained, { force: true });
await rename(artifact, retained);
try {
  await rename(pending, artifact);
} catch (error) {
  await rename(retained, artifact);
  throw error;
}
try {
  await rebuildUpdateMetadata(artifact);
} catch (error) {
  await rm(artifact, { force: true });
  await rename(retained, artifact);
  throw error;
}
console.log(`Wrapped ${artifact} with native bootstrap; retained inner installer at ${retained}`);

async function rebuildUpdateMetadata(path) {
  const blockmap = `${path}.blockmap`;
  await rm(blockmap, { force: true });
  await buildBlockMap(path, 'gzip', blockmap);
  const bytes = await readFile(path);
  const sha512 = createHash('sha512').update(bytes).digest('base64');
  const size = bytes.length;
  for (const name of await readdir(dirname(path))) {
    if (!/^latest(?:-[^.]+)?\.ya?ml$/iu.test(name)) continue;
    const metadataPath = resolve(dirname(path), name);
    const metadata = load(await readFile(metadataPath, 'utf8'));
    if (typeof metadata !== 'object' || metadata === null) continue;
    let changed = false;
    for (const file of Array.isArray(metadata.files) ? metadata.files : []) {
      if (basename(file.url ?? file.path ?? '') !== basename(path)) continue;
      file.sha512 = sha512;
      file.size = size;
      changed = true;
    }
    if (basename(metadata.path ?? '') === basename(path)) {
      metadata.sha512 = sha512;
      metadata.size = size;
      changed = true;
    }
    if (changed) await writeFile(metadataPath, dump(metadata, { lineWidth: -1 }), 'utf8');
  }
  const blockmapSize = (await stat(blockmap)).size;
  if (blockmapSize === 0) throw new Error('native-bootstrap blockmap regeneration was empty');
}

function requireGuiPe(bytes, architecture, label) {
  if (bytes.length < 256 || bytes.readUInt16LE(0) !== 0x5a4d) throw new Error(`${label} is not PE`);
  const pe = bytes.readUInt32LE(0x3c);
  const machine = architecture === 'x64' ? 0x8664 : 0xaa64;
  const optional = pe + 24;
  const subsystem = optional + (bytes.readUInt16LE(optional) === 0x20b ? 68 : 68);
  if (
    bytes.readUInt32LE(pe) !== 0x0000_4550 ||
    bytes.readUInt16LE(pe + 4) !== machine ||
    bytes.readUInt16LE(subsystem) !== 2
  ) {
    throw new Error(`${label} architecture or GUI subsystem is invalid`);
  }
}
