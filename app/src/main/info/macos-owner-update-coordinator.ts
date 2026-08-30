import { execFile, spawn } from 'node:child_process';
import { createHash, createHmac, randomBytes } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, mkdir, readFile, rm } from 'node:fs/promises';
import { basename, isAbsolute, join } from 'node:path';
import { promisify } from 'node:util';
import type { HelperClient } from '../helper/helper-client';

const execFileAsync = promisify(execFile);
const HEX_32 = /^[0-9a-f]{64}$/u;
const POLICY_BYTES = 328;
// Native recovery can spend 30s acquiring exclusion plus 90s restoring and probing.
const NATIVE_RECOVERY_SUPERVISION_MS = 150_000;
// Exceeds the native finalizer's single 120-second absolute uninstall budget.
const UNINSTALL_TERMINAL_SUPERVISION_MS = 150_000;

export interface DownloadedApplicationUpdate {
  readonly files: readonly string[];
  readonly identity?: {
    readonly schemaVersion: 1;
    readonly version: string;
    readonly platform: 'win' | 'mac';
    readonly architecture: 'x64' | 'arm64';
    readonly ownerMode: 'local-unsigned-enabled';
    readonly packageMode: 'update';
    readonly sourceCommit: string;
    readonly sourceTree: string;
    readonly releaseBuildDigest: string;
    readonly packageLayoutDigest: string;
    readonly packageSha256: string;
    readonly channel: string;
    readonly authorization?: {
      readonly scheme: 'p256-sha256-v1';
      readonly verificationKeySha256: string;
      readonly signature: string;
    };
    readonly roles: readonly {
      readonly role: 'gateway' | 'owner' | 'authority' | 'maintenance' | 'electron';
      readonly path: string;
      readonly sha256: string;
      readonly suppressionCapable: boolean;
    }[];
    readonly outerIdentity?: {
      readonly mode: 'certificate' | 'adhoc';
      readonly leafCertificateSha256: string | null;
      readonly identifier: string;
      readonly teamIdentifier: string | null;
      readonly designatedRequirement: string;
      readonly designatedRequirementSha256: string;
    };
    readonly predecessor: {
      readonly platform: 'win' | 'mac';
      readonly architecture: 'x64' | 'arm64';
      readonly version: string;
      readonly releaseBuildDigest: string;
      readonly gatewaySha256: string;
      readonly ownerSha256: string;
    } | null;
    readonly transactionBinding: 'source-target-package-sha256-v1';
  };
}

export class MacosMaintenancePostponedError extends Error {
  constructor() {
    super('Release all Talking Quill shortcut keys and try the update again.');
    this.name = 'MacosMaintenancePostponedError';
  }
}

export async function validateInstalledMacosMaintenanceCapability(options: {
  readonly helperExecutable: string;
  readonly installedApp: string;
  readonly validateHelper?: (executable: string) => Promise<void>;
}): Promise<void> {
  const marker = join(options.installedApp, 'Contents', 'Resources', 'keyboard-owner-installed-v1');
  const policy = join(options.installedApp, 'Contents', 'Resources', 'keyboard-owner-r5m.json');
  const required = [
    options.helperExecutable,
    marker,
    policy,
    join(options.installedApp, 'Contents', 'MacOS', 'talking-quill-macos-service-bridge'),
  ];
  for (const path of required) {
    const metadata = await lstat(path).catch(() => null);
    if (
      metadata === null ||
      !metadata.isFile() ||
      metadata.isSymbolicLink() ||
      metadata.nlink !== 1
    ) {
      throw new Error('The installed macOS owner maintenance capability is unavailable');
    }
  }
  if ((await readFile(marker, 'utf8')) !== 'talking-quill-keyboard-owner-v1\n') {
    throw new Error('The installed macOS owner maintenance marker is invalid');
  }
  await readPolicy(policy);
  const validateHelper =
    options.validateHelper ??
    (async (executable: string) => {
      await execFileAsync(executable, ['--macos-owner-validate-install'], {
        timeout: 5_000,
        killSignal: 'SIGKILL',
        maxBuffer: 64 * 1024,
      });
    });
  await validateHelper(options.helperExecutable).catch((error: unknown) => {
    throw new Error('The installed macOS owner maintenance capability is invalid', {
      cause: error,
    });
  });
}

