export function canonicalAcceptanceJson(value: unknown): string;
export function createSignedAcceptanceRequest(
  command: string,
  options: Readonly<Record<string, any>>,
): string;
export function runPackagedAcceptanceProbe(
  command: string,
  options: Readonly<Record<string, any>>,
): Promise<any>;
export function spawnPackagedProcess(
  executable: string,
  arguments_: readonly string[],
  timeoutMs: number,
  startupFrame?: Buffer,
): {
  readonly pid: number | undefined;
  readonly exited: Promise<number>;
  readonly terminateIfRunning: () => Promise<void>;
};
