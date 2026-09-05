import type { ChildProcessWithoutNullStreams, SpawnOptionsWithoutStdio } from './pi-rpc-operation';
import { piSpawnCommand } from './pi-process-runtime';
import { ProviderError } from './errors';
import type { PiCliIdentity } from './pi-executable';
import type { PiRpcExpectedState, PiRpcPrewarmOptions } from './pi-rpc-types';
import { validatePiRpcLimits } from './pi-rpc-transport';
import { noControls } from './pi-rpc-validation';

export const PI_RPC_PROTOCOL_VERSION = '0.84.3';
export const PI_RPC_SUPPORTED_VERSIONS = Object.freeze(['0.84.2', '0.84.3'] as const);
export const PI_RPC_REQUIRED_SAFETY_FLAGS = Object.freeze([
  '--no-tools',
  '--no-extensions',
  '--no-session',
  '--no-context-files',
  '--no-approve',
  '--no-skills',
  '--no-prompt-templates',
  '--no-themes',
  '--offline',
] as const);

const DEFAULT_OPERATION_TIMEOUT_MS = 120_000;
const DEFAULT_ABORT_GRACE_MS = 250;
const DEFAULT_RETIREMENT_GRACE_MS = 500;
const DEFAULT_TREE_TERMINATION_TIMEOUT_MS = 5_000;
const EXPECTED_ID = /^[A-Za-z0-9][A-Za-z0-9._:@+/-]{0,511}$/u;

export function createPiRpcArguments(
  identity: Pick<PiCliIdentity, 'packageVersion' | 'safetyFlags'>,
  expected: PiRpcExpectedState,
  explicitExtensions: readonly string[] = [],
): readonly string[] {
  assertRpcCompatibility(identity);
  const frozen = validateExpectedState(expected);
  if (explicitExtensions.length > 8) throw new ProviderError('INVALID_CONFIG');
  const extensionArgs: string[] = [];
  for (const extension of explicitExtensions) {
    if (
      extension.length === 0 ||
      extension.length > 512 ||
      extension.startsWith('-') ||
      !noControls(extension)
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    extensionArgs.push('-e', extension);
  }
  return Object.freeze([
    '--mode',
    'rpc',
    '--provider',
    frozen.provider,
    '--model',
    frozen.model,
    '--thinking',
    frozen.thinking,
    ...identity.safetyFlags,
    ...extensionArgs,
  ]);
}

export function assertRpcCompatibility(
  identity: Pick<PiCliIdentity, 'packageVersion' | 'safetyFlags'>,
): void {
  if (
    !PI_RPC_SUPPORTED_VERSIONS.some((version) => version === identity.packageVersion) ||
    identity.safetyFlags.length !== PI_RPC_REQUIRED_SAFETY_FLAGS.length ||
    !identity.safetyFlags.every((flag, index) => flag === PI_RPC_REQUIRED_SAFETY_FLAGS[index])
  ) {
    throw new ProviderError('PI_INCOMPATIBLE');
  }
}

export function validatePrewarmOptions(options: PiRpcPrewarmOptions): {
  readonly args: readonly string[];
  readonly expected: Readonly<PiRpcExpectedState>;
  readonly environment: NodeJS.ProcessEnv;
  readonly platform: NodeJS.Platform;
  readonly timeoutMs: number;
  readonly abortGraceMs: number;
  readonly retirementGraceMs: number;
  readonly treeTerminationTimeoutMs: number;
} {
  const expected = validateExpectedState(options.expected);
  validatePiRpcLimits(options.limits);
  const environment = options.environment ?? process.env;
  const platform = options.platform ?? process.platform;
  const timeoutMs = boundedDuration(options.timeoutMs ?? DEFAULT_OPERATION_TIMEOUT_MS, 600_000);
  const abortGraceMs = boundedDuration(options.abortGraceMs ?? DEFAULT_ABORT_GRACE_MS, 5_000);
  const retirementGraceMs = boundedDuration(
    options.retirementGraceMs ?? DEFAULT_RETIREMENT_GRACE_MS,
    5_000,
  );
  const treeTerminationTimeoutMs = boundedDuration(
    options.treeTerminationTimeoutMs ?? DEFAULT_TREE_TERMINATION_TIMEOUT_MS,
    10_000,
  );
  return Object.freeze({
    args: createPiRpcArguments(options.identity, expected, options.explicitExtensions),
    expected,
    environment,
    platform,
    timeoutMs,
    abortGraceMs,
    retirementGraceMs,
    treeTerminationTimeoutMs,
  });
}

function validateExpectedState(expected: PiRpcExpectedState): Readonly<PiRpcExpectedState> {
  if (
    !EXPECTED_ID.test(expected.provider) ||
    !EXPECTED_ID.test(expected.model) ||
    !['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(expected.thinking)
  ) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return Object.freeze({
    provider: expected.provider,
    model: expected.model,
    thinking: expected.thinking,
  });
}

function boundedDuration(value: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return value;
}

export function spawnPiRpc(
  options: PiRpcPrewarmOptions,
  validated: ReturnType<typeof validatePrewarmOptions>,
  spawn: NonNullable<PiRpcPrewarmOptions['spawnPi']>,
): ChildProcessWithoutNullStreams {
  const command = piSpawnCommand(
    options.identity.canonicalPath,
    validated.args,
    validated.environment,
    validated.platform,
  );
  try {
    return (options.spawnPi ?? spawn)(command.executable, command.args, {
      env: validated.environment,
      cwd: options.workingDirectory ?? process.cwd(),
      shell: false,
      windowsHide: true,
      windowsVerbatimArguments:
        validated.platform === 'win32' && /\.(?:cmd|bat)$/iu.test(options.identity.canonicalPath),
      detached: validated.platform !== 'win32',
      stdio: ['pipe', 'pipe', 'pipe'],
    } satisfies SpawnOptionsWithoutStdio);
  } catch {
    throw new ProviderError('PI_LAUNCH_FAILED', { fallbackEligible: true });
  }
}