/**
 * Bridges electron-updater to the installed owner transaction. Candidate
 * policy is checked before capture is sealed; a detached native coordinator
 * then owns LoginItem exclusion until the replacement proves it can launch.
 */
export class MacosOwnerUpdateCoordinator {
  readonly #helper: () => HelperClient | null;
  readonly #helperExecutable: string;
  readonly #installedApp: string;
  readonly #temporaryRoot: string;

  constructor(options: {
    readonly helper: () => HelperClient | null;
    readonly helperExecutable: string;
    readonly installedApp: string;
    readonly temporaryRoot: string;
  }) {
    this.#helper = options.helper;
    this.#helperExecutable = options.helperExecutable;
    this.#installedApp = options.installedApp;
    this.#temporaryRoot = options.temporaryRoot;
  }

  async prepareUpdate(download: DownloadedApplicationUpdate): Promise<void> {
    await this.#assertInstalledCapabilityAvailable();
    return this.#prepareReplacement('update', download);
  }

  async prepareRollback(download: DownloadedApplicationUpdate): Promise<void> {
    await this.#assertInstalledCapabilityAvailable();
    return this.#prepareReplacement('rollback', download);
  }

  async prepareUninstall(removeInstalledApp: () => Promise<void>): Promise<void> {
    await this.#assertInstalledCapabilityAvailable();
    const transaction = randomBytes(32).toString('hex');
    const staging = join(this.#temporaryRoot, `talking-quill-uninstall-${transaction}`);
    await mkdir(staging, { recursive: true, mode: 0o700 });
    const source = await readPolicy(
      join(this.#installedApp, 'Contents', 'Resources', 'keyboard-owner-r5m.json'),
    );
    await this.#assertInstalledCapabilityAvailable();
    const helper = this.#helper();
    if (helper === null) throw new Error('The native helper is unavailable');
    let maintenance: Awaited<ReturnType<HelperClient['prepareOwnerMaintenance']>> | null = null;
    try {
      maintenance = await helper.prepareOwnerMaintenance(
        {
          operation: 'uninstall',
          transactionId: transaction,
          sourceBuildId: source.releaseBuildDigest,
        },
        15_000,
      );
    } catch {
      throw new MacosMaintenancePostponedError();
    }
    const ownerHandoff = requireOwnerHandoff(maintenance);
    const ready = join(staging, 'coordinator.ready');
    const complete = join(staging, 'coordinator.complete');
    await this.#handoffFinalizer(
      [
        'uninstall',
        transaction,
        source.releaseBuildDigest,
        '-',
        '-',
        '-',
        '-',
        this.#installedApp,
        ready,
        complete,
        String(process.pid),
      ],
      ownerHandoff,
      transaction,
      removeInstalledApp,
    );
  }

  async #assertInstalledCapabilityAvailable(): Promise<void> {
    await validateInstalledMacosMaintenanceCapability({
      helperExecutable: this.#helperExecutable,
      installedApp: this.#installedApp,
    });
  }

