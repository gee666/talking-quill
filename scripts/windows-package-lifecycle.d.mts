export const DIAGNOSTIC_BYTE_LIMIT: number;
export function redactLifecycleDiagnostic(text: unknown, paths?: string[]): string;
export function boundedDiagnosticCapture(limit?: number): {
  append(chunk: Buffer | string): void;
  snapshot(): { text: string; bytes: number; truncated: boolean };
};
