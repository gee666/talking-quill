import { execFileSync, execSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

const root = resolve('.');
const output = resolve(root, 'app/assets/THIRD_PARTY_NOTICES.txt');
const lockPath = resolve(root, 'pnpm-lock.yaml');
const cargoLockPaths = [resolve(root, 'helper/Cargo.lock')];
const modelPath = resolve(root, 'scripts/model-manifest.json');
const attributionPath = resolve(root, 'docs/attribution/anythingllm-mit.txt');
const vendoredNoticeRoot = resolve(root, 'docs/third-party');
const VENDORED_MISSING_MATERIAL = Object.freeze({
  'guid-typescript@1.0.9': ['guid-typescript-1.0.9-LICENSE.txt'],
  'onnxruntime-node@1.21.0': [
    'onnxruntime/v1.21.0/LICENSE',
    'onnxruntime/v1.21.0/ThirdPartyNotices.txt',
  ],
  'onnxruntime-common@1.21.0': [
    'onnxruntime/v1.21.0/LICENSE',
    'onnxruntime/v1.21.0/ThirdPartyNotices.txt',
  ],
  'onnxruntime-common@1.22.0-dev.20250409-89f8206ba4': [
    'onnxruntime/89f8206ba4/LICENSE',
    'onnxruntime/89f8206ba4/ThirdPartyNotices.txt',
  ],
  'onnxruntime-web@1.22.0-dev.20250409-89f8206ba4': [
    'onnxruntime/89f8206ba4/LICENSE',
    'onnxruntime/89f8206ba4/ThirdPartyNotices.txt',
  ],
});
const [lock, cargoLocks, modelSource, anythingLlmMit] = await Promise.all([
  readFile(lockPath, 'utf8'),
  Promise.all(cargoLockPaths.map((path) => readFile(path, 'utf8'))),
  readFile(modelPath, 'utf8'),
  readFile(attributionPath, 'utf8'),
]);

const licenseReport = JSON.parse(
  execSync('pnpm --config.ignore-pnpmfile=true licenses list --prod --json', {
    cwd: root,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  }),
);
const licenseIndex = new Map();
for (const [license, packages] of Object.entries(licenseReport)) {
  for (const entry of packages) {
    for (const version of entry.versions) {
      licenseIndex.set(`${entry.name}@${version}`, {
        license,
        url:
          entry.homepage ??
          `https://www.npmjs.com/package/${encodeURIComponent(entry.name)}/v/${version}`,
      });
    }
  }
}
const workerBuild = await readFile(resolve(root, 'scripts/build-whisper-worker.mjs'), 'utf8');
if (
  !workerBuild.includes(
    "sharp: resolve(appRoot, 'src', 'workers', 'whisper', 'sharp-unavailable.ts')",
  )
) {
  throw new Error('Expected the production Sharp replacement before excluding its optional graph.');
}
const deployment = JSON.parse(
  execSync(
    'pnpm --config.ignore-pnpmfile=true --filter @talking-quill/app list --prod --json --depth Infinity',
    {
      cwd: root,
      encoding: 'utf8',
      maxBuffer: 16 * 1024 * 1024,
    },
  ),
)[0];
if (deployment?.name !== '@talking-quill/app')
  throw new Error('Production app dependency graph unavailable.');
const npmPackages = new Map();
collectNpmDependencies(deployment.dependencies ?? {}, npmPackages, licenseIndex);
// electron-builder ships the Electron runtime itself outside app node_modules.
await addNpmPackage(resolve(root, 'node_modules/electron'), npmPackages);
const npmRecords = [...npmPackages.values()].sort(compareRecord);
assertCompleteLicenses(npmRecords, 'JavaScript');

const cargo = resolveRustTool('cargo');
const rustTargets = [
  'x86_64-pc-windows-msvc',
  'aarch64-pc-windows-msvc',
  'x86_64-apple-darwin',
  'aarch64-apple-darwin',
];
const cargoPackages = new Map();
for (const target of rustTargets) {
  for (const manifestPath of ['helper/Cargo.toml']) {
    const tree = execFileSync(
      cargo,
      [
        'tree',
        '--manifest-path',
        manifestPath,
        '--locked',
        '--offline',
        '--target',
        target,
        '-e',
        'normal',
        '--prefix',
        'none',
        '--format',
        '{p}|{l}',
      ],
      { cwd: root, encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 },
    );
    for (const line of tree.split(/\r?\n/u)) {
      if (line.trim().length === 0) continue;
      const [identitySource, licenseSource] = line.split('|');
      const identity =
        identitySource?.replace(/ \(\*\)$/u, '').replace(/ \(proc-macro\)$/u, '') ?? '';
      const match = /^([^ ]+) v([^ ]+)/u.exec(identity);
      const license = licenseSource?.trim().replace(/ \(\*\)$/u, '') ?? '';
      if (match === null || license.length === 0)
        throw new Error(`Invalid Cargo license record: ${line}`);
      const workspacePackage = match[1] === 'talking-quill-helper';
      const record = {
        name: match[1],
        version: match[2],
        license,
        url: workspacePackage
          ? 'https://github.com/gee666/talking-quill'
          : `https://crates.io/crates/${match[1]}/${match[2]}`,
        targets: new Set([target]),
        directory: workspacePackage ? root : findCargoDirectory(match[1], match[2]),
      };
      const key = `${record.name}@${record.version}`;
      const existing = cargoPackages.get(key);
      if (existing === undefined) cargoPackages.set(key, record);
      else {
        if (existing.license !== record.license)
          throw new Error(`Conflicting Cargo license: ${key}`);
        existing.targets.add(target);
      }
    }
  }
}
const cargoRecords = [...cargoPackages.values()].sort(compareRecord);
assertCompleteLicenses(cargoRecords, 'Rust');
const embeddedLicenseTexts = collectLicenseTexts([...npmRecords, ...cargoRecords]);

const manifest = JSON.parse(modelSource);
const modelLicenses = new Map([
  ['onnx-community/whisper-large-v3-turbo', 'MIT'],
  ['Xenova/whisper-small', 'Apache-2.0'],
]);
const modelLicense = readFileSync(
  resolve(vendoredNoticeRoot, 'models/LICENSE-Apache-2.0.txt'),
  'utf8',
);
const models = manifest.models
  .map((model) => {
    const cardPath = resolve(
      vendoredNoticeRoot,
      `models/${model.id.replace('/', '-')}-${model.revision}.README.md`,
    );
    if (!existsSync(cardPath))
      throw new Error(`Pinned model card missing: ${model.id}@${model.revision}`);
    const card = readFileSync(cardPath, 'utf8');
    const license = modelLicenses.get(model.id);
    const expectedCardMetadata =
      license === 'Apache-2.0'
        ? /^license: apache-2\.0$/mu
        : /^base_model: openai\/whisper-large-v3-turbo$/mu;
    if (license === undefined || !expectedCardMetadata.test(card))
      throw new Error(
        `Pinned model license/source metadata is missing or unexpected: ${model.id}@${model.revision}`,
      );
    const files = model.files
      .map((file) => `  - ${file.path} (${file.size} bytes; SHA-256 ${file.sha256})`)
      .join('\n');
    return `${model.id}\nRevision: ${model.revision}\nDeclared license: ${license}\nAuthoritative repository: https://huggingface.co/${model.id}/tree/${model.revision}\nPinned model card:\n${card.trim()}\n\nPinned file inventory:\n${files}`;
  })
  .join('\n\n');
const npmInventory = npmRecords
  .map((entry) => `${entry.name}@${entry.version} — SPDX: ${entry.license}\n  ${entry.url}`)
  .join('\n');
const cargoInventory = cargoRecords
  .map(
    (entry) =>
      `${entry.name}@${entry.version} — SPDX: ${entry.license}\n  ${entry.url}\n  Targets: ${[...entry.targets].sort().join(', ')}`,
  )
  .join('\n');
const content = `Talking Quill — Production Third-Party Notices

This inventory is generated from the production @talking-quill/app deployment graph, the shipped Electron runtime, and Cargo's offline normal-dependency trees for the Windows and macOS helper targets. Development-only and test-only dependencies are excluded.

Source fingerprints
pnpm-lock.yaml SHA-256: ${sha256(lock)}
helper/Cargo.lock SHA-256: ${sha256(cargoLocks[0])}
scripts/model-manifest.json SHA-256: ${sha256(modelSource)}

Production JavaScript/native dependencies (${npmRecords.length} records)
${npmInventory}

Shipped Rust helper dependencies (${cargoRecords.length} records)
${cargoInventory}

Embedded LICENSE/NOTICE/copyright texts (content-hash deduplicated)
${embeddedLicenseTexts}

License identifiers above are package-declared SPDX expressions. URLs are identifiers only and are not substitutes for required license text.

OpenAI Whisper and Whisper model-family attribution
Copyright (c) 2022 OpenAI

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

Whisper model artifacts
${models}

Apache License 2.0 text applicable to the Xenova Whisper Small model revision
${modelLicense.trim()}

Model-family source: OpenAI Whisper, https://github.com/openai/whisper

AnythingLLM MIT attribution
Provider and local Whisper behavior was independently implemented with reference to MIT-licensed source. Its attribution is preserved under the complete terms below:

${anythingLlmMit.trim()}

Legal review limitation
This generated engineering inventory records available attribution material but does not claim legal sufficiency. Distribution still requires qualified legal review.
`;

const normalizedContent = content.replace(/\r\n?/gu, '\n');
if (process.argv.includes('--check')) {
  const existing = await readFile(output, 'utf8').catch(() => '');
  if (existing !== normalizedContent)
    throw new Error('Third-party notices are stale. Run pnpm notices.');
  console.log('Production third-party notices are current.');
} else {
  await writeFile(output, normalizedContent, 'utf8');
  console.log(`Wrote ${output}`);
}

function collectNpmDependencies(dependencies, records, licenses) {
  for (const [dependencyName, dependency] of Object.entries(dependencies)) {
    // The worker build replaces Sharp with a local fail-closed module, so neither Sharp nor its
    // platform-specific optional binaries are shipped.
    if (dependencyName === 'sharp') continue;
    if (typeof dependency?.version !== 'string')
      throw new Error('Dependency version missing from production graph.');
    const key = `${dependencyName}@${dependency.version}`;
    const declared = licenses.get(key);
    if (declared === undefined)
      throw new Error(`Production package license metadata missing: ${key}`);
    if (
      typeof dependency.path === 'string' &&
      existsSync(resolve(dependency.path, 'package.json'))
    ) {
      const manifest = JSON.parse(readFileSync(resolve(dependency.path, 'package.json'), 'utf8'));
      if (
        manifest.name !== dependencyName ||
        manifest.version !== dependency.version ||
        manifest.license !== declared.license
      ) {
        throw new Error(
          `Installed production package metadata disagrees with license report: ${key}`,
        );
      }
    }
    records.set(key, {
      name: dependencyName,
      version: dependency.version,
      ...declared,
      directory: typeof dependency.path === 'string' ? resolve(dependency.path) : undefined,
    });
    collectNpmDependencies(dependency.dependencies ?? {}, records, licenses);
  }
}
async function addNpmPackage(directory, records) {
  const manifest = JSON.parse(await readFile(resolve(directory, 'package.json'), 'utf8'));
  const record = npmRecord(manifest, directory);
  records.set(`${record.name}@${record.version}`, record);
}
function npmRecord(manifest, directory) {
  if (
    typeof manifest.name !== 'string' ||
    typeof manifest.version !== 'string' ||
    typeof manifest.license !== 'string'
  ) {
    throw new Error('Production package is missing name, version, or SPDX license metadata.');
  }
  return {
    name: manifest.name,
    version: manifest.version,
    license: manifest.license.trim(),
    url: `https://www.npmjs.com/package/${encodeURIComponent(manifest.name)}/v/${manifest.version}`,
    directory,
  };
}
function collectLicenseTexts(records) {
  const byHash = new Map();
  for (const record of records) {
    const directory = record.directory;
    if (typeof directory !== 'string' || !existsSync(directory)) {
      throw new Error(`Installed license directory missing: ${record.name}@${record.version}`);
    }
    const files = readdirSync(directory)
      .filter((name) => /^(?:licen[cs]e|notice|copying|copyright)(?:[._-].*)?$/iu.test(name))
      .filter((name) => statSync(resolve(directory, name)).isFile())
      .sort();
    if (files.length === 0) {
      const identity = `${record.name}@${record.version}`;
      const vendored = VENDORED_MISSING_MATERIAL[identity];
      if (vendored === undefined) {
        throw new Error(
          `Required LICENSE/NOTICE text missing without an exact reviewed source mapping: ${identity}`,
        );
      }
      for (const relative of vendored) {
        const path = resolve(vendoredNoticeRoot, relative);
        if (!existsSync(path))
          throw new Error(`Vendored license material is missing: ${identity}:${relative}`);
        const text = readFileSync(path, 'utf8').trim();
        if (text.length < 20)
          throw new Error(`Vendored license material is empty: ${identity}:${relative}`);
        const hash = sha256(text);
        const owner = `${identity}/${relative}`;
        const existing = byHash.get(hash);
        if (existing === undefined) byHash.set(hash, { text, owners: [owner] });
        else existing.owners.push(owner);
      }
      continue;
    }
    for (const name of files) {
      const text = readFileSync(resolve(directory, name), 'utf8').trim();
      if (text.length < 20)
        throw new Error(`Required license text is empty: ${record.name}/${name}`);
      const hash = sha256(text);
      const owner = `${record.name}@${record.version}/${name}`;
      const existing = byHash.get(hash);
      if (existing === undefined) byHash.set(hash, { text, owners: [owner] });
      else existing.owners.push(owner);
    }
  }
  return [...byHash.entries()]
    .sort(([left], [right]) => left.localeCompare(right, 'en'))
    .map(
      ([hash, entry]) =>
        `--- SHA-256 ${hash}\nApplies to: ${entry.owners.sort().join(', ')}\n\n${entry.text}`,
    )
    .join('\n\n');
}

function assertCompleteLicenses(records, kind) {
  if (records.length === 0) throw new Error(`${kind} production inventory is empty.`);
  for (const record of records) {
    if (/^(?:unknown|unlicensed|see |n\/a|none|)$/iu.test(record.license)) {
      throw new Error(
        `${kind} dependency has missing or placeholder license: ${record.name}@${record.version}`,
      );
    }
  }
}
function findCargoDirectory(name, version) {
  const sourceRoot = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'registry', 'src');
  if (!existsSync(sourceRoot)) return undefined;
  for (const index of readdirSync(sourceRoot).sort()) {
    const candidate = join(sourceRoot, index, `${name}-${version}`);
    if (existsSync(candidate)) return candidate;
  }
  return undefined;
}

function resolveRustTool(name) {
  const executable = process.platform === 'win32' ? `${name}.exe` : name;
  const candidate = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin', executable);
  return existsSync(candidate) ? candidate : executable;
}
function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}
function compareRecord(left, right) {
  return `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`, 'en');
}