  async #prepareReplacement(
    operation: 'update' | 'rollback',
    download: DownloadedApplicationUpdate,
  ): Promise<void> {
    const archive = exactZip(download.files);
    const archiveMetadata = await lstat(archive);
    if (!archiveMetadata.isFile() || archiveMetadata.isSymbolicLink() || archiveMetadata.size === 0)
      throw new Error('The rollback/update ZIP is not a non-empty regular file');
    const transaction = randomBytes(32).toString('hex');
    const staging = join(this.#temporaryRoot, `talking-quill-update-${transaction}`);
    await rm(staging, { recursive: true, force: true });
    await mkdir(staging, { recursive: true, mode: 0o700 });
    await execFileAsync('/usr/bin/ditto', ['-x', '-k', archive, staging], {
      timeout: 30_000,
      killSignal: 'SIGKILL',
      maxBuffer: 64 * 1024,
    });
    const candidate = join(staging, 'Talking Quill.app');
    const [source, target, packageMetadata, archiveSha256] = await Promise.all([
      readPolicy(join(this.#installedApp, 'Contents', 'Resources', 'keyboard-owner-r5m.json')),
      readPolicy(join(candidate, 'Contents', 'Resources', 'keyboard-owner-r5m.json')),
      readFile(
        join(candidate, 'Contents', 'Resources', 'keyboard-owner-release-v1.json'),
        'utf8',
      ).then((value) => JSON.parse(value) as unknown),
      sha256File(archive),
    ]);
    assertTransition(source, target);
    const identity = validateMacosReplacementIdentity({
      identity: download.identity,
      archiveSha256,
      packageMetadata,
      source,
      target,
      expectedArchitecture:
        process.arch === 'x64' || process.arch === 'arm64' ? process.arch : null,
    });
    await this.#assertInstalledCapabilityAvailable();
    const helper = this.#helper();
    if (helper === null) throw new Error('The native helper is unavailable');
    let maintenance: Awaited<ReturnType<HelperClient['prepareOwnerMaintenance']>> | null = null;
    try {
      maintenance = await helper.prepareOwnerMaintenance(
        {
          operation,
          transactionId: transaction,
          sourceBuildId: source.releaseBuildDigest,
          targetBuildId: target.releaseBuildDigest,
          targetOwnerSha256: target.owner.executableSha256,
        },
        15_000,
      );
    } catch {
      await rm(staging, { recursive: true, force: true });
      throw new MacosMaintenancePostponedError();
    }
    const ownerHandoff = requireOwnerHandoff(maintenance);
    const ready = join(staging, 'coordinator.ready');
    const complete = join(staging, 'coordinator.complete');
    await this.#handoffFinalizer(
      [
        operation,
        transaction,
        source.releaseBuildDigest,
        target.releaseBuildDigest,
        target.owner.executableSha256,
        identity.architecture,
        candidate,
        this.#installedApp,
        ready,
        complete,
        String(process.pid),
      ],
      ownerHandoff,
      transaction,
    );
    await waitForMarker(ready, 35_000);
  }

  async #handoffFinalizer(
    arguments_: readonly string[],
    ownerHandoff: string,
    transaction: string,
    uninstallCommit?: () => Promise<void>,
  ): Promise<void> {
    if (!HEX_32.test(ownerHandoff) || !HEX_32.test(transaction))
      throw new Error('The owner maintenance handoff is invalid');
    let child: ReturnType<typeof spawn>;
    try {
      child = spawn(this.#helperExecutable, ['--macos-owner-finalize', ...arguments_], {
        detached: true,
        stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe', 'pipe'],
      });
    } catch (error: unknown) {
      throw new Error('The native maintenance coordinator failed to spawn', { cause: error });
    }
    const extraPipes = child.stdio as (
      NodeJS.ReadableStream | NodeJS.WritableStream | null | undefined
    )[];
    const handoffPipe = extraPipes[3];
    const statusPipe = extraPipes[4];
    const cancellationPipe = extraPipes[5];
    if (
      handoffPipe === null ||
      handoffPipe === undefined ||
      !('end' in handoffPipe) ||
      statusPipe === null ||
      statusPipe === undefined ||
      !('read' in statusPipe) ||
      cancellationPipe === null ||
      cancellationPipe === undefined ||
      !('end' in cancellationPipe)
    ) {
      if (cancellationPipe !== null && cancellationPipe !== undefined && 'end' in cancellationPipe)
        cancellationPipe.end();
      await boundedTerminate(child, 5_000);
      throw new Error('The native maintenance supervision pipes are unavailable');
    }
    handoffPipe.end(Buffer.from(ownerHandoff, 'hex'));
    if (uninstallCommit !== undefined) {
      await superviseUninstallStatus(
        child,
        statusPipe,
        cancellationPipe,
        ownerHandoff,
        transaction,
        uninstallCommit,
        35_000,
        UNINSTALL_TERMINAL_SUPERVISION_MS,
      );
      return;
    }
    const status = await supervisePreReadyStatus(
      child,
      statusPipe,
      cancellationPipe,
      ownerHandoff,
      transaction,
      35_000,
    );
    if (status !== 'ready') {
      cancellationPipe.end();
      await boundedTerminate(child, 10_000);
      throw new Error('The native maintenance coordinator rolled back before handoff');
    }
    const statusControl = statusPipe as NodeJS.ReadableStream & { destroy?: () => void };
    statusControl.destroy?.();
    const unrefCancellation = cancellationPipe as NodeJS.WritableStream & {
      unref?: () => void;
    };
    unrefCancellation.unref?.();
    child.unref();
  }
}

