import { normalize } from 'node:path';
import type { Plugin } from 'vite';

const targets = {
  application: normalize('/src/main/app/application.ts').replaceAll('\\', '/'),
  bootstrap: normalize('/src/main/bootstrap.ts').replaceAll('\\', '/'),
  diagnostic: normalize('/src/main/security/diagnostic-logger.ts').replaceAll('\\', '/'),
  helperClient: normalize('/src/main/helper/helper-client.ts').replaceAll('\\', '/'),
  helperChannel: normalize('/src/main/helper/helper-rpc-channel.ts').replaceAll('\\', '/'),
} as const;

export function windowsInstalledAcceptanceOverlay(): Plugin {
  const transformed = new Set<string>();
  return {
    name: 'talking-quill-windows-installed-acceptance-overlay',
    enforce: 'pre',
    transform(source, id) {
      const target = Object.entries(targets).find(([, suffix]) =>
        id.replaceAll('\\', '/').endsWith(suffix),
      );
      if (target === undefined) return null;
      const [name] = target;
      if (transformed.has(name)) throw new Error(`Acceptance overlay transformed ${name} twice`);
      transformed.add(name);
      return { code: transformWindowsInstalledAcceptanceSource(name, source), map: null };
    },
    buildEnd() {
      const missing = Object.keys(targets).filter((name) => !transformed.has(name));
      if (missing.length > 0) {
        throw new Error(`Acceptance overlay did not reach required modules: ${missing.join(', ')}`);
      }
    },
  };
}

export function transformWindowsInstalledAcceptanceSource(name: string, source: string): string {
  const normalized = source.replaceAll('\r\n', '\n');
  switch (name) {
    case 'application':
      return transformApplication(normalized);
    case 'bootstrap':
      return transformBootstrap(normalized);
    case 'diagnostic':
      return transformDiagnosticLogger(normalized);
    case 'helperClient':
      return transformHelperClient(normalized);
    case 'helperChannel':
      return transformHelperRpcChannel(normalized);
    default:
      throw new Error(`Unknown acceptance overlay module: ${name}`);
  }
}

function transformApplication(source: string): string {
  let output = replaceExact(
    source,
    "import { WindowRoleRegistry } from './window-role-registry';",
    `import { WindowRoleRegistry } from './window-role-registry';
import {
  runInstalledObservation as executeInstalledObservation,
  type InstalledAcceptanceHelper,
  type InstalledObservationRequest,
} from '../acceptance/installed-observation';`,
  );
  output = replaceExact(
    output,
    '  #applicationActivationSequence = 0;\n',
    `  #applicationActivationSequence = 0;
  #installedLoginStartObserved = false;
  readonly #installedLoginWaiters = new Set<() => void>();
`,
  );
  output = replaceExact(
    output,
    '  start(): Promise<void> {',
    `  handleInstalledAcceptanceLoginStart(): void {
    this.#installedLoginStartObserved = true;
    for (const waiter of this.#installedLoginWaiters) waiter();
    this.#installedLoginWaiters.clear();
  }

  async #waitForInstalledAcceptanceLoginStart(timeoutMs: number): Promise<boolean> {
    if (this.#installedLoginStartObserved) return true;
    return new Promise((resolveWait) => {
      const complete = () => {
        clearTimeout(timer);
        this.#installedLoginWaiters.delete(complete);
        resolveWait(true);
      };
      const timer = setTimeout(() => {
        this.#installedLoginWaiters.delete(complete);
        resolveWait(false);
      }, timeoutMs);
      this.#installedLoginWaiters.add(complete);
    });
  }

  async runInstalledAcceptance(request: InstalledObservationRequest): Promise<void> {
    if (
      this.#lifecycle !== 'running' ||
      this.#helper === null ||
      this.#settings === null ||
      this.#windows === null
    ) {
      throw new Error('Application-owned installed observation is not ready');
    }
    const windows = this.#windows;
    await executeInstalledObservation(this.#helper as InstalledAcceptanceHelper, request, {
      profiles: this.#settings.get().dictationProfiles,
      persistentWindowRolesReady: windows.hasPersistentWindowRoles(),
      userDataRoot: app.getPath('userData'),
      showValidationWidget: async () => {
        if (!(await windows.createWidgetForActivation())) return false;
        return windows.showWidget(this.#settings?.get().app.widgetSize ?? 'default');
      },
      hideValidationWidget: () => windows.removeWidget(),
      windowsLoginStart: this.#windowsLoginStart,
      mainWindowVisible: windows.isMainVisible(),
      waitForIgnoredLoginStart: (timeoutMs) =>
        this.#waitForInstalledAcceptanceLoginStart(timeoutMs),
      probeDiagnostics: async () => ({
        enabled: this.#diagnostics?.enabled === true,
        injectedFailureContained:
          (await this.#diagnostics?.probeInstalledAcceptanceWriteFailure()) === true,
      }),
    });
  }

  start(): Promise<void> {`,
  );
  return output;
}

