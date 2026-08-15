import {
  ModelInfoSchema,
  ProviderCompletionRequestSchema,
  ProviderConfigSchema,
  parsePiNpmExtensionSource,
  type Destination,
  type ModelInfo,
  type ProviderCompletionRequest,
  type ProviderConfig,
  type ProviderValidationResult,
  type VisionCapability,
} from '../../shared/schemas/providers';
import { createHash } from 'node:crypto';
import { constants as fsConstants, type Dirent, type Stats } from 'node:fs';
import { access, readFile, readdir, realpath, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { posix, resolve, win32 } from 'node:path';
import type {
  PreparedProviderCompletion,
  ProviderInvocationConfig,
  SmartProvider,
} from './contracts';
import type { EgressObserver } from '../security/egress-audit';
import { ProviderError } from './errors';
import { MAX_NATIVE_OUTPUT_CHARACTERS } from './native-common';
import { resolveCanonicalPiCli } from './pi-discovery';
import { identityKey, revalidatePiCliIdentity, type PiCliIdentity } from './pi-executable';
import {
  assertRpcCompatibility,
  prewarmPiRpcOperation,
  type PiRpcOperation,
  type PiRpcTimingStage,
  type TerminatePiRpcTree,
} from './pi-rpc-operation';
import { PiOperationScheduler, type PiSpeculativeOperationPermit } from './pi-operation-scheduler';
import { environmentValue, runPiInvocation, type SpawnPi } from './pi-process-runtime';
export { resolveCanonicalPiCli } from './pi-discovery';
export type { PiCliIdentity } from './pi-executable';
export { terminateProcessTree } from './pi-process-runtime';
export type { PiTreeTerminationOptions, SpawnPi } from './pi-process-runtime';

const MODEL_CACHE_TTL_MS = 5 * 60_000;
const MAX_STDOUT_BYTES = 2 * 1024 * 1024;
const MAX_STDERR_BYTES = 16 * 1024;
const MAX_MODELS = 5_000;
const DEFAULT_TIMEOUT_MS = 120_000;
const PI_EXTENSION_RESOLUTION_TIMEOUT_MS = 5_000;
const PI_EXTENSION_PACKAGE_JSON_MAX_BYTES = 1024 * 1024;
const PI_EXTENSION_PACKAGE_MAX_ENTRIES = 10_000;
export const PI_RPC_READY_TTL_MS = 15_000;
const PI_TERMINATION_RESERVE_MS = 5_000;
const PI_MIN_OPERATION_TIMEOUT_MS = PI_TERMINATION_RESERVE_MS + 500;
const CONNECTION_TEST_PROMPT = 'Reply with exactly: TALKING_QUILL_CONNECTION_OK';
const MODEL_ID = /^[A-Za-z0-9][A-Za-z0-9._:@+-]{0,127}\/[A-Za-z0-9][A-Za-z0-9._:@+-]{0,383}$/u;

export interface PiProviderOptions {
  readonly spawnPi?: SpawnPi;
  readonly environment?: NodeJS.ProcessEnv;
  readonly platform?: NodeJS.Platform;
  readonly workingDirectory?: string;
  readonly now?: () => number;
  readonly observeEgress?: EgressObserver;
  readonly configuredPath?: () => string | null;
  readonly interactiveAppData?: string;
  readonly interactiveHome?: string;
  readonly resolveCli?: (signal?: AbortSignal) => Promise<PiCliIdentity>;
  readonly revalidateCli?: (identity: PiCliIdentity, signal?: AbortSignal) => Promise<void>;
  readonly canonicalizeExtensionPath?: (path: string) => Promise<string>;
  readonly statExtensionPath?: (path: string) => Promise<Stats>;
  readonly accessExtensionPath?: (path: string, mode?: number) => Promise<void>;
  readonly readExtensionFile?: (path: string) => Promise<string>;
  readonly readExtensionDirectory?: (path: string) => Promise<readonly Dirent[]>;
  readonly extensionResolutionTimeoutMs?: number;
  readonly rpcReadyTtlMs?: number;
  readonly prewarmRpcOperation?: typeof prewarmPiRpcOperation;
  readonly terminateRpcTree?: TerminatePiRpcTree;
  /** Receives timing stage enums only; no content, paths, model IDs, or extension IDs are exposed. */
  readonly onRpcTiming?: (stage: PiRpcTimingStage) => void;
}

type PiConfig = ProviderConfig & {
  readonly providerId: 'pi';
  readonly modelId: string;
  readonly thinking: NonNullable<ProviderConfig['thinking']>;
};
interface ModelCatalog {
  readonly key: string;
  readonly models: readonly ModelInfo[];
}
interface ResolvedPiExtensions {
  readonly args: readonly string[];
  readonly canonicalSources: readonly string[];
  readonly cacheKey: readonly (readonly (string | number)[])[];
}
interface ResolvedPiExtension {
  readonly argument: string;
  readonly canonicalSource: string;
  readonly cacheKey: readonly (string | number)[];
}
interface FrozenPiLaunch {
  readonly config: PiConfig;
  readonly configuredPath: string | null;
  readonly identity: PiCliIdentity;
  readonly extensions: ResolvedPiExtensions;
  readonly expected: Readonly<{ provider: string; model: string; thinking: PiConfig['thinking'] }>;
  readonly timeoutMs: number;
}
interface ModelCache extends ModelCatalog {
  readonly expiresAt: number;
}

export class PiProvider implements SmartProvider {
  readonly id = 'pi' as const;
  readonly credentialPolicy = 'none' as const;
  readonly #spawnPi: SpawnPi | undefined;
  readonly #environment: NodeJS.ProcessEnv;
  readonly #platform: NodeJS.Platform;
  readonly #workingDirectory: string;
  readonly #now: () => number;
  readonly #observeEgress: EgressObserver;
  readonly #configuredPath: () => string | null;
  readonly #resolveCli: (
    configuredPath: string | null,
    signal?: AbortSignal,
  ) => Promise<PiCliIdentity>;
  readonly #revalidateCli: (identity: PiCliIdentity, signal?: AbortSignal) => Promise<void>;
  readonly #canonicalizeExtensionPath: (path: string) => Promise<string>;
  readonly #statExtensionPath: (path: string) => Promise<Stats>;
  readonly #accessExtensionPath: (path: string, mode?: number) => Promise<void>;
  readonly #readExtensionFile: (path: string) => Promise<string>;
  readonly #readExtensionDirectory: (path: string) => Promise<readonly Dirent[]>;
  readonly #extensionResolutionTimeoutMs: number;
  readonly #rpcReadyTtlMs: number;
  readonly #prewarmRpcOperation: typeof prewarmPiRpcOperation;
  readonly #terminateRpcTree: TerminatePiRpcTree | undefined;
  readonly #onRpcTiming: ((stage: PiRpcTimingStage) => void) | undefined;
  readonly #scheduler = new PiOperationScheduler();
  #identity: { readonly configuredPath: string | null; readonly value: PiCliIdentity } | null =
    null;
  #models: ModelCache | null = null;

  constructor(options: PiProviderOptions = {}) {
    this.#spawnPi = options.spawnPi;
    this.#platform = options.platform ?? process.platform;
    this.#environment = withInteractiveHome(
      options.environment ?? process.env,
      this.#platform,
      options.interactiveHome,
    );
    this.#workingDirectory = options.workingDirectory ?? process.cwd();
    this.#now = options.now ?? Date.now;
    this.#observeEgress = options.observeEgress ?? (() => undefined);
    this.#configuredPath = options.configuredPath ?? (() => null);
    this.#canonicalizeExtensionPath = options.canonicalizeExtensionPath ?? realpath;
    this.#statExtensionPath = options.statExtensionPath ?? stat;
    this.#accessExtensionPath = options.accessExtensionPath ?? access;
    this.#readExtensionFile = options.readExtensionFile ?? ((path) => readFile(path, 'utf8'));
    this.#readExtensionDirectory =
      options.readExtensionDirectory ?? ((path) => readdir(path, { withFileTypes: true }));
    this.#extensionResolutionTimeoutMs =
      options.extensionResolutionTimeoutMs ?? PI_EXTENSION_RESOLUTION_TIMEOUT_MS;
    this.#rpcReadyTtlMs = boundedPiDuration(options.rpcReadyTtlMs ?? PI_RPC_READY_TTL_MS);
    this.#prewarmRpcOperation = options.prewarmRpcOperation ?? prewarmPiRpcOperation;
    this.#terminateRpcTree = options.terminateRpcTree;
    this.#onRpcTiming = options.onRpcTiming;
    const customResolveCli = options.resolveCli;
    this.#resolveCli =
      customResolveCli === undefined
        ? (configuredPath, signal) =>
            resolveCanonicalPiCli(
              this.#environment,
              this.#platform,
              configuredPath,
              options.interactiveAppData,
              signal,
            )
        : (_configuredPath, signal) => customResolveCli(signal);
    this.#revalidateCli =
      options.revalidateCli ??
      (options.resolveCli === undefined
        ? (identity, signal) => revalidatePiCliIdentity(identity, this.#platform, signal)
        : () => Promise.resolve());
  }

  credentialBinding(config: ProviderConfig): string {
    this.#baseConfig(config);
    return 'pi:user-cli';
  }

  async validate(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<ProviderValidationResult> {
    const config = this.#runtimeConfig(invocation.config);
    this.#observeEgress('provider');
    const extensions = await this.#resolveExtensions(config, signal);
    const identity = await this.#resolveIdentity(signal);
    const models = parsePiModels(
      await this.#runResolved(
        identity,
        ['--list-models', ...identity.safetyFlags, ...extensions.args],
        null,
        signal,
        DEFAULT_TIMEOUT_MS,
      ),
    );
    if (models.length > 0 && !models.some(({ id }) => id === config.modelId))
      throw new ProviderError('MODEL_NOT_FOUND');
    const output = await this.#runResolved(
      identity,
      [
        '-p',
        '--model',
        config.modelId,
        '--thinking',
        config.thinking,
        ...identity.safetyFlags,
        ...extensions.args,
      ],
      CONNECTION_TEST_PROMPT,
      signal,
      Math.max(config.timeoutMs ?? DEFAULT_TIMEOUT_MS, PI_MIN_OPERATION_TIMEOUT_MS),
    );
    if (output.trim().length === 0) throw new ProviderError('INVALID_RESPONSE');
    return Object.freeze({ ok: true, destination: 'cloud', modelCount: models.length });
  }

  async listModels(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<readonly ModelInfo[]> {
    const config = this.#baseConfig(invocation.config);
    this.#observeEgress('provider');
    const extensions = await this.#resolveExtensions(config, signal);
    const identity = await this.#resolveIdentity(signal);
    const key = JSON.stringify([identityKey(identity), extensions.cacheKey]);
    if (
      invocation.refreshModels !== true &&
      this.#models?.key === key &&
      this.#models.expiresAt > this.#now()
    )
      return this.#models.models;
    const models = parsePiModels(
      await this.#runResolved(
        identity,
        ['--list-models', ...identity.safetyFlags, ...extensions.args],
        null,
        signal,
        DEFAULT_TIMEOUT_MS,
      ),
    );
    this.#models = { key, models, expiresAt: this.#now() + MODEL_CACHE_TTL_MS };
    return models;
  }

  capabilities(): VisionCapability {
    return 'unsupported';
  }

  async prepareCompletion(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<PreparedProviderCompletion | null> {
    const config = freezePiConfig(this.#runtimeConfig(invocation.config));
    // The observer remains the first side effect before extension filesystem work or process work.
    this.#observeEgress('provider');
    const configuredPath = this.#configuredPath();
    const initialExtensions = await this.#resolveExtensions(config, signal);
    let identity = await this.#resolveIdentity(signal, configuredPath);
    const permit = await this.#scheduler.acquireSpeculative(signal);
    let linked: LinkedAbortSignal | null = null;
    try {
      identity = await this.#resolvePreparedIdentity(identity, configuredPath, signal);
      const extensions = await this.#resolveExtensions(config, signal);
      if (this.#configuredPath() !== configuredPath) {
        throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      }
      if (!sameExtensionIdentity(initialExtensions, extensions)) {
        throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      }
      const [provider, model] = splitPiModelId(config.modelId);
      const snapshot: FrozenPiLaunch = Object.freeze({
        config,
        configuredPath,
        identity,
        extensions,
        expected: Object.freeze({ provider, model, thinking: config.thinking }),
        timeoutMs: Math.max(config.timeoutMs ?? DEFAULT_TIMEOUT_MS, PI_MIN_OPERATION_TIMEOUT_MS),
      });
      try {
        assertRpcCompatibility(identity);
      } catch {
        permit.release();
        return this.#createFallbackPreparedCompletion(snapshot);
      }
      const startupLink = linkAbortSignals([signal, permit.revocationSignal]);
      linked = startupLink;
      try {
        const operation = await this.#prewarmRpcOperation({
          identity,
          expected: snapshot.expected,
          explicitExtensions: extensions.canonicalSources,
          signal: startupLink.signal,
          environment: this.#environment,
          platform: this.#platform,
          workingDirectory: this.#workingDirectory,
          ...(this.#spawnPi === undefined ? {} : { spawnPi: this.#spawnPi }),
          ...(this.#terminateRpcTree === undefined
            ? {}
            : { terminateTree: this.#terminateRpcTree }),
          timeoutMs: snapshot.timeoutMs,
          ...(this.#onRpcTiming === undefined ? {} : { onTiming: this.#onRpcTiming }),
        });
        linked = null;
        return this.#createRpcPreparedCompletion(snapshot, operation, permit, startupLink);
      } catch (error: unknown) {
        startupLink.dispose();
        linked = null;
        const normalized = toPreparedProviderError(error);
        if (!normalized.fallbackEligible) {
          permit.fail(normalized);
          throw normalized;
        }
        permit.release();
        if (signal.aborted) throw new ProviderError('CANCELLED');
        return this.#createFallbackPreparedCompletion(snapshot);
      }
    } catch (error: unknown) {
      linked?.dispose();
      permit.release();
      throw error;
    }
  }

  async cleanTranscript(
    invocation: ProviderInvocationConfig,
    requestInput: ProviderCompletionRequest,
    signal: AbortSignal,
  ): Promise<string> {
    const config = this.#runtimeConfig(invocation.config);
    const request = ProviderCompletionRequestSchema.parse(requestInput);
    if (request.image !== undefined) throw new ProviderError('INVALID_CONFIG');
    const modelId = request.modelId ?? config.modelId;
    assertModelId(modelId);
    this.#observeEgress('provider');
    const extensions = await this.#resolveExtensions(config, signal);
    try {
      const identity = await this.#resolveIdentity(signal);
      const output = await this.#runResolved(
        identity,
        [
          '-p',
          '--model',
          modelId,
          '--thinking',
          config.thinking,
          ...identity.safetyFlags,
          ...extensions.args,
        ],
        request.input,
        signal,
        Math.max(config.timeoutMs ?? DEFAULT_TIMEOUT_MS, PI_MIN_OPERATION_TIMEOUT_MS),
      );
      if (output.length > MAX_NATIVE_OUTPUT_CHARACTERS || output.trim().length === 0)
        throw new ProviderError('INVALID_RESPONSE');
      return output.trim();
    } catch (error: unknown) {
      this.#models = null;
      throw error;
    }
  }

  classifyDestination(
    invocation: ProviderInvocationConfig,
    signal: AbortSignal,
  ): Promise<Destination> {
    this.#baseConfig(invocation.config);
    return signal.aborted
      ? Promise.reject(new ProviderError('CANCELLED'))
      : Promise.resolve('cloud');
  }

  #baseConfig(input: ProviderConfig): ProviderConfig & { readonly providerId: 'pi' } {
    try {
      const config = ProviderConfigSchema.parse(input);
      if (config.providerId !== 'pi') throw new ProviderError('INVALID_CONFIG');
      return config as ProviderConfig & { readonly providerId: 'pi' };
    } catch (error: unknown) {
      if (error instanceof ProviderError) throw error;
      throw new ProviderError('INVALID_CONFIG');
    }
  }

  #runtimeConfig(input: ProviderConfig): PiConfig {
    const config = this.#baseConfig(input);
    if (config.modelId == null || config.thinking === undefined)
      throw new ProviderError('INVALID_CONFIG');
    assertModelId(config.modelId);
    return config as PiConfig;
  }

  async #resolveExtensions(
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

  async #resolvePreparedIdentity(
    initial: PiCliIdentity,
    configuredPath: string | null,
    signal: AbortSignal,
  ): Promise<PiCliIdentity> {
    try {
      await waitForAbort(this.#revalidateCli(initial, signal), signal);
      return initial;
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
      this.#identity = null;
      this.#models = null;
      return await this.#resolveIdentity(signal, configuredPath, true);
    }
  }

  #createFallbackPreparedCompletion(snapshot: FrozenPiLaunch): PreparedProviderCompletion {
    const closed = deferred<undefined>();
    let consumed = false;
    let settled = false;
    const timer = setTimeout(() => settle(), this.#rpcReadyTtlMs);
    timer.unref();
    const settle = (): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      closed.resolve(undefined);
    };
    const requestClose = (): void => settle();
    const complete = async (
      requestInput: ProviderCompletionRequest,
      signal: AbortSignal,
    ): Promise<string> => {
      if (consumed) throw new ProviderError('INVALID_CONFIG');
      consumed = true;
      clearTimeout(timer);
      if (settled) throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      let request: ProviderCompletionRequest & { readonly input: string };
      try {
        request = this.#preparedRequest(snapshot, requestInput);
      } catch (error: unknown) {
        settle();
        throw error;
      }
      if (signal.aborted) {
        settle();
        throw new ProviderError('CANCELLED', { fallbackEligible: true });
      }
      try {
        return await this.#runFrozenPrint(snapshot, request.input, signal);
      } finally {
        settle();
      }
    };
    return Object.freeze({ complete, requestClose, closed: closed.promise });
  }

  #createRpcPreparedCompletion(
    snapshot: FrozenPiLaunch,
    operation: PiRpcOperation,
    permit: PiSpeculativeOperationPermit,
    lifetimeLink: LinkedAbortSignal,
  ): PreparedProviderCompletion {
    const closed = deferred<undefined>();
    const closeController = new AbortController();
    let consumed = false;
    let terminal = false;
    let settled = false;
    let cleanupSettled = false;
    let cleanupError: ProviderError | null = null;
    let retirementSettled = false;
    let retirementError: ProviderError | null = null;
    const timer = setTimeout(() => requestClose(), this.#rpcReadyTtlMs);
    timer.unref();

    const settleIfTerminal = (): void => {
      if (settled || !terminal || !cleanupSettled || !retirementSettled) return;
      settled = true;
      clearTimeout(timer);
      lifetimeLink.dispose();
      const error = cleanupError ?? retirementError;
      if (error === null) closed.resolve(undefined);
      else closed.reject(error);
    };
    void operation.cleanup.then(
      () => {
        cleanupSettled = true;
        permit.release();
        settleIfTerminal();
      },
      (error: unknown) => {
        cleanupSettled = true;
        cleanupError = toPreparedProviderError(error);
        permit.fail(cleanupError);
        settleIfTerminal();
      },
    );
    void operation.retirement.then(
      () => {
        retirementSettled = true;
        settleIfTerminal();
      },
      (error: unknown) => {
        retirementSettled = true;
        retirementError = toPreparedProviderError(error);
        settleIfTerminal();
      },
    );

    const requestClose = (): void => {
      if (!terminal) terminal = true;
      clearTimeout(timer);
      if (!closeController.signal.aborted) closeController.abort();
      void operation.abort().catch(() => undefined);
      settleIfTerminal();
    };
    const complete = async (
      requestInput: ProviderCompletionRequest,
      signal: AbortSignal,
    ): Promise<string> => {
      if (consumed) throw new ProviderError('INVALID_CONFIG');
      consumed = true;
      clearTimeout(timer);
      if (terminal) throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      let request: ProviderCompletionRequest & { readonly input: string };
      try {
        request = this.#preparedRequest(snapshot, requestInput);
      } catch (error: unknown) {
        requestClose();
        throw error;
      }
      if (signal.aborted) {
        requestClose();
        throw new ProviderError('CANCELLED', { fallbackEligible: true });
      }
      const completionLink = linkAbortSignals([signal, closeController.signal]);
      try {
        if (!retirementSettled) {
          try {
            await this.#assertFrozenLaunchCurrent(snapshot, completionLink.signal);
          } catch (error: unknown) {
            terminal = true;
            void operation.abort().catch(() => undefined);
            throw toResourceChangeError(error, signal);
          }
          try {
            const result = await operation.prompt(request.input, completionLink.signal, () =>
              permit.commit(),
            );
            terminal = true;
            settleIfTerminal();
            return result.text;
          } catch (error: unknown) {
            const normalized = toPreparedProviderError(error);
            if (!normalized.fallbackEligible || completionLink.signal.aborted) {
              terminal = true;
              settleIfTerminal();
              throw normalized;
            }
          }
        }
        terminal = true;
        await operation.cleanup;
        return await this.#runFrozenPrint(snapshot, request.input, signal);
      } finally {
        completionLink.dispose();
        settleIfTerminal();
      }
    };
    return Object.freeze({ complete, requestClose, closed: closed.promise });
  }

  #preparedRequest(
    snapshot: FrozenPiLaunch,
    requestInput: ProviderCompletionRequest,
  ): ProviderCompletionRequest & { readonly input: string } {
    let request: ProviderCompletionRequest;
    try {
      request = ProviderCompletionRequestSchema.parse(requestInput);
    } catch {
      throw new ProviderError('INVALID_CONFIG', { fallbackEligible: true });
    }
    if (
      request.image !== undefined ||
      (request.modelId ?? snapshot.config.modelId) !== snapshot.config.modelId
    ) {
      throw new ProviderError('INVALID_CONFIG', { fallbackEligible: true });
    }
    return request;
  }

  async #assertFrozenLaunchCurrent(snapshot: FrozenPiLaunch, signal: AbortSignal): Promise<void> {
    if (this.#configuredPath() !== snapshot.configuredPath) {
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
    }
    try {
      await waitForAbort(this.#revalidateCli(snapshot.identity, signal), signal);
      const extensions = await this.#resolveExtensions(snapshot.config, signal);
      if (!sameExtensionIdentity(snapshot.extensions, extensions))
        throw new Error('resource changed');
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
      this.#identity = null;
      this.#models = null;
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
    }
  }

  async #runFrozenPrint(
    snapshot: FrozenPiLaunch,
    input: string,
    signal: AbortSignal,
  ): Promise<string> {
    const permit = await this.#scheduler.acquireForeground(signal);
    try {
      await this.#assertFrozenLaunchCurrent(snapshot, signal);
      const result = await runPiInvocation(
        snapshot.identity.canonicalPath,
        [
          '-p',
          '--model',
          snapshot.config.modelId,
          '--thinking',
          snapshot.config.thinking,
          ...snapshot.identity.safetyFlags,
          ...canonicalExtensionArguments(snapshot.extensions.canonicalSources),
        ],
        input,
        signal,
        snapshot.timeoutMs,
        {
          spawnPi: this.#spawnPi,
          environment: this.#environment,
          platform: this.#platform,
          workingDirectory: this.#workingDirectory,
          maxStdoutBytes: MAX_STDOUT_BYTES,
          maxStderrBytes: MAX_STDERR_BYTES,
        },
      );
      if (result.code !== 0) throw classifyPiFailure(result.stderr);
      if (
        result.stdout.length > MAX_NATIVE_OUTPUT_CHARACTERS ||
        result.stdout.trim().length === 0
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return result.stdout.trim();
    } finally {
      permit.release();
    }
  }

  async #resolveIdentity(
    signal: AbortSignal,
    configuredPath = this.#configuredPath(),
    operationHeld = false,
  ): Promise<PiCliIdentity> {
    const cached = await this.#revalidatedCachedIdentity(configuredPath, signal);
    if (cached !== null) return cached;
    const permit = operationHeld ? null : await this.#scheduler.acquireForeground(signal);
    try {
      // Another queued resolver may have populated the same frozen configured path.
      const queuedCache = await this.#revalidatedCachedIdentity(configuredPath, signal);
      if (queuedCache !== null) return queuedCache;
      try {
        const value = await waitForAbort(this.#resolveCli(configuredPath, signal), signal);
        this.#identity = Object.freeze({ configuredPath, value });
        return value;
      } catch (error: unknown) {
        if (error instanceof ProviderError) throw error;
        throw new ProviderError('PI_NOT_FOUND');
      }
    } finally {
      permit?.release();
    }
  }

  async #revalidatedCachedIdentity(
    configuredPath: string | null,
    signal: AbortSignal,
  ): Promise<PiCliIdentity | null> {
    if (this.#identity?.configuredPath !== configuredPath) return null;
    try {
      await waitForAbort(this.#revalidateCli(this.#identity.value, signal), signal);
      return this.#identity.value;
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
      this.#identity = null;
      this.#models = null;
      return null;
    }
  }

  async #runResolved(
    resolvedIdentity: PiCliIdentity,
    args: readonly string[],
    input: string | null,
    signal: AbortSignal,
    timeoutMs: number,
  ): Promise<string> {
    if (signal.aborted) throw new ProviderError('CANCELLED');
    const permit = await this.#scheduler.acquireForeground(signal);
    try {
      let identity = resolvedIdentity;
      try {
        await waitForAbort(this.#revalidateCli(identity, signal), signal);
      } catch (error: unknown) {
        if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
        this.#identity = null;
        this.#models = null;
        identity = await this.#resolveIdentity(signal, this.#configuredPath(), true);
      }
      const result = await runPiInvocation(identity.canonicalPath, args, input, signal, timeoutMs, {
        spawnPi: this.#spawnPi,
        environment: this.#environment,
        platform: this.#platform,
        workingDirectory: this.#workingDirectory,
        maxStdoutBytes: MAX_STDOUT_BYTES,
        maxStderrBytes: MAX_STDERR_BYTES,
      });
      if (result.code === 0) return result.stdout;
      throw classifyPiFailure(result.stderr);
    } finally {
      permit.release();
    }
  }
}

export function parsePiModels(output: string): readonly ModelInfo[] {
  if (Buffer.byteLength(output, 'utf8') > MAX_STDOUT_BYTES)
    throw new ProviderError('RESPONSE_TOO_LARGE');
  const lines = output
    .replace(/^\uFEFF/u, '')
    .replace(new RegExp(`${String.fromCharCode(27)}\\[[0-?]*[ -/]*[@-~]`, 'gu'), '')
    .replace(/\r/g, '')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0 || /^no models/iu.test(lines[0] ?? '')) return Object.freeze([]);
  const headerIndex = lines.findIndex((line) => /provider/iu.test(line) && /model/iu.test(line));
  if (headerIndex < 0) return Object.freeze([]);
  const header = (lines[headerIndex] ?? '').toLowerCase().split(/\s{2,}|\t+|\s+/u);
  const providerIndex = header.indexOf('provider');
  const modelIndex = header.indexOf('model');
  const contextIndex = header.findIndex((value) => value.startsWith('context'));
  const imagesIndex = header.findIndex((value) => value.startsWith('image'));
  if (providerIndex < 0 || modelIndex < 0) return Object.freeze([]);
  const models: ModelInfo[] = [];
  const seen = new Set<string>();
  for (const line of lines.slice(headerIndex + 1)) {
    const columns = line.split(/\s{2,}|\t+/u).filter(Boolean);
    const fallback =
      columns.length < Math.max(providerIndex, modelIndex) + 1 ? line.split(/\s+/u) : columns;
    const provider = fallback[providerIndex];
    const model = fallback[modelIndex];
    if (provider === undefined || model === undefined) continue;
    const id = model.includes('/') ? model : `${provider}/${model}`;
    if (!MODEL_ID.test(id) || seen.has(id)) continue;
    let contextWindow: number | null = null;
    if (contextIndex >= 0 && fallback[contextIndex] !== undefined)
      contextWindow = parseCompactTokens(fallback[contextIndex] ?? '');
    const vision =
      imagesIndex >= 0 && /^(?:yes|true|supported)$/iu.test(fallback[imagesIndex] ?? '')
        ? 'supported'
        : 'unsupported';
    const parsed = ModelInfoSchema.safeParse({ id, name: id, contextWindow, vision });
    if (parsed.success) {
      seen.add(id);
      models.push(parsed.data);
    }
    if (models.length >= MAX_MODELS) break;
  }
  return Object.freeze(models.sort((left, right) => left.id.localeCompare(right.id)));
}