export async function supervisePreReadyStatus(
  child: ReturnType<typeof spawn>,
  statusPipe: NodeJS.ReadableStream,
  cancellationPipe: NodeJS.WritableStream,
  ownerHandoff: string,
  transaction: string,
  timeoutMs: number,
  recoveryTimeoutMs = NATIVE_RECOVERY_SUPERVISION_MS,
): Promise<'ready' | 'error'> {
  try {
    const status = await waitForAuthenticatedStatus(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      cancellationPipe,
      timeoutMs,
      ['ready', 'error'],
    );
    return status === 'ready' ? 'ready' : 'error';
  } catch (error: unknown) {
    cancellationPipe.end();
    await awaitAuthenticatedRollbackOrTerminate(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      recoveryTimeoutMs,
    );
    throw error;
  }
}

export async function superviseUninstallStatus(
  child: ReturnType<typeof spawn>,
  statusPipe: NodeJS.ReadableStream,
  cancellationPipe: NodeJS.WritableStream,
  ownerHandoff: string,
  transaction: string,
  removeInstalledApp: () => Promise<void>,
  handoffTimeoutMs: number,
  terminalTimeoutMs: number,
  exitTimeoutMs = 5_000,
  recoveryTimeoutMs = NATIVE_RECOVERY_SUPERVISION_MS,
): Promise<'complete' | 'cleanup_pending'> {
  try {
    const handoff = await waitForAuthenticatedStatus(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      cancellationPipe,
      handoffTimeoutMs,
      ['uninstall_ready', 'error'],
    );
    if (handoff === 'error')
      throw new Error('The native uninstall coordinator failed before removal handoff');

    await removeInstalledApp();
    const terminal = await waitForAuthenticatedStatus(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      cancellationPipe,
      terminalTimeoutMs,
      ['complete', 'cleanup_pending', 'error'],
    );
    await waitForTerminalExit(child, exitTimeoutMs);
    endWritableOnce(cancellationPipe);
    if (terminal === 'error') throw new Error('The native uninstall coordinator failed closed');
    if (terminal === 'complete') return 'complete';
    return 'cleanup_pending';
  } catch (error: unknown) {
    endWritableOnce(cancellationPipe);
    await awaitAuthenticatedRollbackOrTerminate(
      child,
      statusPipe,
      ownerHandoff,
      transaction,
      recoveryTimeoutMs,
    );
    throw error;
  }
}

function endWritableOnce(stream: NodeJS.WritableStream): void {
  const writable = stream as NodeJS.WritableStream & { readonly writableEnded?: boolean };
  if (writable.writableEnded !== true) writable.end();
}

export async function waitForTerminalExit(
  child: ReturnType<typeof spawn>,
  timeoutMs: number,
): Promise<void> {
  const exited = await waitForChildExit(child, timeoutMs);
  if (!exited) {
    await boundedTerminate(child, 2_000);
    throw new Error('The native uninstall coordinator did not exit after terminal status');
  }
}

type FinalizerStatus = 'ready' | 'uninstall_ready' | 'complete' | 'cleanup_pending' | 'error';

