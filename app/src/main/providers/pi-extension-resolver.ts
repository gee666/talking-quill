import { createHash } from 'node:crypto';
import { constants as fsConstants, type Dirent, type Stats } from 'node:fs';
import { access, readFile, readdir, realpath, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { posix, resolve, win32 } from 'node:path';
import { parsePiNpmExtensionSource, type ProviderConfig } from '../../shared/schemas/providers';
import { ProviderError } from './errors';
import { environmentValue } from './pi-process-runtime';
import type { PiProviderOptions } from './pi-provider-options';

const PI_EXTENSION_RESOLUTION_TIMEOUT_MS = 5_000;
const PI_EXTENSION_PACKAGE_JSON_MAX_BYTES = 1024 * 1024;
const PI_EXTENSION_PACKAGE_MAX_ENTRIES = 10_000;
export interface ResolvedPiExtensions {
  readonly args: readonly string[];
  readonly canonicalSources: readonly string[];
  readonly cacheKey: readonly (readonly (string | number)[])[];
}
interface ResolvedPiExtension {
  readonly argument: string;
  readonly canonicalSource: string;
  readonly cacheKey: readonly (string | number)[];
}

/** Resolves only explicit opt-ins, with one filesystem budget and stable resource identities. */
export class PiExtensionResolver {
  readonly #environment: NodeJS.ProcessEnv;
  readonly #platform: NodeJS.Platform;
  readonly #workingDirectory: string;
  readonly #canonicalizeExtensionPath: (path: string) => Promise<string>;
  readonly #statExtensionPath: (path: string) => Promise<Stats>;
  readonly #accessExtensionPath: (path: string, mode?: number) => Promise<void>;
  readonly #readExtensionFile: (path: string) => Promise<string>;
  readonly #readExtensionDirectory: (path: string) => Promise<readonly Dirent[]>;
  readonly #extensionResolutionTimeoutMs: number;

  constructor(
    options: PiProviderOptions,
    context: {
      readonly environment: NodeJS.ProcessEnv;
      readonly platform: NodeJS.Platform;
      readonly workingDirectory: string;
    },
  ) {
    this.#environment = context.environment;
    this.#platform = context.platform;
    this.#workingDirectory = context.workingDirectory;
    this.#canonicalizeExtensionPath = options.canonicalizeExtensionPath ?? realpath;
    this.#statExtensionPath = options.statExtensionPath ?? stat;
    this.#accessExtensionPath = options.accessExtensionPath ?? access;
    this.#readExtensionFile = options.readExtensionFile ?? ((path) => readFile(path, 'utf8'));
    this.#readExtensionDirectory =
      options.readExtensionDirectory ?? ((path) => readdir(path, { withFileTypes: true }));
    this.#extensionResolutionTimeoutMs =
      options.extensionResolutionTimeoutMs ?? PI_EXTENSION_RESOLUTION_TIMEOUT_MS;
  }

  async resolve(
    config: Pick<ProviderConfig, 'piExtensionSources'>,
    signal: AbortSignal,
  ): Promise<ResolvedPiExtensions> {
    const args: string[] = [];
    const canonicalSources: string[] = [];
    const cacheKey: (readonly (string | number)[])[] = [];
    const sources = config.piExtensionSources ?? [];
    if (sources.length === 0)
      return Object.freeze({
        args: Object.freeze(args),
        canonicalSources: Object.freeze(canonicalSources),
        cacheKey: Object.freeze(cacheKey),
      });
    const budget = new AbortController();
    const budgetTimer = setTimeout(() => budget.abort(), this.#extensionResolutionTimeoutMs);
    try {
      for (const source of sources) {
        const packageName = parsePiNpmExtensionSource(source);
        const extension =
          packageName === null
            ? await this.#resolveLocalExtension(source, signal, budget.signal)
            : await this.#resolveInstalledNpmExtension(packageName, signal, budget.signal);
        args.push('-e', extension.argument);
        canonicalSources.push(extension.canonicalSource);
        cacheKey.push(extension.cacheKey);
      }
      return Object.freeze({
        args: Object.freeze(args),
        canonicalSources: Object.freeze(canonicalSources),
        cacheKey: Object.freeze(cacheKey),
      });
    } catch (error: unknown) {
      throwIfPiAborted(signal);
      if (error instanceof ProviderError && error.code === 'TIMEOUT') throw error;
      throw new ProviderError('INVALID_CONFIG');
    } finally {
      clearTimeout(budgetTimer);
    }
  }

  async #resolveLocalExtension(
    source: string,
    signal: AbortSignal,
    budgetSignal: AbortSignal,
  ): Promise<ResolvedPiExtension> {
    const canonicalPath = await runPiExtensionFileSystemOperation(
      () => this.#canonicalizeExtensionPath(resolve(this.#workingDirectory, source)),
      signal,
      budgetSignal,
    );
    if (isNetworkRootedPath(canonicalPath)) throw new Error('network path');
    const metadata = await runPiExtensionFileSystemOperation(
      () => this.#statExtensionPath(canonicalPath),
      signal,
      budgetSignal,
    );
    if (!metadata.isFile()) throw new Error('not a file');
    await runPiExtensionFileSystemOperation(
      () => this.#accessExtensionPath(canonicalPath, fsConstants.R_OK),
      signal,
      budgetSignal,
    );
    return Object.freeze({
      argument: source,
      canonicalSource: canonicalPath,
      cacheKey: Object.freeze(['local', canonicalPath, ...fileIdentityParts(metadata)]),
    });
  }

  async #resolveInstalledNpmExtension(
    packageName: string,
    signal: AbortSignal,
    budgetSignal: AbortSignal,
  ): Promise<ResolvedPiExtension> {
    const paths = this.#platform === 'win32' ? win32 : posix;
    const agentDirectory = effectivePiAgentDirectory(
      this.#environment,
      this.#platform,
      this.#workingDirectory,
    );
    const installRoot = paths.resolve(agentDirectory, 'npm');
    const nodeModulesRoot = paths.resolve(installRoot, 'node_modules');
    const requestedPackageRoot = paths.resolve(nodeModulesRoot, packageName);
    if (
      isNetworkRootedPath(agentDirectory) ||
      !isPathWithin(nodeModulesRoot, requestedPackageRoot, this.#platform)
    ) {
      throw new Error('invalid package path');
    }
    const canonicalInstallRoot = await runPiExtensionFileSystemOperation(
      () => this.#canonicalizeExtensionPath(installRoot),
      signal,
      budgetSignal,
    );
    const canonicalPackageRoot = await runPiExtensionFileSystemOperation(
      () => this.#canonicalizeExtensionPath(requestedPackageRoot),
      signal,
      budgetSignal,
    );
    if (
      isNetworkRootedPath(canonicalInstallRoot) ||
      isNetworkRootedPath(canonicalPackageRoot) ||
      !isPathWithin(canonicalInstallRoot, canonicalPackageRoot, this.#platform)
    ) {
      throw new Error('network or escaped package path');
    }
    const packageMetadata = await runPiExtensionFileSystemOperation(
      () => this.#statExtensionPath(canonicalPackageRoot),
      signal,
      budgetSignal,
    );
    if (!packageMetadata.isDirectory()) throw new Error('package root is not a directory');
    await runPiExtensionFileSystemOperation(
      () => this.#accessExtensionPath(canonicalPackageRoot, fsConstants.R_OK),
      signal,
      budgetSignal,
    );

    const packageJsonPath = paths.join(canonicalPackageRoot, 'package.json');
    const canonicalPackageJsonPath = await runPiExtensionFileSystemOperation(
      () => this.#canonicalizeExtensionPath(packageJsonPath),
      signal,
      budgetSignal,
    );
    if (
      isNetworkRootedPath(canonicalPackageJsonPath) ||
      !samePath(paths.dirname(canonicalPackageJsonPath), canonicalPackageRoot, this.#platform)
    ) {
      throw new Error('invalid package manifest path');
    }
    const packageJsonMetadata = await runPiExtensionFileSystemOperation(
      () => this.#statExtensionPath(canonicalPackageJsonPath),
      signal,
      budgetSignal,
    );
    if (
      !packageJsonMetadata.isFile() ||
      packageJsonMetadata.size > PI_EXTENSION_PACKAGE_JSON_MAX_BYTES
    ) {
      throw new Error('invalid package manifest');
    }
    await runPiExtensionFileSystemOperation(
      () => this.#accessExtensionPath(canonicalPackageJsonPath, fsConstants.R_OK),
      signal,
      budgetSignal,
    );
    const manifestText = await runPiExtensionFileSystemOperation(
      () => this.#readExtensionFile(canonicalPackageJsonPath),
      signal,
      budgetSignal,
    );
    const manifest = parseInstalledPiExtensionManifest(manifestText, packageName);
    const packageTreeIdentity = await this.#installedPackageTreeIdentity(
      canonicalPackageRoot,
      signal,
      budgetSignal,
    );
    return Object.freeze({
      argument: canonicalPackageRoot,
      canonicalSource: canonicalPackageRoot,
      cacheKey: Object.freeze([
        'npm',
        packageName,
        manifest.version,
        canonicalPackageRoot,
        ...fileIdentityParts(packageMetadata),
        canonicalPackageJsonPath,
        ...fileIdentityParts(packageJsonMetadata),
        packageTreeIdentity,
      ]),
    });
  }

  async #installedPackageTreeIdentity(
    canonicalPackageRoot: string,
    signal: AbortSignal,
    budgetSignal: AbortSignal,
  ): Promise<string> {
    const paths = this.#platform === 'win32' ? win32 : posix;
    const pending = [canonicalPackageRoot];
    const visited = new Set<string>();
    const hash = createHash('sha256');
    let entriesSeen = 0;
    while (pending.length > 0) {
      const directory = pending.pop();
      if (directory === undefined) break;
      const directoryKey = this.#platform === 'win32' ? directory.toLowerCase() : directory;
      if (visited.has(directoryKey)) continue;
      visited.add(directoryKey);
      const entries = await runPiExtensionFileSystemOperation(
        () => this.#readExtensionDirectory(directory),
        signal,
        budgetSignal,
      );
      for (const entry of [...entries].sort((left, right) => left.name.localeCompare(right.name))) {
        entriesSeen += 1;
        if (entriesSeen > PI_EXTENSION_PACKAGE_MAX_ENTRIES) {
          throw new ProviderError('INVALID_CONFIG');
        }
        const candidate = paths.join(directory, entry.name);
        const canonical = await runPiExtensionFileSystemOperation(
          () => this.#canonicalizeExtensionPath(candidate),
          signal,
          budgetSignal,
        );
        if (
          isNetworkRootedPath(canonical) ||
          !isPathWithin(canonicalPackageRoot, canonical, this.#platform)
        ) {
          throw new Error('package resource escaped its root');
        }
        const metadata = await runPiExtensionFileSystemOperation(
          () => this.#statExtensionPath(canonical),
          signal,
          budgetSignal,
        );
        const kind = metadata.isDirectory() ? 'directory' : metadata.isFile() ? 'file' : 'other';
        hash.update(paths.relative(canonicalPackageRoot, canonical));
        hash.update('\0');
        hash.update(kind);
        hash.update('\0');
        hash.update(fileIdentityParts(metadata).join(':'));
        hash.update('\0');
        if (metadata.isDirectory()) pending.push(canonical);
      }
    }
    return hash.digest('hex');
  }
}

export function sameExtensionIdentity(
  left: ResolvedPiExtensions,
  right: ResolvedPiExtensions,
): boolean {
  return (
    JSON.stringify([left.canonicalSources, left.cacheKey]) ===
    JSON.stringify([right.canonicalSources, right.cacheKey])
  );
}

export function canonicalExtensionArguments(sources: readonly string[]): readonly string[] {
  return Object.freeze(sources.flatMap((source) => ['-e', source]));
}

function effectivePiAgentDirectory(
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  workingDirectory: string,
): string {
  const paths = platform === 'win32' ? win32 : posix;
  const configured = environmentValue(environment, platform, 'PI_CODING_AGENT_DIR');
  const environmentHome = environmentValue(
    environment,
    platform,
    platform === 'win32' ? 'USERPROFILE' : 'HOME',
  );
  const home =
    environmentHome === undefined || environmentHome.length === 0 ? homedir() : environmentHome;
  const source =
    configured === undefined || configured.length === 0
      ? paths.join(home, '.pi', 'agent')
      : expandPiAgentTilde(configured, home, platform);
  return paths.resolve(workingDirectory, source);
}

function expandPiAgentTilde(path: string, home: string, platform: NodeJS.Platform): string {
  if (path === '~') return home;
  if (path.startsWith('~/') || (platform === 'win32' && path.startsWith('~\\')))
    return (platform === 'win32' ? win32 : posix).join(home, path.slice(2));
  return path;
}

function isPathWithin(root: string, candidate: string, platform: NodeJS.Platform): boolean {
  const paths = platform === 'win32' ? win32 : posix;
  const relativePath = paths.relative(root, candidate);
  return (
    relativePath.length > 0 &&
    relativePath !== '..' &&
    !relativePath.startsWith(`..${paths.sep}`) &&
    !paths.isAbsolute(relativePath)
  );
}

function samePath(left: string, right: string, platform: NodeJS.Platform): boolean {
  return platform === 'win32' ? left.toLowerCase() === right.toLowerCase() : left === right;
}

function parseInstalledPiExtensionManifest(
  manifestText: string,
  expectedPackageName: string,
): { readonly version: string } {
  const manifest: unknown = JSON.parse(manifestText);
  if (typeof manifest !== 'object' || manifest === null || Array.isArray(manifest))
    throw new Error('invalid package manifest');
  const name = 'name' in manifest ? manifest.name : undefined;
  const version = 'version' in manifest ? manifest.version : undefined;
  if (
    name !== expectedPackageName ||
    typeof version !== 'string' ||
    version.length === 0 ||
    version.length > 128 ||
    version.trim() !== version
  ) {
    throw new Error('package manifest does not match the configured package');
  }
  return Object.freeze({ version });
}

function fileIdentityParts(metadata: Stats): readonly (string | number)[] {
  return Object.freeze([
    String(metadata.dev),
    String(metadata.ino),
    metadata.size,
    metadata.mtimeMs,
  ]);
}

function isNetworkRootedPath(path: string): boolean {
  return /^(?:[\\/]{2}|[\\/]\?\?[\\/])/u.test(path);
}

function runPiExtensionFileSystemOperation<Result>(
  operation: () => Promise<Result>,
  signal: AbortSignal,
  budgetSignal: AbortSignal,
): Promise<Result> {
  if (signal.aborted) return Promise.reject(new ProviderError('CANCELLED'));
  if (budgetSignal.aborted) return Promise.reject(new ProviderError('TIMEOUT'));
  let pending: Promise<Result>;
  try {
    pending = operation();
  } catch (error: unknown) {
    return Promise.reject(error instanceof Error ? error : new ProviderError('UNAVAILABLE'));
  }
  return new Promise<Result>((resolveOperation, rejectOperation) => {
    let settled = false;
    const settle = (callback: () => void): void => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', cancelled);
      budgetSignal.removeEventListener('abort', timedOut);
      callback();
    };
    const cancelled = (): void => settle(() => rejectOperation(new ProviderError('CANCELLED')));
    const timedOut = (): void => settle(() => rejectOperation(new ProviderError('TIMEOUT')));
    signal.addEventListener('abort', cancelled, { once: true });
    budgetSignal.addEventListener('abort', timedOut, { once: true });
    void pending.then(
      (result) => settle(() => resolveOperation(result)),
      (error: unknown) =>
        settle(() =>
          rejectOperation(error instanceof Error ? error : new ProviderError('UNAVAILABLE')),
        ),
    );
  });
}

function throwIfPiAborted(signal: AbortSignal): void {
  if (signal.aborted) throw new ProviderError('CANCELLED');
}
