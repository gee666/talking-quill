import { execFile, spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { lstat, mkdir, readFile, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';
import type { HelperClient } from '../helper/helper-client';
import {
  assertTransition,
  exactZip,
  readPolicy,
  sha256File,
  validateMacosReplacementIdentity,
  type DownloadedApplicationUpdate,
} from './macos-update-validation';
import { handoffFinalizer } from './macos-finalizer-supervision';

export {
  validateMacosReplacementIdentity,
  type DownloadedApplicationUpdate,
} from './macos-update-validation';
export { supervisePreReadyStatus, superviseUninstallStatus } from './macos-finalizer-supervision';
export {
  awaitAuthenticatedRollbackOrTerminate,
  boundedTerminate,
  waitForTerminalExit,
} from './macos-finalizer-status';

export type MacosFinalizerChild = ReturnType<typeof spawn>;

const execFileAsync = promisify(execFile);

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

export class MacosMaintenancePostponedError extends Error {
  constructor() {
    super('Release all Talking Quill shortcut keys and try the update again.');
    this.name = 'MacosMaintenancePostponedError';
  }
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

  #handoffFinalizer(
    arguments_: readonly string[],
    ownerHandoff: string,
    transaction: string,
    uninstallCommit?: () => Promise<void>,
  ): Promise<void> {
    return handoffFinalizer(
      () =>
        spawn(this.#helperExecutable, ['--macos-owner-finalize', ...arguments_], {
          detached: true,
          stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe', 'pipe'],
        }),
      ownerHandoff,
      transaction,
      uninstallCommit,
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
}

function requireOwnerHandoff(
  maintenance: { readonly ownerHandoff: string } | null | undefined,
): string {
  if (maintenance === null || maintenance === undefined)
    throw new Error('Owner maintenance handoff is unavailable');
  return maintenance.ownerHandoff;
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
