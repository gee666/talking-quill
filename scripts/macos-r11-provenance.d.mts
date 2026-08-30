export interface MacosR11Run {
  readonly runId: string;
  readonly runAttempt: string;
  readonly headSha: string;
  readonly event: 'workflow_dispatch';
  readonly ref: string;
}

export interface MacosR11ProvenanceInput {
  readonly arch: 'x64' | 'arm64';
  readonly repository: string;
  readonly repositoryData: unknown;
  readonly candidateRunId: string;
  readonly baselineRunId: string;
  readonly candidateRun: unknown;
  readonly baselineRun: unknown;
  readonly candidateArtifacts: unknown;
  readonly baselineArtifacts: unknown;
  readonly baselineZipSha256: string;
}

export interface MacosR11Provenance {
  readonly schemaVersion: 1;
  readonly kind: 'macos-r11-authenticated-inputs';
  readonly repository: string;
  readonly workflowPath: '.github/workflows/release-unsigned.yml';
  readonly arch: 'x64' | 'arm64';
  readonly policy: { readonly event: 'workflow_dispatch'; readonly ref: string };
  readonly candidate: MacosR11Run & {
    readonly artifact: { readonly id: string; readonly name: string };
  };
  readonly baseline: MacosR11Run & {
    readonly artifact: { readonly id: string; readonly name: string };
    readonly acceptedZipSha256: string;
  };
}

export function authenticateMacosR11Provenance(input: MacosR11ProvenanceInput): MacosR11Provenance;
export function verifyRun(
  run: unknown,
  repository: string,
  defaultBranch: string,
  label: string,
): MacosR11Run;
export function verifyArtifact(
  response: unknown,
  name: string,
  runId: string,
): { readonly id: string; readonly name: string };
