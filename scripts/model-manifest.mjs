import { createHash, randomUUID } from 'node:crypto';
import { lstat, mkdir, readFile, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';

const TRANSFORMERS_VERSION = '3.8.1';
const REQUEST_TIMEOUT_MS = 30_000;
const MAX_REDIRECTS = 5;
const METADATA_PATHS = [
  'config.json',
  'generation_config.json',
  'preprocessor_config.json',
  'tokenizer.json',
  'tokenizer_config.json',
];
const WEIGHT_PATHS = [
  'onnx/encoder_model_quantized.onnx',
  'onnx/decoder_model_merged_quantized.onnx',
];
const REQUIRED_PATHS = [...METADATA_PATHS, ...WEIGHT_PATHS];
const PINS = [
  {
    id: 'onnx-community/whisper-large-v3-turbo',
    revision: '360ebcde2559d60bb474678be3c1de9ef347d01a',
  },
  { id: 'Xenova/whisper-small', revision: '2d67713f236afa48a18992566e7647f6ca848e13' },
];
const EXPECTED_FILES = new Map([
  [
    'onnx-community/whisper-large-v3-turbo',
    [
      ['config.json', 1332, '35cd83669f75bc2867f3b3a4461850392d5e308cd6ea951c3700539883c28df1'],
      [
        'generation_config.json',
        3897,
        '16f95291d2f47c944d3c2b19390bba7965666555c1ea2a0bdc850d1fab45612f',
      ],
      [
        'preprocessor_config.json',
        340,
        '7ccc62c6f2765af1f3b46c00c9b5894426835a05021c8b9c01eecb6dfb542711',
      ],
      [
        'tokenizer.json',
        2480617,
        '6d8cbd7cd0d8d5815e478dac67b85a26bbe77c1f5e0c6d76d1ce2abc0e5f21ca',
      ],
      [
        'tokenizer_config.json',
        282843,
        '844b642c73a91359722f47b35705f7174686df33d252695d8572cf9ac03a6389',
      ],
      [
        'onnx/encoder_model_quantized.onnx',
        644822195,
        'd2f853dc3254fdc0079f55dd4433ea716ac98ec5574d3b475f288f2a77cebba9',
      ],
      [
        'onnx/decoder_model_merged_quantized.onnx',
        439936716,
        '61481bd3be3a445d5a4b9070e8f8b2c6cc4fbbbbdc9f0e7ed048a132b8b84e0d',
      ],
    ],
  ],
  [
    'Xenova/whisper-small',
    [
      ['config.json', 2232, '5a6429d21d7a3379dd0861b74510f9f7076f32b563bffc9fcb072482d55ab3be'],
      [
        'generation_config.json',
        3837,
        '0b7407a4e53a677f826e03c75d409e6f830663932bf43dda3b08c5efa2223279',
      ],
      [
        'preprocessor_config.json',
        339,
        'a6a76d28c93edb273669eb9e0b0636a2bddbb1272c3261e47b7ca6dfdbac1b8d',
      ],
      [
        'tokenizer.json',
        2480466,
        '27fc476bfe7f17299480be2273fc0608e4d5a99aba2ab5dec5374b4482d1a566',
      ],
      [
        'tokenizer_config.json',
        282683,
        '2a4c4281cf9f51ac6ccc406fdc711a087afe6530f671fa7b80953edc498275ce',
      ],
      [
        'onnx/encoder_model_quantized.onnx',
        92324809,
        '969f5ac12974340386bf7a02ea6626003e5e2dee396ffc6ab0eec282bf55ba06',
      ],
      [
        'onnx/decoder_model_merged_quantized.onnx',
        156780950,
        'fcfc6100dc7339e7507e10f8b274350be7c4f8d8b575f0293f94cc0e156d6d24',
      ],
    ],
  ],
]);
const EXPECTED_TOTALS = new Map([
  ['onnx-community/whisper-large-v3-turbo', 1087527940],
  ['Xenova/whisper-small', 251875316],
]);

const arguments_ = process.argv.slice(2);
const modes = ['--check', '--verify-live', '--write'].filter((mode) => arguments_.includes(mode));
if (modes.length !== 1)
  throw new Error('Expected exactly one mode: --check, --verify-live, or --write');
const mode = modes[0];
const outputPath = readOutputPath(arguments_);

if (mode === '--check') {
  const text = await readFile(outputPath, 'utf8');
  const manifest = validateExactCommittedManifest(JSON.parse(text));
  if (text !== serialize(manifest)) throw new Error('Model manifest is not canonical JSON');
  console.log(`Pinned model manifest passed deterministic offline validation: ${outputPath}`);
  process.exit(0);
}

const generated = validateLiveManifest({
  schemaVersion: 1,
  transformersVersion: TRANSFORMERS_VERSION,
  models: await Promise.all(PINS.map(inspectLiveModel)),
});
const serialized = serialize(generated);
if (mode === '--verify-live') {
  const committedText = await readFile(outputPath, 'utf8');
  const committed = validateExactCommittedManifest(JSON.parse(committedText));
  if (serialized !== serialize(committed)) {
    throw new Error(`Pinned Hugging Face source does not match ${outputPath}`);
  }
  console.log('Pinned model source verified live without downloading model weights');
} else {
  await writeAtomic(outputPath, serialized);
  console.log(`Atomically wrote live-derived model manifest: ${outputPath}`);
}

async function inspectLiveModel(pin) {
  const apiUrl = `https://huggingface.co/api/models/${pin.id}/revision/${pin.revision}?blobs=true`;
  const metadata = await fetchJson(apiUrl, 4 * 1024 * 1024);
  if (metadata.sha !== pin.revision || !Array.isArray(metadata.siblings)) {
    throw new Error(`Pinned revision was not returned for ${pin.id}`);
  }
  const siblings = new Map(metadata.siblings.map((entry) => [entry.rfilename, entry]));
  const files = [];
  for (const path of METADATA_PATHS) {
    const entry = siblings.get(path);
    if (!entry || !Number.isSafeInteger(entry.size) || entry.size <= 0) {
      throw new Error(`Missing metadata artifact ${pin.id}/${path}`);
    }
    const bytes = await fetchBytes(resolveArtifactUrl(pin, path), 8 * 1024 * 1024);
    if (bytes.byteLength !== entry.size) throw new Error(`Size mismatch for ${pin.id}/${path}`);
    files.push({
      path,
      size: bytes.byteLength,
      sha256: createHash('sha256').update(bytes).digest('hex'),
    });
  }
  for (const path of WEIGHT_PATHS) {
    const entry = siblings.get(path);
    const size = entry?.lfs?.size;
    const sha256 = entry?.lfs?.sha256;
    if (!Number.isSafeInteger(size) || size <= 0 || !/^[a-f0-9]{64}$/.test(sha256 ?? '')) {
      throw new Error(`Missing immutable LFS metadata for ${pin.id}/${path}`);
    }
    files.push({ path, size, sha256 });
  }
  return {
    id: pin.id,
    revision: pin.revision,
    dtype: 'q8',
    totalBytes: files.reduce((sum, file) => sum + file.size, 0),
    files,
  };
}

function validateLiveManifest(value) {
  if (
    typeof value !== 'object' ||
    value === null ||
    value.schemaVersion !== 1 ||
    value.transformersVersion !== TRANSFORMERS_VERSION ||
    !Array.isArray(value.models) ||
    value.models.length !== PINS.length
  ) {
    throw new Error('Invalid live model manifest envelope');
  }
  for (const [index, pin] of PINS.entries()) {
    const model = value.models[index];
    if (
      typeof model !== 'object' ||
      model === null ||
      model.id !== pin.id ||
      model.revision !== pin.revision ||
      model.dtype !== 'q8' ||
      !Number.isSafeInteger(model.totalBytes) ||
      model.totalBytes <= 0 ||
      !Array.isArray(model.files) ||
      model.files.length !== REQUIRED_PATHS.length
    ) {
      throw new Error(`Invalid live manifest entry for ${pin.id}`);
    }
    for (const [fileIndex, path] of REQUIRED_PATHS.entries()) {
      const file = model.files[fileIndex];
      if (
        typeof file !== 'object' ||
        file === null ||
        file.path !== path ||
        !Number.isSafeInteger(file.size) ||
        file.size <= 0 ||
        !/^[a-f0-9]{64}$/.test(file.sha256 ?? '')
      ) {
        throw new Error(`Invalid live artifact ${pin.id}/${path}`);
      }
    }
    if (model.files.reduce((sum, file) => sum + file.size, 0) !== model.totalBytes) {
      throw new Error(`Invalid live total size for ${pin.id}`);
    }
  }
  return value;
}

function validateExactCommittedManifest(value) {
  const validated = validateLiveManifest(value);
  for (const model of validated.models) {
    const expectedFiles = EXPECTED_FILES.get(model.id);
    const expectedTotal = EXPECTED_TOTALS.get(model.id);
    if (expectedFiles === undefined || model.totalBytes !== expectedTotal) {
      throw new Error(`Unexpected committed model totals for ${model.id}`);
    }
    for (const [index, expected] of expectedFiles.entries()) {
      const file = model.files[index];
      if (
        file === undefined ||
        file.path !== expected[0] ||
        file.size !== expected[1] ||
        file.sha256 !== expected[2]
      ) {
        throw new Error(`Committed artifact does not match exact pin: ${model.id}/${expected[0]}`);
      }
    }
  }
  return validated;
}

async function fetchJson(url, maximumBytes) {
  return JSON.parse(new TextDecoder().decode(await fetchBytes(url, maximumBytes)));
}

async function fetchBytes(initialUrl, maximumBytes) {
  let current = new URL(initialUrl);
  for (let redirects = 0; redirects <= MAX_REDIRECTS; redirects += 1) {
    assertSafeUrl(current);
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort('request timeout'), REQUEST_TIMEOUT_MS);
    timer.unref();
    let response;
    try {
      response = await fetch(current, { redirect: 'manual', signal: controller.signal });
    } finally {
      clearTimeout(timer);
    }
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      const location = response.headers.get('location');
      await response.body?.cancel().catch(() => undefined);
      if (location === null || redirects === MAX_REDIRECTS) {
        throw new Error(`Unsafe or excessive redirect from ${current.origin}`);
      }
      current = new URL(location, current);
      continue;
    }
    if (!response.ok) {
      await response.body?.cancel().catch(() => undefined);
      throw new Error(`Unable to read ${current.origin}: HTTP ${response.status}`);
    }
    const declaredLength = Number(response.headers.get('content-length'));
    if (Number.isFinite(declaredLength) && declaredLength > maximumBytes) {
      await response.body?.cancel().catch(() => undefined);
      throw new Error(`Response from ${current.origin} exceeded its size limit`);
    }
    if (response.body === null) throw new Error(`Response from ${current.origin} had no body`);
    const reader = response.body.getReader();
    const chunks = [];
    let total = 0;
    let complete = false;
    try {
      for (;;) {
        const next = await readWithTimeout(reader, REQUEST_TIMEOUT_MS);
        if (next.done) {
          complete = true;
          break;
        }
        total += next.value.byteLength;
        if (total > maximumBytes) {
          throw new Error(`Response from ${current.origin} exceeded its size limit`);
        }
        chunks.push(next.value);
      }
    } finally {
      if (!complete) await reader.cancel().catch(() => undefined);
    }
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return bytes;
  }
  throw new Error('Redirect handling failed');
}