async function waitForAuthenticatedStatus(
  child: ReturnType<typeof spawn>,
  stream: NodeJS.ReadableStream,
  handoff: string,
  transaction: string,
  cancellation: NodeJS.WritableStream,
  timeoutMs: number,
  acceptedStates: readonly FinalizerStatus[],
): Promise<FinalizerStatus> {
  return new Promise((resolveStatus, reject) => {
    let settled = false;
    let body = '';
    let recordedChildFailure: Error | null = null;
    const finish = (error: Error | null, state?: FinalizerStatus): boolean => {
      if (settled) return true;
      settled = true;
      clearTimeout(timer);
      child.off('error', onError);
      child.off('exit', onExit);
      stream.removeListener('data', onData);
      stream.removeListener('error', onStreamError);
      stream.removeListener('end', onStreamEnd);
      stream.removeListener('close', onStreamClose);
      stream.pause();
      if (error !== null) reject(error);
      else if (state !== undefined) resolveStatus(state);
      return true;
    };
    const onError = () => {
      recordedChildFailure = new Error('The native maintenance coordinator failed to start');
    };
    const onExit = (code: number | null) => {
      recordedChildFailure = new Error(
        `The native maintenance coordinator exited before authenticated status (${String(code)})`,
      );
    };
    const processStatusBytes = (chunk: string | Buffer): boolean => {
      if (settled) return true;
      body += typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      if (Buffer.byteLength(body) > 512)
        return finish(new Error('The native maintenance status exceeded its bound'));
      const newline = body.indexOf('\n');
      if (newline < 0) return false;
      if (newline !== body.length - 1)
        return finish(new Error('The native maintenance status had trailing data'));
      try {
        const wire = JSON.parse(body) as Record<string, unknown>;
        const state = wire.state;
        if (
          wire.version !== 1 ||
          wire.transaction !== transaction ||
          typeof state !== 'string' ||
          !acceptedStates.includes(state as FinalizerStatus) ||
          wire.mac !== finalizerStatusMac(handoff, transaction, state)
        )
          throw new Error('invalid');
        return finish(null, state as FinalizerStatus);
      } catch {
        return finish(new Error('The native maintenance status was not authenticated'));
      }
    };
    const drainBufferedStatus = (): boolean => {
      const drained: Buffer[] = [];
      let buffered = readBufferedStatusChunk(stream);
      while (buffered !== null) {
        drained.push(Buffer.from(buffered));
        buffered = readBufferedStatusChunk(stream);
      }
      return drained.length > 0 && processStatusBytes(Buffer.concat(drained));
    };
    const finishAtStreamBoundary = (boundary: 'end' | 'close') => {
      if (settled) return;
      try {
        if (drainBufferedStatus()) return;
      } catch {
        finish(new Error('The native maintenance status pipe failed'));
        return;
      }
      finish(
        recordedChildFailure ??
          new Error(`The native maintenance status pipe reached ${boundary} without status`),
      );
    };
    const onData = (chunk: string | Buffer) => processStatusBytes(chunk);
    const onStreamError = () => {
      if (settled) return;
      try {
        if (drainBufferedStatus()) return;
      } catch {
        // The original stream failure remains the authoritative error.
      }
      finish(new Error('The native maintenance status pipe failed'));
    };
    const onStreamEnd = () => finishAtStreamBoundary('end');
    const onStreamClose = () => finishAtStreamBoundary('close');
    // Deadline cancellation is cooperative: closing fd 5 instructs the native
    // coordinator to finish rollback and re-registration.
    const timer = setTimeout(() => {
      cancellation.end();
      finish(new Error('The native maintenance coordinator exceeded its pre-ready deadline'));
    }, timeoutMs);
    // An exit may already be recorded while its authenticated terminal line is
    // buffered in the pipe. Preserve that ordering evidence before listeners.
    if (childHasExited(child)) {
      recordedChildFailure = new Error(
        'The native maintenance coordinator exited before authenticated status',
      );
    }
    stream.pause();
    child.once('error', onError);
    child.once('exit', onExit);
    stream.on('data', onData);
    stream.once('error', onStreamError);
    stream.once('end', onStreamEnd);
    stream.once('close', onStreamClose);
    // Drain every byte currently buffered while paused, then parse the complete
    // aggregate so a valid line cannot hide buffered trailing/malformed data.
    try {
      if (drainBufferedStatus()) return;
    } catch {
      finish(new Error('The native maintenance status pipe failed'));
      return;
    }
    // Child exit is only recorded. The pipe may still contain or later deliver
    // the required authenticated line before its writer reaches EOF.
    if (childHasExited(child) && recordedChildFailure === null) {
      recordedChildFailure = new Error(
        'The native maintenance coordinator exited before authenticated status',
      );
    }
    stream.resume();
    // Readers remain installed until authenticated status, exit, stream error,
    // or deadline; every path converges on finish() exactly once.
  });
}

function readBufferedStatusChunk(stream: NodeJS.ReadableStream): string | Buffer | null {
  const value: unknown = stream.read();
  if (value === null || typeof value === 'string' || Buffer.isBuffer(value)) return value;
  throw new Error('The native maintenance status pipe returned invalid buffered data');
}

async function sha256File(path: string): Promise<string> {
  const hash = createHash('sha256');
  await new Promise<void>((resolveHash, reject) => {
    const stream = createReadStream(path);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.once('end', resolveHash);
    stream.once('error', reject);
  });
  return hash.digest('hex');
}

function finalizerStatusMac(handoff: string, transaction: string, state: string): string {
  return createHmac('sha256', Buffer.from(handoff, 'hex'))
    .update('talking-quill/macos-finalizer-status/v1\0')
    .update(transaction)
    .update(state)
    .digest('hex');
}

