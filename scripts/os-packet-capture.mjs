import { spawn, spawnSync } from 'node:child_process';
import { lstat, mkdir, readFile, realpath, rm } from 'node:fs/promises';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));

export function parseTcpdumpInterfaces(source) {
  return Object.freeze(
    source
      .split(/\r?\n/u)
      .map((line) => /^\s*\d+\.([^\s]+)/u.exec(line)?.[1])
      .filter((value) => value !== undefined),
  );
}

export function selectMacCaptureInterface(interfaces, requested = 'pktap', scope = 'all') {
  if (!['pktap', 'lo0'].includes(requested))
    throw new Error('macOS capture interface must be pktap or lo0.');
  if (!interfaces.includes(requested))
    throw new Error(`tcpdump interface ${requested} is unavailable; refusing incomplete capture.`);
  if (requested === 'lo0' && scope !== 'loopback')
    throw new Error(
      'lo0 cannot prove all-interface coverage; use pktap or explicitly request loopback scope.',
    );
  return requested;
}

export function macTcpdumpArguments(output, interfaces, requested, scope) {
  return ['-i', selectMacCaptureInterface(interfaces, requested, scope), '-U', '-w', output];
}

function detectCaptureStatus(platform = process.platform) {
  const tool = platform === 'win32' ? 'pktmon' : platform === 'darwin' ? 'tcpdump' : null;
  const toolAvailable = tool !== null && commandExists(tool, platform);
  let interfaces = [];
  if (platform === 'darwin' && toolAvailable) {
    const result = spawnSync('tcpdump', ['-D'], { encoding: 'utf8' });
    if (result.status === 0) interfaces = parseTcpdumpInterfaces(result.stdout);
  }
  return Object.freeze({
    schemaVersion: 2,
    evidenceType: 'os-packet-capture',
    platform,
    tool,
    toolAvailable,
    interfaces,
    permissionVerified: false,
    limitation: toolAvailable
      ? 'Tool and interfaces were enumerated. Permission, readiness, and capture completeness are verified only by an actual run and independent pcap review.'
      : 'No supported OS packet capture tool was detected; instrumentation is not a substitute for OS packet capture.',
  });
}

export function parseCaptureInvocation(arguments_, root = repositoryRoot) {
  const separator = arguments_.indexOf('--');
  if (separator < 0) throw new Error('Expected -- before the captured command.');
  const options = new Map();
  for (const argument of arguments_.slice(0, separator)) {
    const prefix = ['--output=', '--interface=', '--scope='].find((item) =>
      argument.startsWith(item),
    );
    if (prefix === undefined) throw new Error(`Unknown capture option: ${argument}`);
    if (options.has(prefix)) throw new Error(`Duplicate capture option: ${prefix.slice(0, -1)}`);
    options.set(prefix, argument.slice(prefix.length));
  }
  const command = arguments_[separator + 1];
  if (command === undefined || command.length === 0) throw new Error('Expected a command after --');
  const output = resolve(root, options.get('--output=') ?? 'tmp/security/egress-capture.pcapng');
  const tmpRoot = resolve(root, 'tmp');
  const relativeOutput = relative(tmpRoot, output);
  if (
    relativeOutput.length === 0 ||
    isAbsolute(relativeOutput) ||
    relativeOutput === '..' ||
    relativeOutput.startsWith(`..${sep}`)
  ) {
    throw new Error('Capture output must be under tmp/.');
  }
  if (!/\.pcap(?:ng)?$/iu.test(output)) {
    throw new Error('Capture output must use a .pcap or .pcapng extension.');
  }
  return Object.freeze({
    command,
    commandArgs: Object.freeze(arguments_.slice(separator + 2)),
    output,
    tmpRoot,
    requestedInterface: options.get('--interface=') || null,
    scope: options.get('--scope=') || 'all',
  });
}

async function main() {
  const statusOnly = process.argv.includes('--status') || !process.argv.includes('--');
  const status = detectCaptureStatus();
  if (statusOnly) {
    console.log(JSON.stringify(status, null, 2));
    process.exitCode = status.toolAvailable ? 0 : 2;
    return;
  }
  if (process.env.TALKING_QUILL_ALLOW_OS_CAPTURE !== '1')
    throw new Error('Set TALKING_QUILL_ALLOW_OS_CAPTURE=1 to explicitly permit OS packet capture.');
  if (!status.toolAvailable || status.tool === null)
    throw new Error(`OS packet capture unavailable: ${status.limitation}`);
  const { command, commandArgs, output, tmpRoot, requestedInterface, scope } =
    parseCaptureInvocation(process.argv.slice(2));
  await prepareCaptureOutput(tmpRoot, output);
  await rm(output, { force: true });

  let commandStatus;
  if (process.platform === 'win32') {
    const etl = `${output}.pktmon.etl`;
    await rm(etl, { force: true });
    try {
      requireSuccess(
        spawnSync('pktmon', ['start', '--capture', '--pkt-size', '0', '--file-name', etl]),
      );
      let stopResult;
      try {
        commandStatus = spawnSync(command, commandArgs, { stdio: 'inherit', shell: false });
      } finally {
        stopResult = spawnSync('pktmon', ['stop'], { stdio: 'inherit' });
      }
      requireSuccess(stopResult);
      requireSuccess(spawnSync('pktmon', ['etl2pcap', etl, '--out', output], { stdio: 'inherit' }));
    } finally {
      await rm(etl, { force: true });
    }
  } else {
    const requested =
      requestedInterface ?? process.env.TALKING_QUILL_MAC_CAPTURE_INTERFACE ?? 'pktap';
    const args = macTcpdumpArguments(output, status.interfaces, requested, scope);
    const capture = spawn(status.tool, args, { stdio: ['ignore', 'ignore', 'pipe'] });
    const exit = exitResult(capture); // installed immediately: no fast-exit listener race
    let captureFailure = null;
    try {
      await waitForTcpdumpReady(capture, exit);
      commandStatus = spawnSync(command, commandArgs, { stdio: 'inherit', shell: false });
    } finally {
      if (capture.exitCode !== null || capture.signalCode !== null) {
        const result = await exit;
        captureFailure = new Error(
          `tcpdump exited before capture shutdown (${String(result.code ?? result.signal)}): ${result.stderr}`,
        );
      } else if (!capture.kill('SIGINT')) {
        captureFailure = new Error('Unable to request tcpdump capture shutdown.');
      } else {
        const result = await exit;
        if (result.code !== 0 && result.signal !== 'SIGINT') {
          captureFailure = new Error(
            `tcpdump failed during capture shutdown (${String(result.code ?? result.signal)}): ${result.stderr}`,
          );
        }
      }
    }
    if (captureFailure !== null) throw captureFailure;
  }
  if (commandStatus.error !== undefined || commandStatus.status !== 0)
    throw (
      commandStatus.error ??
      new Error(`Captured command failed with ${String(commandStatus.status)}`)
    );
  await verifyPcap(output);
  console.log(
    `OS packet capture written to ${output}. Review it with an independent packet analyzer.`,
  );
}

