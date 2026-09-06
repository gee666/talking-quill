import { createHash } from 'node:crypto';

const HEX = /^[0-9a-f]{64}$/u;
const ARCHITECTURES = new Set(['x64', 'arm64', 'x64+arm64']);

export function canonicalJson(value) {
  if (value === null || typeof value === 'boolean' || typeof value === 'string') {
    return JSON.stringify(value);
  }
  if (typeof value === 'number' && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (value !== null && typeof value === 'object') {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(',')}}`;
  }
  throw new Error('Release manifest contains a non-JSON value');
}

export function sealReleaseManifest(body) {
  const candidate = { ...body };
  delete candidate.manifestSha256;
  validateReleaseManifestBody(candidate);
  return {
    ...candidate,
    manifestSha256: createHash('sha256').update(canonicalJson(candidate)).digest('hex'),
  };
}

export function validateReleaseManifest(value) {
  exactObject(
    value,
    [
      'schemaVersion',
      'repository',
      'tag',
      'version',
      'sourceCommit',
      'sourceTree',
      'platform',
      'architecture',
      'promotable',
      'workflowRunId',
      'generatedAt',
      'provenance',
      'assets',
      'manifestSha256',
    ],
    'release manifest',
  );
  const { manifestSha256, ...body } = value;
  validateReleaseManifestBody(body);
  if (!HEX.test(manifestSha256 ?? '')) throw new Error('Release manifest digest is invalid');
  const actual = createHash('sha256').update(canonicalJson(body)).digest('hex');
  if (actual !== manifestSha256) throw new Error('Release manifest canonical digest mismatch');
  return value;
}

function validateReleaseManifestBody(value) {
  exactObject(
    value,
    [
      'schemaVersion',
      'repository',
      'tag',
      'version',
      'sourceCommit',
      'sourceTree',
      'platform',
      'architecture',
      'promotable',
      'workflowRunId',
      'generatedAt',
      'provenance',
      'assets',
    ],
    'release manifest body',
  );
  if (
    value.schemaVersion !== 2 ||
    typeof value.repository !== 'string' ||
    !/^v\d+\.\d+\.\d+$/u.test(value.tag ?? '') ||
    !/^\d+\.\d+\.\d+$/u.test(value.version ?? '') ||
    value.tag !== `v${value.version}` ||
    !/^[0-9a-f]{40}$/u.test(value.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(value.sourceTree ?? '') ||
    value.platform !== 'win' ||
    !ARCHITECTURES.has(value.architecture) ||
    value.promotable !== true ||
    !(value.workflowRunId === null || typeof value.workflowRunId === 'string') ||
    !(
      value.generatedAt === null ||
      (typeof value.generatedAt === 'string' && !Number.isNaN(Date.parse(value.generatedAt)))
    )
  )
    throw new Error('Release manifest identity is invalid');
  validateEntries(
    value.assets,
    ['name', 'bytes', 'sha256'],
    (entry) =>
      Number.isSafeInteger(entry.bytes) && entry.bytes >= 0 && HEX.test(entry.sha256 ?? ''),
    'asset',
  );
  validateEntries(
    value.provenance,
    ['name', 'platform', 'arch', 'mode', 'sourceTree', 'sourceTreeSha256'],
    (entry) =>
      entry.platform === 'win' &&
      ['x64', 'arm64'].includes(entry.arch) &&
      ['setup', 'update'].includes(entry.mode) &&
      entry.sourceTree === value.sourceTree &&
      /^[0-9a-f]{40}$/u.test(entry.sourceTree ?? '') &&
      HEX.test(entry.sourceTreeSha256 ?? ''),
    'provenance',
  );
  const expectedArchitectures =
    value.architecture === 'x64+arm64' ? ['arm64', 'x64'] : [value.architecture];
  // Fresh-only releases have no updater payload or update provenance. If any
  // update is present, require the complete setup/update set for every arch.
  const modes = value.provenance.some(({ mode }) => mode === 'update')
    ? ['setup', 'update']
    : ['setup'];
  const expectedPairs = expectedArchitectures.flatMap((arch) =>
    modes.map((mode) => `${arch}:${mode}`),
  );
  const actualPairs = value.provenance.map(({ arch, mode }) => `${arch}:${mode}`).sort();
  if (JSON.stringify(actualPairs) !== JSON.stringify(expectedPairs.sort())) {
    throw new Error('Release manifest setup/update architecture provenance is incomplete');
  }
}

function validateEntries(entries, keys, predicate, name) {
  if (!Array.isArray(entries) || entries.length === 0)
    throw new Error(`Release manifest ${name} list is invalid`);
  const names = [];
  for (const entry of entries) {
    exactObject(entry, keys, `release manifest ${name}`);
    if (!safeName(entry.name) || !predicate(entry))
      throw new Error(`Release manifest ${name} is invalid`);
    names.push(entry.name);
  }
  const sorted = [...names].sort();
  if (
    JSON.stringify(names) !== JSON.stringify(sorted) ||
    new Set(names.map((name_) => name_.toLowerCase())).size !== names.length
  ) {
    throw new Error(`Release manifest ${name} names must be sorted and unique`);
  }
}

function safeName(value) {
  return (
    typeof value === 'string' &&
    value.length > 0 &&
    value.length <= 255 &&
    !/[\\/\p{C}]/u.test(value) &&
    value !== '.' &&
    value !== '..'
  );
}

function exactObject(value, keys, name) {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    throw new Error(`${name} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected))
    throw new Error(`${name} fields are invalid`);
}
