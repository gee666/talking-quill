export function canonicalAcceptanceJson(value: unknown): string;
export function createSignedAcceptanceRequest(
  command: string,
  options: Readonly<Record<string, any>>,
): string;
export function createOneUseJsonChannel(
  pipeName: string,
  timeoutMs: number,
): {
  readonly listening: Promise<void>;
  readonly value: Promise<any>;
  readonly close: () => void;
};
export function runPackagedAcceptanceProbe(
  command: string,
  options: Readonly<Record<string, any>>,
): Promise<any>;
export function spawnPackagedProcess(
  executable: string,
  arguments_: readonly string[],
  timeoutMs: number,
): {
  readonly pid: number | undefined;
  readonly exited: Promise<number>;
  readonly terminateIfRunning: () => Promise<void>;
};
