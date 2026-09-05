import type { ZodType } from 'zod';
import type { DictationProfile } from '../../shared/schemas/dictation-profiles';
import type { HelperClient } from '../helper';
import type { AcceptanceRunRequestPayload } from './authorization-schema';

export interface InstalledAcceptanceHelper extends HelperClient {
  requestAcceptance(method: string, resultSchema: ZodType, timeoutMs: number): Promise<unknown>;
}

export interface InstalledObservationRequest {
  readonly command: AcceptanceRunRequestPayload['command'];
  readonly heartbeatDurationMs: 6_250 | 120_000;
  readonly pipeName: string;
  readonly launchCorrelation: string;
  readonly physicalObservation: boolean;
  readonly automationValidation: boolean;
  readonly automationArmedPipe: string | null;
  readonly automationCase: string | null;
  readonly expectedUserDataRoot: string | null;
}

export interface InstalledObservationContext {
  readonly profiles: readonly DictationProfile[];
  readonly persistentWindowRolesReady: boolean;
  readonly userDataRoot: string;
  readonly showValidationWidget: () => Promise<boolean>;
  readonly hideValidationWidget: () => void;
  readonly windowsLoginStart: boolean;
  readonly mainWindowVisible: boolean;
  readonly waitForIgnoredLoginStart: (timeoutMs: number) => Promise<boolean>;
  readonly probeDiagnostics: () => Promise<{
    readonly enabled: boolean;
    readonly injectedFailureContained: boolean;
  }>;
}

export type ObservationWriter = (pipeName: string, value: unknown) => Promise<void>;
