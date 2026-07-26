import type { RecordingService } from '../audio/recording-service';
import type { HelperClient } from '../helper';
import type { SessionHistoryRecord } from '../history/session-history-mapper';
import type { InsertionService } from '../insertion/insertion-service';
import type {
  FrozenSmartTranscriptSession,
  SmartTranscriptProcessor as ProductionSmartTranscriptProcessor,
} from '../smart/smart-transcription-service';
import type {
  WhisperStreamingSession,
  WhisperWorkerClient,
} from '../transcription/whisper-worker-client';
import type { VoiceCommandMatch } from '../../shared/schemas/commands';
import type { ActivationBinding } from '../../shared/helper/protocol';

export type EchoHelperPort = Pick<
  HelperClient,
  | 'readiness'
  | 'subscribeNotifications'
  | 'subscribeReadiness'
  | 'setSessionCapture'
  | 'getFrontApp'
  | 'resetSessionCapture'
> & {
  configureActivation(enabled: boolean, bindings: readonly ActivationBinding[]): Promise<unknown>;
};

export type EchoRecordingPort = Pick<RecordingService, 'startDictation' | 'stopDictation'>;
export type EchoWhisperPort = Pick<WhisperWorkerClient, 'transcribe' | 'startSession'>;
export type EchoInsertionPort = Pick<InsertionService, 'insert'>;

export interface EchoHistoryPort {
  record(outcome: SessionHistoryRecord): boolean;
}

export interface EchoModelUseGrant {
  readonly status: { readonly state: string };
  release(): void;
}

export interface LegacySmartTranscriptProcessor {
  process(text: string, signal: AbortSignal): Promise<string>;
}

export type SmartTranscriptProcessor =
  ProductionSmartTranscriptProcessor | LegacySmartTranscriptProcessor;

export interface VoiceCommandMatcherPort {
  match(transcript: string): VoiceCommandMatch | null;
}

export type { FrozenSmartTranscriptSession, WhisperStreamingSession };
