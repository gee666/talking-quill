import type { AcceptanceReservationRequest } from './windows-acceptance-replay-ledger.mjs';

export interface AcceptancePreflightInput {
  readonly plan: any;
  readonly sequence: any;
  readonly nowMs: number;
  readonly reserveNonces?: boolean;
  readonly reserveReplayNonces?:
    | ((
        requests: readonly AcceptanceReservationRequest[],
      ) => Promise<Readonly<{ reservedCount: number }>>)
    | undefined;
}

export function verifyAcceptancePreflight(
  input: AcceptancePreflightInput,
): Promise<Readonly<Record<string, unknown>>>;
