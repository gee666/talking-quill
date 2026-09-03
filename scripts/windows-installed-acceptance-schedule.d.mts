export const ACCEPTANCE_FAULT_PHASES: readonly string[];
export const ACCEPTANCE_MATRIX: readonly string[];
export const ACCEPTANCE_REQUEST_SCHEDULE: readonly Readonly<{
  invocationId: string;
  command: string;
  latestStartOffsetMs: number;
  deadlineOffsetMs: number;
}>[];
export const ACCEPTANCE_PHASE_SCHEDULE: readonly Readonly<{
  phase: string;
  deadlineOffsetMs: number;
}>[];
export const MAX_ACCEPTANCE_REQUEST_MS: number;
export const MAX_ACCEPTANCE_RUN_MS: number;
