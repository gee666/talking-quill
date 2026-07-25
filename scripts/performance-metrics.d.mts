export interface ProcessMemory {
  readonly role: string;
  readonly privateWorkingSetBytes: number;
  readonly grossWorkingSetBytes: number;
  readonly privateBytes: number;
}

export interface MemoryTotals {
  readonly privateWorkingSetBytes: number;
  readonly grossWorkingSetBytes: number;
  readonly privateBytes: number;
}

export interface RoleMemory extends MemoryTotals {
  readonly count: number;
}

export interface ProcessSummary extends MemoryTotals {
  readonly processCount: number;
  readonly roles: Readonly<Record<string, RoleMemory>>;
}

export function classifyWindowsProcess(name: string, commandLine?: string): string;
export function summarizeProcesses(processes: readonly ProcessMemory[]): ProcessSummary;
export function describeBytes(bytes: number): number;
export function range(values: readonly number[]): {
  readonly min: number;
  readonly median: number;
  readonly max: number;
};
