export const CLEANUP_TIMEOUT_MS: number;

export interface CleanupSpawnResult {
  readonly error?: Error;
  readonly signal: NodeJS.Signals | null;
  readonly status: number | null;
}

export interface CleanupSpawnOptions {
  readonly shell: false;
  readonly stdio: 'inherit';
  readonly timeout: number;
  readonly windowsHide: true;
}

export type CleanupSpawn = (
  executable: string,
  arguments_: readonly string[],
  options: CleanupSpawnOptions,
) => CleanupSpawnResult;

export function resolveWindowsPowerShell(
  environment: NodeJS.ProcessEnv,
  platform: NodeJS.Platform,
): string;

export function runWindowsBootstrapCleanup(
  forwardedArguments: readonly string[],
  options?: {
    readonly environment?: NodeJS.ProcessEnv;
    readonly platform?: NodeJS.Platform;
    readonly spawn?: CleanupSpawn;
    readonly timeoutMs?: number;
  },
): number;