function readWithTimeout(reader, timeoutMs) {
  return new Promise((resolveRead, reject) => {
    const timer = setTimeout(
      () => reject(new Error('Manifest response became inactive')),
      timeoutMs,
    );
    timer.unref();
    reader.read().then(
      (value) => {
        clearTimeout(timer);
        resolveRead(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function assertSafeUrl(url) {
  if (
    url.protocol !== 'https:' ||
    !(
      url.hostname === 'huggingface.co' ||
      url.hostname === 'hf.co' ||
      url.hostname.endsWith('.hf.co') ||
      url.hostname.endsWith('.huggingface.co') ||
      url.hostname.endsWith('.xethub.hf.co')
    )
  ) {
    throw new Error(`Untrusted manifest source: ${url.origin}`);
  }
}

async function writeAtomic(path, value) {
  await mkdir(dirname(path), { recursive: true });
  const temporary = `${path}.${randomUUID()}.tmp`;
  const backup = `${path}.replaced`;
  const [targetExists, backupExists] = await Promise.all([exists(path), exists(backup)]);
  if (!targetExists && backupExists) await rename(backup, path);
  else if (targetExists && backupExists) await rm(backup, { force: true });
  await writeFile(temporary, value, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
  let moved = false;
  try {
    try {
      await rename(path, backup);
      moved = true;
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
    }
    await rename(temporary, path);
  } catch (error) {
    if (moved) await rename(backup, path).catch(() => undefined);
    await rm(temporary, { force: true });
    throw error;
  }
  if (moved) await rm(backup, { force: true }).catch(() => undefined);
}

async function exists(path) {
  try {
    await lstat(path);
    return true;
  } catch (error) {
    if (error?.code === 'ENOENT') return false;
    throw error;
  }
}

function resolveArtifactUrl(model, path) {
  const encodedPath = path.split('/').map(encodeURIComponent).join('/');
  return `https://huggingface.co/${model.id}/resolve/${model.revision}/${encodedPath}`;
}

function readOutputPath(arguments_) {
  const inline = arguments_.find((argument) => argument.startsWith('--output='));
  if (inline !== undefined) return resolve(inline.slice('--output='.length));
  const index = arguments_.indexOf('--output');
  if (index >= 0) {
    const value = arguments_[index + 1];
    if (value === undefined || value.startsWith('--')) throw new Error('--output requires a path');
    return resolve(value);
  }
  return resolve(import.meta.dirname, 'model-manifest.json');
}

function serialize(value) {
  return `${JSON.stringify(value, null, 2)}\n`;
}
