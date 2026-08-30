export interface AcceptanceReservationPayload {
  readonly buildId: string;
  readonly requestNonce: string;
  readonly invocationId: string;
  readonly runWindow: Readonly<Record<string, number>>;
  readonly latestStartOffsetMs: number;
  readonly deadlineOffsetMs: number;
  readonly expiresAtMs: number;
}
export interface AcceptanceReservationRequest {
  readonly invocationId: string;
  readonly payload: AcceptanceReservationPayload;
}
export function reserveAcceptanceRequestNonces(
  stableTempRoot: string,
  requests: readonly AcceptanceReservationRequest[],
): Promise<Readonly<{ reservedCount: number }>>;
export function acceptanceReservationRecord(
  payload: AcceptanceReservationPayload,
): Readonly<Record<string, unknown>>;
export function acceptanceReplayLedgerPaths(
  stableTempRoot: string,
  buildId: string,
  requestNonce: string,
): Readonly<{ buildRoot: string; reserved: string; consumed: string }>;
