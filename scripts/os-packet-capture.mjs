import { spawn, spawnSync } from 'node:child_process';
import { mkdir, readFile, rm } from 'node:fs/promises';
import { dirname, relative, resolve } from 'node:path';
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

export function tcpdumpArguments(platform, output, interfaces, requested, scope) {
  if (platform !== 'darwin') return ['-i', 'any', '-U', '-w', output];
  return ['-i', selectMacCaptureInterface(interfaces, requested, scope), '-U', '-w', output];
}

export function detectCaptureStatus(platform = process.platform) {
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
  const separator = process.argv.indexOf('--');
  const command = process.argv[separator + 1];
  if (command === undefined) throw new Error('Expected a command after --');
  const commandArgs = process.argv.slice(separator + 2);
  const output = resolve(readOption('--output=') ?? 'tmp/security/egress-capture.pcapng');
  const tmpRoot = resolve(repositoryRoot, 'tmp');
  if (relative(tmpRoot, output).startsWith('..'))
    throw new Error('Capture output must be under tmp/.');
  await mkdir(dirname(output), { recursive: true });
  await rm(output, { force: true });

  let commandStatus;
  if (process.platform === 'win32') {
    const etl = output.replace(/\.pcapng$/iu, '.etl');
    await rm(etl, { force: true });
    requireSuccess(
      spawnSync('pktmon', ['start', '--capture', '--pkt-size', '0', '--file-name', etl]),
    );
    try {
      commandStatus = spawnSync(command, commandArgs, { stdio: 'inherit', shell: false });
    } finally {
      spawnSync('pktmon', ['stop'], { stdio: 'inherit' });
    }
    requireSuccess(spawnSync('pktmon', ['etl2pcap', etl, '--out', output], { stdio: 'inherit' }));
  } else {
    const requested =
      readOption('--interface=') ?? process.env.TALKING_QUILL_MAC_CAPTURE_INTERFACE ?? 'pktap';
    const scope = readOption('--scope=') ?? 'all';
    const args = tcpdumpArguments(process.platform, output, status.interfaces, requested, scope);
    const capture = spawn(status.tool, args, { stdio: ['ignore', 'ignore', 'pipe'] });
    const exit = exitResult(capture); // installed immediately: no fast-exit listener race
    let captureFailure = null;
    try {
      await waitForTcpdumpReady(capture, exit);
      commandStatus = spawnSync(command, commandArgs, { stdio: 'inherit', shell: false });
    } finally {
      if (capture.exitCode === null && capture.signalCode === null) capture.kill('SIGINT');
      const result = await exit;
      if (result.code !== 0 && result.code !== null)
        captureFailure = new Error(`tcpdump failed with ${String(result.code)}: ${result.stderr}`);
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
  const bytes = await readFile(path);
  if (bytes.length < 24) throw new Error('Capture output is empty or has a truncated pcap header.');
  const magic = bytes.subarray(0, 4).toString('hex');
  if (!['a1b2c3d4', 'd4c3b2a1', 'a1b23c4d', '4d3cb2a1', '0a0d0d0a'].includes(magic))
    throw new Error('Capture output is not pcap/pcapng.');
}
function commandExists(command, platform) {
  return (
    spawnSync(platform === 'win32' ? 'where' : 'which', [command], { stdio: 'ignore' }).status === 0
  );
}
function readOption(prefix) {
  return process.argv.find((argument) => argument.startsWith(prefix))?.slice(prefix.length) ?? null;
}
function requireSuccess(result) {
  if (result.error !== undefined || result.status !== 0)
    throw result.error ?? new Error(`Capture command failed with ${String(result.status)}`);
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url))
  await main();
