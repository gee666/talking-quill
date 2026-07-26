import type { VoiceCommand } from '../../shared/schemas/commands';
import type { DictationProfile } from '../../shared/schemas/dictation-profiles';
import type { ProcessingMode } from '../../shared/schemas/history';
import type { Settings } from '../../shared/schemas/settings';
import type { SessionHistoryRecord } from '../history/session-history-mapper';
import type { EchoSessionState } from './session-reducer';
import type {
  EchoHistoryPort,
  FrozenSmartTranscriptSession,
  LegacySmartTranscriptProcessor,
  SmartTranscriptProcessor,
} from './echo-session-ports';

export class SessionOutcomeWriter {
  readonly #history: EchoHistoryPort | null;
  readonly #smart: SmartTranscriptProcessor | null;
  #historyWrittenSessionId: string | null = null;
  #historyAllowed = false;
  #providerId: string | null = null;
  #modelId: string | null = null;
  #voiceCommand: VoiceCommand | null = null;
  #smartSession: FrozenSmartTranscriptSession | null = null;
  #screenshotFilename: string | null = null;

  constructor(options: {
    readonly history: EchoHistoryPort | null;
    readonly smart: SmartTranscriptProcessor | null;
  }) {
    this.#history = options.history;
    this.#smart = options.smart;
  }

  get smartSession(): FrozenSmartTranscriptSession | null {
    return this.#smartSession;
  }

  beginSession(
    settings: Readonly<Settings>,
    profile: Readonly<DictationProfile>,
    processingMode: ProcessingMode,
  ): void {
    this.#historyWrittenSessionId = null;
    this.#historyAllowed = settings.privacy.historyEnabled;
    this.#voiceCommand = null;
    this.discardSmartSession();
    const selectedProviderId = settings.smartProcessing.selectedProviderId;
    this.#providerId = selectedProviderId;
    this.#modelId = settings.smartProcessing.providers[selectedProviderId]?.modelId ?? null;
    if (processingMode === 'smart' && this.#smart !== null) {
      try {
        this.#smartSession =
          'beginSession' in this.#smart
            ? this.#smart.beginSession(profile)
            : legacySmartSession(this.#smart, this.#providerId, this.#modelId);
        this.#providerId = this.#smartSession.providerId;
        this.#modelId = this.#smartSession.modelId;
      } catch {
        this.#smartSession = null;
      }
    }
  }

  revokeHistory(): void {
    this.#historyAllowed = false;
  }

  setVoiceCommand(command: VoiceCommand): void {
    this.#voiceCommand = command;
  }

  setScreenshotFilename(filename: string | null): void {
    this.#screenshotFilename = filename;
  }

  observeTransition(previous: EchoSessionState, next: EchoSessionState): void {
    if (
      (this.#history === null || !this.#historyAllowed) &&
      previous.phase !== next.phase &&
      (next.phase === 'restoringClipboard' || next.phase === 'completed')
    ) {
      this.discardSmartSession();
    }
    if (
      this.#history === null ||
      !this.#historyAllowed ||
      next.sessionId === null ||
      next.sessionId === this.#historyWrittenSessionId ||
      previous.phase === next.phase
    ) {
      return;
    }
    if (next.dictationMode === null || next.processingMode === null || next.transcript === null) {
      return;
    }
    let outcome: SessionHistoryRecord | null = null;
    if (
      this.#voiceCommand !== null &&
      (next.phase === 'restoringClipboard' || next.phase === 'completed')
    ) {
      outcome = {
        kind: 'voice-command',
        dictationMode: next.dictationMode,
        processingMode: next.processingMode,
        rawText: next.transcript,
        voiceTrigger: this.#voiceCommand.trigger,
        voiceSnippet: this.#voiceCommand.snippet,
      };
    } else if (next.phase === 'restoringClipboard' || next.phase === 'completed') {
      if (next.processingMode === 'raw') {
        outcome = {
          kind: 'raw-completed',
          dictationMode: next.dictationMode,
          processingMode: next.processingMode,
          rawText: next.transcript,
        };
      } else if (next.fallbackReason !== null) {
        outcome = {
          kind: 'smart-fallback',
          dictationMode: next.dictationMode,
          processingMode: next.processingMode,
          rawText: next.transcript,
          providerId: this.#providerId,
          modelId: this.#modelId,
          errorCategory: next.fallbackCategory ?? next.fallbackReason,
        };
      } else if (next.finalText !== null) {
        outcome = {
          kind: 'smart-completed',
          dictationMode: next.dictationMode,
          processingMode: next.processingMode,
          rawText: next.transcript,
          processedText: next.finalText,
          providerId: this.#providerId,
          modelId: this.#modelId,
          screenshotFilename: this.#screenshotFilename,
        };
      }
    } else if (next.phase === 'error') {
      outcome = {
        kind: 'error',
        dictationMode: next.dictationMode,
        processingMode: next.processingMode,
        rawText: next.transcript,
        errorCategory: 'dictation-error',
      };
    }
    if (outcome === null) return;
    this.#historyWrittenSessionId = next.sessionId;
    try {
      const stored = this.#history.record(outcome);
      if (stored && outcome.kind === 'smart-completed') this.#commitSmartSession();
      else this.discardSmartSession();
    } catch {
      this.discardSmartSession();
      // History is optional local persistence. It must never alter an insertion outcome.
    }
  }

  discardSmartSession(): void {
    const session = this.#takeSmartSession();
    if (session === null) return;
    try {
      session.cleanup();
    } catch {
      // Smart context and temporary screenshots are optional and never alter session completion.
    }
  }

  #commitSmartSession(): void {
    const session = this.#takeSmartSession();
    if (session === null) return;
    try {
      session.commitScreenshot();
    } catch {
      try {
        session.cleanup();
      } catch {
        // History is already committed. Artifact finalization remains best-effort.
      }
    }
  }

  #takeSmartSession(): FrozenSmartTranscriptSession | null {
    const session = this.#smartSession;
    this.#smartSession = null;
    this.#screenshotFilename = null;
    return session;
  }
}

function legacySmartSession(
  processor: LegacySmartTranscriptProcessor,
  providerId: string | null,
  modelId: string | null,
): FrozenSmartTranscriptSession {
  return {
    providerId: providerId ?? 'test-provider',
    modelId,
    prepare: () => Promise.resolve(),
    process: async (text, signal) => ({
      text: await processor.process(text, signal),
      screenshotFilename: null,
    }),
    commitScreenshot: () => undefined,
    cleanup: () => undefined,
  };
}
