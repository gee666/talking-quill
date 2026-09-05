import type { VoiceCommand } from '../../shared/schemas/commands';
import type { DictationProfile } from '../../shared/schemas/dictation-profiles';

export interface SmartProcessingResult {
  readonly text: string;
  readonly screenshotFilename: string | null;
  readonly voiceCommand?: VoiceCommand | null;
}

export interface FrozenSmartTranscriptSession {
  readonly providerId: string;
  readonly modelId: string | null;
  /** Starts provider-only work that is safe before the user submits any transcript. */
  prepareForListening?(signal: AbortSignal): Promise<void>;
  /** Prepares submit-time context and joins any provider preparation already in flight. */
  prepare(signal: AbortSignal): Promise<void>;
  process(text: string, signal: AbortSignal): Promise<SmartProcessingResult>;
  commitScreenshot(): void;
  cleanup(): void;
}

export interface SmartTranscriptProcessor {
  beginSession(profile?: Readonly<DictationProfile>): FrozenSmartTranscriptSession;
}

export interface RetainedScreenshotHandle {
  readonly filename: string;
  cleanup(): void;
}