function parseCompactTokens(value: string): number | null {
  const match = /^(\d+(?:\.\d+)?)([kKmM])?$/u.exec(value);
  if (match?.[1] === undefined) return null;
  const multiplier =
    match[2]?.toLowerCase() === 'm' ? 1_000_000 : match[2]?.toLowerCase() === 'k' ? 1_000 : 1;
  const result = Number(match[1]) * multiplier;
  return Number.isSafeInteger(result) && result > 0 && result <= 2_000_000 ? result : null;
}
function assertModelId(modelId: string): void {
  if (!MODEL_ID.test(modelId)) throw new ProviderError('INVALID_CONFIG');
}

function splitPiModelId(modelId: string): readonly [string, string] {
  assertModelId(modelId);
  const separator = modelId.indexOf('/');
  return Object.freeze([modelId.slice(0, separator), modelId.slice(separator + 1)]);
}

function freezePiConfig(config: PiConfig): PiConfig {
  return Object.freeze({
    ...config,
    ...(config.piExtensionSources === undefined
      ? {}
      : { piExtensionSources: Object.freeze([...config.piExtensionSources]) }),
  }) as PiConfig;
}

function sameExtensionIdentity(left: ResolvedPiExtensions, right: ResolvedPiExtensions): boolean {
  return (
    JSON.stringify([left.canonicalSources, left.cacheKey]) ===
    JSON.stringify([right.canonicalSources, right.cacheKey])
  );
}

