export const PHYSICAL_OBSERVATION_WINDOW_MS: 60000;
export const PHYSICAL_TEARDOWN_ALLOWANCE_MS: 20000;
export const PHYSICAL_TOTAL_BOUND_MS: 80000;
export const HEARTBEAT_READINESS_WINDOW_MS: 120000;
export const MAX_ACCEPTANCE_RUN_MS: number;
export const ACCEPTANCE_MATRIX: readonly string[];
export interface AcceptancePhaseScheduleEntry {
  readonly phase: string;
  readonly latestStartOffsetMs: number;
  readonly deadlineOffsetMs: number;
}
export interface AcceptanceRequestScheduleEntry extends AcceptancePhaseScheduleEntry {
  readonly invocationId: string;
  readonly command: string;
}
export const ACCEPTANCE_PHASE_SCHEDULE: readonly AcceptancePhaseScheduleEntry[];
export const ACCEPTANCE_REQUEST_SCHEDULE: readonly AcceptanceRequestScheduleEntry[];
export class AcceptanceStoppedError extends Error {
  readonly evidence: unknown;
}
export interface FrozenArtifactInput {
  readonly installerPath: string;
  readonly installerSha256: string;
  readonly unpackedRoot: string;
  readonly metadataPath: string;
  readonly metadataSha256: string;
  readonly releaseIdentityPath?: string;
  readonly releaseIdentitySha256?: string;
  readonly validationEvidencePath?: string;
  readonly validationEvidenceSha256?: string;
}
export interface FrozenCandidateArtifactInput extends FrozenArtifactInput {
  readonly electronPath: string;
  readonly electronSha256: string;
  readonly appAsarPath: string;
  readonly appAsarSha256: string;
}
export function createInstalledAcceptancePlan(
  input: {
    readonly architecture: 'x64' | 'arm64';
    readonly artifacts: Readonly<{
      predecessor: FrozenArtifactInput;
      candidate: FrozenCandidateArtifactInput;
      fresh: FrozenArtifactInput;
      repair: FrozenArtifactInput;
      fault: FrozenArtifactInput;
    }>;
    readonly acceptance: {
      readonly buildId: string;
      readonly sourceRevision: string;
      readonly buildManifestPath: string;
      readonly buildManifestSha256: string;
      readonly manifestPublicKeySpkiBase64url: string;
      readonly runWindow: {
        readonly notBeforeMs: number;
        readonly expiresAtMs: number;
        readonly maxTotalRunMs: number;
      };
      readonly signedRequestsPath: string;
      readonly signedRequestsSha256: string;
      readonly syntheticSenderPath: string;
      readonly syntheticSenderSha256: string;
      readonly trustedLauncherPath: string;
      readonly trustedLauncherSha256: string;
      readonly syntheticSenderArguments?: readonly string[];
    };
    readonly outputPath?: string;
  },
  fileSystem?: unknown,
): Promise<Readonly<Record<string, unknown>>>;
export function executeInstalledAcceptance(
  plan: any,
  adapters: { readonly fileSystem: any; readonly runner: any },
  options?: { readonly dryRun?: boolean; readonly nowMs?: number | (() => number) },
): Promise<Readonly<Record<string, unknown>>>;
export function validateAcceptanceRunSequence(
  acceptance: unknown,
  nowMs?: number,
): Readonly<{
  buildId: string;
  runWindow: Readonly<Record<string, number>>;
  requests: readonly AcceptanceRequestScheduleEntry[];
}>;
export function validateAcceptancePhaseStart(
  runWindow: unknown,
  phase: string,
  nowMs: number,
): Readonly<Record<string, unknown>>;
export function nodeFileSystem(): unknown;
export function redactEvidence(
  value: Readonly<Record<string, unknown>>,
  key?: string,
): Record<string, unknown>;
export function createWindowsAcceptanceRunner(
  plan: unknown,
  osAdapter?: unknown,
  adapterFactory?: (acceptance: unknown) => unknown,
): unknown;
export function createWindowsOsAdapter(acceptance?: unknown): unknown;
export function startTrustedAcceptanceBroker(
  launcher: Readonly<{ path: string; bytes: number; sha256: string }>,
  dependencies?: unknown,
): Promise<unknown>;
export function externalTimeout(
  observationMs: number,
  teardownMs: number,
  operation: (
    signal: AbortSignal,
    markObservationStarted: () => void,
  ) => Promise<Record<string, unknown>>,
): Promise<Record<string, unknown>>;
export function runProductionPhase(
  phase: string,
  input: unknown,
  state: unknown,
  osAdapter: unknown,
): Promise<Record<string, unknown>>;
