export function createProducerArtifactSetIdentity(payload: unknown): string;
export function producerArtifactSetPayloadFromPlan(
  plan: any,
  payloadEntries: readonly Readonly<{ path: string; bytes: number; sha256: string }>[],
): Readonly<Record<string, unknown>>;