function canonicalExtensionArguments(sources: readonly string[]): readonly string[] {
  return Object.freeze(sources.flatMap((source) => ['-e', source]));
}

function boundedPiDuration(value: number): number {
  if (!Number.isSafeInteger(value) || value < 1 || value > DEFAULT_TIMEOUT_MS) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return value;
}

function toPreparedProviderError(error: unknown): ProviderError {
  return error instanceof ProviderError ? error : new ProviderError('UNAVAILABLE');
}

function toResourceChangeError(error: unknown, signal: AbortSignal): ProviderError {
  const normalized = toPreparedProviderError(error);
  if (signal.aborted || normalized.code === 'CANCELLED') {
    return new ProviderError('CANCELLED', { fallbackEligible: true });
  }
  return new ProviderError(normalized.code, { fallbackEligible: true });
}

interface LinkedAbortSignal {
  readonly signal: AbortSignal;
  dispose(): void;
}

function linkAbortSignals(signals: readonly AbortSignal[]): LinkedAbortSignal {
  const controller = new AbortController();
  const abort = (): void => {
    if (!controller.signal.aborted) controller.abort();
  };
  for (const signal of signals) {
    if (signal.aborted) abort();
    else signal.addEventListener('abort', abort, { once: true });
  }
  return Object.freeze({
    signal: controller.signal,
    dispose: () => {
      for (const signal of signals) signal.removeEventListener('abort', abort);
    },
  });
}

