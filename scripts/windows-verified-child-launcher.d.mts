export interface VerifiedExecutableIdentity {
  readonly path: string;
  readonly sha256: string;
  readonly bytes: number;
  readonly arguments?: readonly string[];
}
export function verifiedChildArguments(
  bootstrap: VerifiedExecutableIdentity,
  child: VerifiedExecutableIdentity,
  timeoutMs: number,
): string[];
export function launchVerifiedChildSync(options: {
  readonly bootstrap: VerifiedExecutableIdentity;
  readonly child: VerifiedExecutableIdentity;
  readonly timeoutMs: number;
  readonly input: Buffer;
  readonly maxBuffer: number;
}): unknown;
export function launchVerifiedChild(options: {
  readonly bootstrap: VerifiedExecutableIdentity;
  readonly child: VerifiedExecutableIdentity;
  readonly timeoutMs: number;
}): unknown;