async function prepareCaptureOutput(tmpRoot, output) {
  await mkdir(tmpRoot, { recursive: true });
  await assertCanonicalDirectory(tmpRoot);
  const parent = dirname(output);
  const segments = relative(tmpRoot, parent).split(sep).filter(Boolean);
  let current = tmpRoot;
  for (const segment of segments) {
    current = resolve(current, segment);
    let metadata;
    try {
      metadata = await lstat(current);
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error;
      await mkdir(current);
      metadata = await lstat(current);
    }
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
      throw new Error(`Capture output directory is not a canonical directory: ${current}`);
    }
    await assertCanonicalDirectory(current);
  }
}

async function assertCanonicalDirectory(path) {
  const physical = await realpath(path);
  const comparable = (value) => (process.platform === 'win32' ? value.toLowerCase() : value);
  if (comparable(physical) !== comparable(resolve(path))) {
    throw new Error(`Capture output directory escapes through a filesystem redirect: ${path}`);
  }
}

async function waitForTcpdumpReady(capture, exit) {
  let stderr = '';
  let timer;
  try {
    await Promise.race([
      new Promise((resolveReady, reject) => {
        timer = setTimeout(
          () => reject(new Error(`tcpdump did not report readiness: ${stderr}`)),
          10_000,
        );
        capture.stderr.setEncoding('utf8');
        capture.stderr.on('data', (chunk) => {
          stderr = (stderr + chunk).slice(-65_536);
          if (/listening on /iu.test(stderr)) resolveReady();
        });
      }),
      exit.then((result) => {
        throw new Error(`tcpdump exited before readiness: ${result.stderr}`);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
function exitResult(child) {
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr = (stderr + chunk).slice(-65_536);
  });
  return new Promise((resolveExit, reject) => {
    child.once('error', reject);
    child.once('close', (code, signal) => resolveExit({ code, signal, stderr }));
  });
}
async function verifyPcap(path) {
  validatePcapBytes(await readFile(path));
}
export function validatePcapBytes(bytes) {
  if (bytes.length < 24) throw new Error('Capture output is empty or has a truncated pcap header.');
  const magic = bytes.subarray(0, 4).toString('hex');
  if (['a1b2c3d4', 'd4c3b2a1', 'a1b23c4d', '4d3cb2a1'].includes(magic)) return;
  if (magic !== '0a0d0d0a') throw new Error('Capture output is not pcap/pcapng.');
  if (bytes.length < 28) throw new Error('Capture output has a truncated pcapng section header.');
  const byteOrder = bytes.subarray(8, 12).toString('hex');
  const littleEndian = byteOrder === '4d3c2b1a';
  if (!littleEndian && byteOrder !== '1a2b3c4d')
    throw new Error('Capture output has an invalid pcapng byte-order marker.');
  const blockLength = littleEndian ? bytes.readUInt32LE(4) : bytes.readUInt32BE(4);
  if (blockLength < 28 || blockLength % 4 !== 0 || blockLength > bytes.length) {
    throw new Error('Capture output has an invalid pcapng section length.');
  }
  const majorVersion = littleEndian ? bytes.readUInt16LE(12) : bytes.readUInt16BE(12);
  const minorVersion = littleEndian ? bytes.readUInt16LE(14) : bytes.readUInt16BE(14);
  if (majorVersion !== 1 || minorVersion !== 0) {
    throw new Error(
      `Capture output uses unsupported pcapng version ${String(majorVersion)}.${String(minorVersion)}.`,
    );
  }
  const trailingLength = littleEndian
    ? bytes.readUInt32LE(blockLength - 4)
    : bytes.readUInt32BE(blockLength - 4);
  if (trailingLength !== blockLength)
    throw new Error('Capture output has inconsistent pcapng block lengths.');
}
function commandExists(command, platform) {
  return (
    spawnSync(platform === 'win32' ? 'where' : 'which', [command], { stdio: 'ignore' }).status === 0
  );
}
function requireSuccess(result) {
  if (result.error !== undefined || result.status !== 0)
    throw result.error ?? new Error(`Capture command failed with ${String(result.status)}`);
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  await main();