export async function awaitAuthenticatedRollbackOrTerminate(
  child: ReturnType<typeof spawn>,
  stream: NodeJS.ReadableStream,
  handoff: string,
  transaction: string,
  timeoutMs: number,
): Promise<void> {
  const authenticated = new Promise<boolean>((resolve) => {
    let body = '';
    let settled = false;
    const finish = (value: boolean) => {
      if (settled) return;
      settled = true;
      stream.removeListener('data', onData);
      child.removeListener('exit', onExit);
      resolve(value);
    };
    const onExit = () => finish(false);
    const onData = (chunk: string | Buffer) => {
      body += typeof chunk === 'string' ? chunk : chunk.toString('utf8');
      if (Buffer.byteLength(body) > 512) return finish(false);
      const newline = body.indexOf('\n');
      if (newline < 0) return;
      try {
        const wire = JSON.parse(body.slice(0, newline + 1)) as Record<string, unknown>;
        finish(
          wire.version === 1 &&
            wire.transaction === transaction &&
            wire.state === 'error' &&
            wire.mac === finalizerStatusMac(handoff, transaction, 'error'),
        );
      } catch {
        finish(false);
      }
    };
    if (childHasExited(child)) {
      finish(false);
      return;
    }
    stream.on('data', onData);
    child.once('exit', onExit);
    // Exit can occur between the first state check and listener installation;
    // Node records it on the ChildProcess even when the event already fired.
    if (childHasExited(child)) finish(false);
  });
  const observed = await Promise.race([
    authenticated,
    new Promise<false>((resolve) => setTimeout(() => resolve(false), timeoutMs)),
  ]);
  if (!observed || (child.exitCode === null && child.signalCode === null)) {
    await boundedTerminate(child, 2_000);
  }
}

function childHasExited(child: ReturnType<typeof spawn>): boolean {
  return child.exitCode !== null || child.signalCode !== null;
}

async function waitForChildExit(
  child: ReturnType<typeof spawn>,
  timeoutMs: number,
): Promise<boolean> {
  if (childHasExited(child)) return true;
  return new Promise((resolve) => {
    let settled = false;
    const finish = (exited: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.removeListener('exit', onExit);
      resolve(exited);
    };
    const onExit = () => finish(true);
    const timer = setTimeout(() => finish(false), timeoutMs);
    child.once('exit', onExit);
    // Close the event-before-listener and event-during-listener-installation races.
    if (childHasExited(child)) finish(true);
  });
}

export async function boundedTerminate(
  child: ReturnType<typeof spawn>,
  timeoutMs: number,
): Promise<void> {
  if (await waitForChildExit(child, timeoutMs)) return;
  if (childHasExited(child)) return;
  child.kill('SIGTERM');
  if (await waitForChildExit(child, 2_000)) return;
  if (childHasExited(child)) return;
  child.kill('SIGKILL');
  await waitForChildExit(child, 2_000);
}

function requireOwnerHandoff(
  maintenance: { readonly ownerHandoff: string } | null | undefined,
): string {
  if (maintenance === null || maintenance === undefined)
    throw new Error('Owner maintenance handoff is unavailable');
  return maintenance.ownerHandoff;
}

function exactZip(files: readonly string[]): string {
  const zips = files.filter((path) => path.toLowerCase().endsWith('.zip'));
  const candidate = zips[0];
  if (
    zips.length !== 1 ||
    candidate === undefined ||
    !isAbsolute(candidate) ||
    basename(candidate).length === 0
  )
    throw new Error('The macOS update did not provide one exact ZIP candidate');
  return candidate;
}

interface PolicyWire {
  readonly releaseBuildDigest: string;
  readonly gateway: { readonly executableSha256: string };
  readonly owner: { readonly executableSha256: string };
  readonly bridge: { readonly executableSha256: string };
  readonly gatewayReleasePolicy: string;
}