function deferred<Result>() {
  let resolveDeferred!: (value: Result | PromiseLike<Result>) => void;
  let rejectDeferred!: (reason?: unknown) => void;
  const promise = new Promise<Result>((resolvePromise, rejectPromise) => {
    resolveDeferred = resolvePromise;
    rejectDeferred = rejectPromise;
  });
  return Object.freeze({ promise, resolve: resolveDeferred, reject: rejectDeferred });
}

function classifyPiFailure(stderr: string): ProviderError {
  const value = stderr.toLowerCase();
  if (/no api key|authentication|unauthori[sz]ed|log in|\/login/u.test(value))
    return new ProviderError('AUTHENTICATION_FAILED');
  if (/rate limit|too many requests/u.test(value)) return new ProviderError('RATE_LIMITED');
  if (/model[^\r\n]*(?:not found|not available|unknown)/u.test(value))
    return new ProviderError('MODEL_NOT_FOUND');
  if (/unknown option|invalid (?:option|argument)|usage:/u.test(value))
    return new ProviderError('PI_LAUNCH_FAILED');
  return new ProviderError('REMOTE_FAILURE');
}
function withInteractiveHome(
  source: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
  home: string | undefined,
): NodeJS.ProcessEnv {
  if (platform !== 'win32' || home === undefined) return source;
  return Object.fromEntries([
    ...Object.entries(source).filter(([key]) => key.toLowerCase() !== 'userprofile'),
    ['USERPROFILE', home],
  ]);
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

function waitForAbort<Result>(operation: Promise<Result>, signal: AbortSignal): Promise<Result> {
  if (signal.aborted) return Promise.reject(new ProviderError('CANCELLED'));
  return new Promise((resolveOperation, rejectOperation) => {
    const abort = (): void => rejectOperation(new ProviderError('CANCELLED'));
    signal.addEventListener('abort', abort, { once: true });
    void operation.then(
      (result) => {
        signal.removeEventListener('abort', abort);
        resolveOperation(result);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        rejectOperation(error instanceof Error ? error : new ProviderError('UNAVAILABLE'));
      },
    );
  });
}
