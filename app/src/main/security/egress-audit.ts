import { appendFileSync, writeFileSync } from 'node:fs';

export const EGRESS_CATEGORIES = ['provider', 'update', 'model-download'] as const;
export type EgressCategory = (typeof EGRESS_CATEGORIES)[number];
export type EgressObserver = (category: EgressCategory) => void;

export class EgressProofBlockedError extends Error {
  constructor(readonly category: EgressCategory) {
    super(`Deterministic egress proof blocked ${category} before socket I/O.`);
    this.name = 'EgressProofBlockedError';
  }
}

/**
 * Test-only categorized instrumentation. Records no URL, host, headers, body, transcript,
 * credential, model name, or response. It deliberately throws before socket I/O so deterministic
 * scenario tests cannot be mistaken for packet-capture evidence.
 */
export function createEgressProofObserver(path: string, enabled: boolean): EgressObserver {
  if (!enabled) return () => undefined;
  writeFileSync(path, '', { encoding: 'utf8', mode: 0o600 });
  return (category) => {
    appendFileSync(path, `${JSON.stringify({ schemaVersion: 1, category })}\n`, {
      encoding: 'utf8',
      mode: 0o600,
    });
    throw new EgressProofBlockedError(category);
  };
}