export function validateMacosReplacementIdentity(input: {
  readonly identity: DownloadedApplicationUpdate['identity'];
  readonly archiveSha256: string;
  readonly packageMetadata: unknown;
  readonly source: PolicyWire;
  readonly target: PolicyWire;
  readonly expectedArchitecture: 'x64' | 'arm64' | null;
}): NonNullable<DownloadedApplicationUpdate['identity']> {
  const identity = input.identity;
  if (identity === undefined) {
    throw new Error('Exact macOS updater identity is required');
  }
  const predecessor = identity.predecessor;
  const expectedRoles = [
    {
      role: 'gateway',
      path: 'Talking Quill.app/Contents/Resources/helper/talking-quill-helper',
      sha256: input.target.gateway.executableSha256,
      suppressionCapable: false,
    },
    {
      role: 'owner',
      path: 'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
      sha256: input.target.owner.executableSha256,
      suppressionCapable: true,
    },
    {
      role: 'authority',
      path: 'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
      sha256: input.target.bridge.executableSha256,
      suppressionCapable: false,
    },
  ];
  const metadata = input.packageMetadata as Record<string, unknown> | null;
  const metadataUpdate = metadata?.update as Record<string, unknown> | undefined;
  if (
    input.expectedArchitecture === null ||
    identity.platform !== 'mac' ||
    identity.architecture !== input.expectedArchitecture ||
    identity.packageSha256 !== input.archiveSha256 ||
    identity.releaseBuildDigest !== input.target.releaseBuildDigest ||
    identity.roles.length !== expectedRoles.length ||
    identity.roles.some(
      (role, index) => JSON.stringify(role) !== JSON.stringify(expectedRoles[index]),
    ) ||
    predecessor?.platform !== 'mac' ||
    predecessor.architecture !== input.expectedArchitecture ||
    predecessor.releaseBuildDigest !== input.source.releaseBuildDigest ||
    predecessor.gatewaySha256 !== input.source.gateway.executableSha256 ||
    predecessor.ownerSha256 !== input.source.owner.executableSha256 ||
    metadata?.schemaVersion !== 1 ||
    metadata.kind !== 'talking-quill-local-owner-release' ||
    metadata.version !== identity.version ||
    metadata.platform !== identity.platform ||
    metadata.architecture !== identity.architecture ||
    metadata.ownerMode !== identity.ownerMode ||
    metadata.sourceCommit !== identity.sourceCommit ||
    metadata.sourceTree !== identity.sourceTree ||
    metadata.releaseBuildDigest !== identity.releaseBuildDigest ||
    metadata.packageLayoutDigest !== identity.packageLayoutDigest ||
    JSON.stringify(metadata.roles) !== JSON.stringify(identity.roles) ||
    JSON.stringify(metadata.predecessor) !== JSON.stringify(identity.predecessor) ||
    JSON.stringify(metadata.outerIdentity) !== JSON.stringify(identity.outerIdentity) ||
    identity.outerIdentity?.mode !== 'certificate' ||
    metadataUpdate?.channel !== identity.channel ||
    metadataUpdate.payload !== 'zip' ||
    metadataUpdate.transactionBinding !== identity.transactionBinding ||
    metadataUpdate.maintenanceInstaller !== 'macos-owner-finalizer'
  ) {
    throw new Error('Downloaded updater identity does not bind the complete macOS artifact');
  }
  return identity;
}

async function readPolicy(path: string): Promise<PolicyWire> {
  const wire = JSON.parse(await readFile(path, 'utf8')) as PolicyWire;
  if (
    !HEX_32.test(wire.releaseBuildDigest) ||
    !HEX_32.test(wire.gateway.executableSha256) ||
    !HEX_32.test(wire.owner.executableSha256) ||
    !HEX_32.test(wire.bridge.executableSha256)
  )
    throw new Error('The installed owner policy is malformed');
  return wire;
}

function assertTransition(source: PolicyWire, target: PolicyWire): void {
  const bytes = Buffer.from(target.gatewayReleasePolicy, 'base64url');
  if (bytes.length !== POLICY_BYTES || bytes.subarray(0, 8).toString('ascii') !== 'TQKOPOL1')
    throw new Error('The target release policy is malformed');
  const hex = (start: number) => bytes.subarray(start, start + 32).toString('hex');
  if (
    hex(16) !== target.releaseBuildDigest ||
    hex(48) !== target.gateway.executableSha256 ||
    hex(80) !== target.owner.executableSha256 ||
    bytes[13] !== 1 ||
    hex(224) !== source.releaseBuildDigest ||
    hex(256) !== source.gateway.executableSha256 ||
    hex(288) !== source.owner.executableSha256
  ) {
    throw new Error('The update does not enroll the exact installed predecessor');
  }
}

async function waitForMarker(path: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const marker = await readFile(path, 'utf8');
      if (marker === 'ready\n') return;
      throw new Error('The native maintenance marker is invalid');
    } catch (error: unknown) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error('The native maintenance coordinator did not become ready');
}