function transformBootstrap(source: string): string {
  let output = replaceExact(
    source,
    "import { createNativeOwnedTreeRemoval } from './data/native-owned-tree-removal';",
    `import { createNativeOwnedTreeRemoval } from './data/native-owned-tree-removal';
import type { InstalledObservationRequest } from './acceptance/installed-observation';`,
  );
  output = replaceExact(
    output,
    '  readonly hiddenStartupFailure?: boolean;\n',
    '  readonly hiddenStartupFailure?: boolean;\n  readonly installedObservation?: InstalledObservationRequest;\n',
  );
  output = replaceExact(
    output,
    "      if (loginStart === 'login-start') return;",
    `      if (loginStart === 'login-start') {
        application?.handleInstalledAcceptanceLoginStart();
        return;
      }`,
  );
  output = replaceExact(
    output,
    `        await application.start();
        if (restoreRequested !== null) application.handleApplicationActivation(restoreRequested);`,
    `        await application.start();
        if (options.installedObservation !== undefined) {
          try {
            await application.runInstalledAcceptance(options.installedObservation);
          } finally {
            await application.stop();
          }
          return;
        }
        if (restoreRequested !== null) application.handleApplicationActivation(restoreRequested);`,
  );
  return output;
}

function transformDiagnosticLogger(source: string): string {
  return replaceExact(
    source,
    '  async initialize(): Promise<void> {',
    `  async probeInstalledAcceptanceWriteFailure(): Promise<boolean> {
    try {
      await this.#enqueue(() => Promise.reject(new Error('acceptance-injected-write-failure')));
      return false;
    } catch {
      return true;
    }
  }

  async initialize(): Promise<void> {`,
  );
}

function transformHelperClient(source: string): string {
  let output = replaceExact(
    source,
    "import { z } from 'zod';",
    "import { z, type ZodType } from 'zod';",
  );
  output = replaceExact(
    output,
    '  async getRuntimeObservability(): Promise<HelperRuntimeObservability> {',
    `  requestAcceptance(method: string, resultSchema: ZodType, timeoutMs: number): Promise<unknown> {
    const session = this.#rpcSession;
    if (session === null || !this.#ordinaryRequestsAvailable() || !this.#desiredRunning) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is terminating'));
    }
    return this.#rpcChannel.requestAcceptance(session, method, resultSchema, {
      timeoutMs,
      timeoutReason: 'request-timeout',
      allowDraining: false,
      supervision: false,
    });
  }

  async getRuntimeObservability(): Promise<HelperRuntimeObservability> {`,
  );
  return output;
}

function transformHelperRpcChannel(source: string): string {
  let output = replaceExact(
    source,
    "import { performance } from 'node:perf_hooks';",
    "import { performance } from 'node:perf_hooks';\nimport { z, type ZodType } from 'zod';",
  );
  output = replaceExact(
    output,
    '  readonly method: HelperMethod;\n  readonly activationConfiguration:',
    '  readonly method: string;\n  readonly resultSchema: ZodType;\n  readonly activationConfiguration:',
  );
  const start = output.indexOf('  request<Method extends HelperMethod>(');
  const end = output.indexOf('\n  close(session: HelperRpcSession', start);
  if (start < 0 || end < 0) throw new Error('Acceptance overlay could not locate helper request');
  const original = output.slice(start, end);
  let shared = replaceExact(
    original,
    `  request<Method extends HelperMethod>(
    session: HelperRpcSession,
    method: Method,
    params: HelperParams<Method>,
    options: HelperRpcRequestOptions,
  ): Promise<HelperResult<Method>> {`,
    `  #request(
    session: HelperRpcSession,
    method: string,
    params: unknown,
    paramsSchema: ZodType,
    resultSchema: ZodType,
    options: HelperRpcRequestOptions,
  ): Promise<unknown> {`,
  );
  shared = replaceExact(
    shared,
    'const validParams = helperParamsSchemas[method].parse(params);',
    'const validParams = paramsSchema.parse(params);',
  );
  shared = replaceExact(
    shared,
    'return new Promise<HelperResult<Method>>((resolve, reject) => {',
    'return new Promise<unknown>((resolve, reject) => {',
  );
  shared = replaceExact(
    shared,
    '        method,\n        activationConfiguration:',
    '        method,\n        resultSchema,\n        activationConfiguration:',
  );
  shared = replaceExact(
    shared,
    '        resolve: (result) => resolve(result as HelperResult<Method>),',
    '        resolve,',
  );
  const wrappers = `  request<Method extends HelperMethod>(
    session: HelperRpcSession,
    method: Method,
    params: HelperParams<Method>,
    options: HelperRpcRequestOptions,
  ): Promise<HelperResult<Method>> {
    return this.#request(
      session,
      method,
      params,
      helperParamsSchemas[method],
      helperResultSchemas[method],
      options,
    ) as Promise<HelperResult<Method>>;
  }

  requestAcceptance(
    session: HelperRpcSession,
    method: string,
    resultSchema: ZodType,
    options: HelperRpcRequestOptions,
  ): Promise<unknown> {
    return this.#request(session, method, {}, z.object({}).strict(), resultSchema, options);
  }

`;
  output = `${output.slice(0, start)}${wrappers}${shared}${output.slice(end)}`;
  output = replaceExact(
    output,
    'const result = helperResultSchemas[pending.method].safeParse(response.data.result);',
    'const result = pending.resultSchema.safeParse(response.data.result);',
  );
  return output;
}

function replaceExact(source: string, oldText: string, newText: string): string {
  const first = source.indexOf(oldText);
  if (first < 0)
    throw new Error(`Acceptance overlay source anchor is missing: ${oldText.slice(0, 80)}`);
  if (source.slice(first + oldText.length).includes(oldText)) {
    throw new Error(`Acceptance overlay source anchor is duplicated: ${oldText.slice(0, 80)}`);
  }
  return `${source.slice(0, first)}${newText}${source.slice(first + oldText.length)}`;
}
