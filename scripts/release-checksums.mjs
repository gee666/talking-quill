import { createHash } from 'node:crypto';
import { lstatSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const args = process.argv.slice(2).filter((argument) => argument !== '--');
if (args.length > 1) throw new Error('Expected zero or one release artifact directory.');
const directory = resolve(args[0] ?? 'release-artifacts');
const outputName = 'SHA256SUMS.txt';
const manifestName = 'release-manifest.json';
const manifest = JSON.parse(readFileSync(resolve(directory, manifestName), 'utf8'));
if (manifest?.schemaVersion !== 1 || !Array.isArray(manifest.assets))
  throw new Error('Release manifest is invalid.');
const expected = [...manifest.assets.map((asset) => asset.name), manifestName].sort();
const names = readdirSync(directory)
  .filter((name) => name !== outputName)
  .sort();
if (JSON.stringify(names) !== JSON.stringify(expected))
  throw new Error(`Checksum asset allowlist mismatch: ${names.join(', ')}`);
const caseFolded = names.map((name) => name.toLowerCase());
if (new Set(caseFolded).size !== names.length)
  throw new Error('Case-insensitive duplicate release asset names are forbidden.');
const manifestAssets = new Map(
  manifest.assets.map((asset) => [asset.name, { bytes: asset.bytes, sha256: asset.sha256 }]),
);
const lines = names.map((name) => {
  if (/\p{C}/u.test(name))
    throw new Error(
      `Control characters are forbidden in release asset names: ${JSON.stringify(name)}`,
    );
  const path = resolve(directory, name);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink())
    throw new Error(`Release asset must be a regular file: ${name}`);
  const digest = createHash('sha256').update(readFileSync(path)).digest('hex');
  const recorded = manifestAssets.get(name);
  if (
    name !== manifestName &&
    (recorded === undefined || recorded.bytes !== metadata.size || recorded.sha256 !== digest)
  ) {
    throw new Error(`Release asset changed after manifest assembly: ${name}`);
  }
  return `${digest}  ${name}`;
});
writeFileSync(resolve(directory, outputName), `${lines.join('\n')}\n`, 'utf8');
console.log(
  `Checksummed all ${lines.length} uploaded assets; only ${outputName} is self-excluded.`,
);
