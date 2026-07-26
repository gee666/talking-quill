export function parseTcpdumpInterfaces(source: string): readonly string[];
export function selectMacCaptureInterface(
  interfaces: readonly string[],
  requested?: string,
  scope?: string,
): string;
export function macTcpdumpArguments(
  output: string,
  interfaces: readonly string[],
  requested: string,
  scope: string,
): readonly string[];
export interface CaptureInvocation {
  readonly command: string;
  readonly commandArgs: readonly string[];
  readonly output: string;
  readonly tmpRoot: string;
  readonly requestedInterface: string | null;
  readonly scope: string;
}
export function parseCaptureInvocation(
  arguments_: readonly string[],
  root?: string,
): CaptureInvocation;
export function validatePcapBytes(bytes: Buffer): void;
