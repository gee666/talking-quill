import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdir, readFile, rename, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';

const ESPEAK = 'C:/Program Files/eSpeak NG/espeak-ng.exe';
const FFMPEG = 'ffmpeg';
const SAMPLE_RATE = 16_000;
const FIXTURE_DIRECTORY = resolve('tests', 'fixtures', 'audio');
const TEMPORARY_DIRECTORY = resolve('tmp', 'audio-fixture-generation');
const fixtures = [
  {
    name: 'speech-short-16k-mono.wav',
    durationSeconds: 8,
    expectedSha256: '5e8257938418febb3687496a48ab80cac537c054cd4a52a749ade967bf8ee59a',
  },
  {
    name: 'speech-boundaries-90s-16k-mono.wav',
    durationSeconds: 90,
    expectedSha256: 'e1da4174bda6cc213003773fc6ef3327ec7462443d0e4fc0d951904e0b3b4c91',
  },
];
const shortText =
  'Talking Quill keeps every spoken word private and local. Clear speech becomes useful text.';
const boundaryClips = [
  [5_000, 'The session begins with a calm natural sentence.'],
  [15_000, 'We continue speaking at a steady pace.'],
  [24_100, 'alpha boundary marker'],
  [35_000, 'The middle section remains clear and conversational.'],
  [44_100, 'bravo boundary marker'],
  [55_000, 'Another ordinary sentence keeps the recording realistic.'],
  [64_100, 'charlie boundary marker'],
  [75_000, 'The final section continues without rushing.'],
  [84_100, 'delta. delta boundary marker'],
];

const checkOnly = process.argv.includes('--check');
const writeFixtures = process.argv.includes('--write');
if (checkOnly === writeFixtures) throw new Error('Expected exactly one mode: --check or --write');

if (writeFixtures) {
  await generateFixtures();
}
for (const fixture of fixtures) {
  const path = join(FIXTURE_DIRECTORY, fixture.name);
  const bytes = await readFile(path);
  const metadata = parsePcm16Wav(bytes);
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  if (
    metadata.sampleRate !== SAMPLE_RATE ||
    metadata.channels !== 1 ||
    metadata.bitsPerSample !== 16
  ) {
    throw new Error(`${fixture.name} is not 16 kHz mono PCM16`);
  }
  if (metadata.samples !== fixture.durationSeconds * SAMPLE_RATE) {
    throw new Error(`${fixture.name} has an unexpected duration`);
  }
  if (checkOnly && sha256 !== fixture.expectedSha256) {
    throw new Error(`${fixture.name} SHA-256 mismatch: ${sha256}`);
  }
  console.log(
    `${fixture.name}: sha256=${sha256} samples=${metadata.samples} duration=${fixture.durationSeconds}s`,
  );
}
if (checkOnly) console.log('Committed realistic speech fixtures verified');

async function generateFixtures() {
  await rm(TEMPORARY_DIRECTORY, { recursive: true, force: true });
  await Promise.all([
    mkdir(TEMPORARY_DIRECTORY, { recursive: true }),
    mkdir(FIXTURE_DIRECTORY, { recursive: true }),
  ]);
  const shortSource = join(TEMPORARY_DIRECTORY, 'short-espeak.wav');
  run(ESPEAK, ['-v', 'en-us', '-s', '145', '-p', '45', '-a', '150', '-w', shortSource, shortText]);
  const shortOutput = join(TEMPORARY_DIRECTORY, fixtures[0].name);
  run(FFMPEG, [
    '-hide_banner',
    '-loglevel',
    'error',
    '-y',
    '-i',
    shortSource,
    '-af',
    'aresample=16000,apad=whole_dur=8,atrim=duration=8',
    '-ac',
    '1',
    '-ar',
    String(SAMPLE_RATE),
    '-c:a',
    'pcm_s16le',
    '-fflags',
    '+bitexact',
    '-flags:a',
    '+bitexact',
    '-map_metadata',
    '-1',
    shortOutput,
  ]);

  const clipPaths = [];
  for (const [index, [, text]] of boundaryClips.entries()) {
    const path = join(TEMPORARY_DIRECTORY, `boundary-${String(index)}.wav`);
    run(ESPEAK, ['-v', 'en-us', '-s', '145', '-p', '45', '-a', '150', '-w', path, text]);
    clipPaths.push(path);
  }
  const boundaryOutput = join(TEMPORARY_DIRECTORY, fixtures[1].name);
  const inputArguments = [
    '-f',
    'lavfi',
    '-i',
    'anullsrc=r=16000:cl=mono:d=90',
    ...clipPaths.flatMap((path) => ['-i', path]),
  ];
  const delayed = boundaryClips.map(
    ([delay], index) =>
      `[${String(index + 1)}:a]aresample=16000,adelay=${String(delay)}:all=1[c${String(index)}]`,
  );
  const mixedInputs = ['[0:a]', ...boundaryClips.map((_, index) => `[c${String(index)}]`)].join('');
  const filter = `${delayed.join(';')};${mixedInputs}amix=inputs=${String(boundaryClips.length + 1)}:duration=first:dropout_transition=0:normalize=0,alimiter=limit=0.95,atrim=duration=90[out]`;
  run(FFMPEG, [
    '-hide_banner',
    '-loglevel',
    'error',
    '-y',
    ...inputArguments,
    '-filter_complex',
    filter,
    '-map',
    '[out]',
    '-ac',
    '1',
    '-ar',
    String(SAMPLE_RATE),
    '-c:a',
    'pcm_s16le',
    '-fflags',
    '+bitexact',
    '-flags:a',
    '+bitexact',
    '-map_metadata',
    '-1',
    boundaryOutput,
  ]);

  for (const fixture of fixtures) {
    await rm(join(FIXTURE_DIRECTORY, fixture.name), { force: true });
    await rename(join(TEMPORARY_DIRECTORY, fixture.name), join(FIXTURE_DIRECTORY, fixture.name));
  }
  await rm(TEMPORARY_DIRECTORY, { recursive: true, force: true });
}

function run(command, arguments_) {
  const result = spawnSync(command, arguments_, { encoding: 'utf8', windowsHide: true });
  if (result.error !== undefined) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${command} failed (${String(result.status)}): ${result.stderr || result.stdout}`,
    );
  }
}

function parsePcm16Wav(bytes) {
  if (bytes.toString('ascii', 0, 4) !== 'RIFF' || bytes.toString('ascii', 8, 12) !== 'WAVE') {
    throw new Error('Expected RIFF/WAVE');
  }
  let offset = 12;
  let format = null;
  let dataLength = null;
  while (offset + 8 <= bytes.length) {
    const id = bytes.toString('ascii', offset, offset + 4);
    const length = bytes.readUInt32LE(offset + 4);
    const body = offset + 8;
    if (body + length > bytes.length) throw new Error('Invalid WAV chunk length');
    if (id === 'fmt ') {
      format = {
        audioFormat: bytes.readUInt16LE(body),
        channels: bytes.readUInt16LE(body + 2),
        sampleRate: bytes.readUInt32LE(body + 4),
        bitsPerSample: bytes.readUInt16LE(body + 14),
      };
    }
    if (id === 'data') dataLength = length;
    offset = body + length + (length % 2);
  }
  if (format === null || dataLength === null || format.audioFormat !== 1) {
    throw new Error('Expected PCM WAV data');
  }
  return { ...format, samples: dataLength / (format.bitsPerSample / 8) / format.channels };
}
