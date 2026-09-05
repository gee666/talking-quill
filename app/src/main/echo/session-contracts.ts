import type { HelperActivationContext } from '../../shared/helper/protocol';
import type { VoiceCommand } from '../../shared/schemas/commands';
import type {
  EchoAbortReason,
  EchoSessionSnapshot,
  PiFallbackCategory,
} from '../../shared/schemas/echo-session';
import type { ProcessingMode } from '../../shared/schemas/history';

export interface EchoSessionState extends EchoSessionSnapshot {
  readonly startedAt: number | null;
  readonly finalText: string | null;
  readonly insertionState: 'none' | 'pending' | 'cancel-requested' | 'committed';
  readonly fallbackReason: 'provider-error' | 'timeout' | null;
  readonly captureReady: boolean;
  readonly audioReady: boolean;
  /** Native activation identity retained only in the main process for later target-safe paste. */
  readonly activationContext: Readonly<HelperActivationContext> | null;
  readonly submitPending: boolean;
}

export type EchoSessionEvent =
  | {
      readonly type: 'shortcut-down';
      readonly sessionId: string;
      readonly alternate: boolean;
      readonly processingMode: ProcessingMode;
      readonly activationContext: Readonly<HelperActivationContext>;
      readonly now: number;
    }
  | { readonly type: 'hold-elapsed'; readonly now: number }
  | { readonly type: 'shortcut-up'; readonly now: number }
  | { readonly type: 'capture-started'; readonly preferredUnavailable?: boolean }
  | { readonly type: 'audio-started' }
  | { readonly type: 'keyboard-disconnected' }
  | { readonly type: 'level'; readonly rms: number; readonly elapsedMs: number }
  | {
      readonly type: 'submit';
      readonly source: 'silence' | 'enter' | 'shortcut' | 'stop' | 'duration-cap';
    }
  | { readonly type: 'transcribed'; readonly text: string; readonly smart: boolean }
  | {
      readonly type: 'voice-command-matched';
      readonly transcript: string;
      readonly command: VoiceCommand;
    }
  | { readonly type: 'smart-completed'; readonly text: string }
  | {
      readonly type: 'abort';
      readonly reason: EchoAbortReason;
      readonly fallbackCategory?: PiFallbackCategory;
    }
  | { readonly type: 'insertion-committed' }
  | { readonly type: 'insertion-cancelled' }
  | {
      readonly type: 'inserted';
      readonly copied: boolean;
      readonly indeterminate?: boolean;
    }
  | { readonly type: 'fail'; readonly message: string; readonly transcript?: string }
  | { readonly type: 'operational-failure'; readonly message: string }
  | { readonly type: 'reset' };

export type EchoSessionEffect =
  | { readonly type: 'start-capture' }
  | { readonly type: 'begin-extended-transcription' }
  | { readonly type: 'stop-and-transcribe' }
  | { readonly type: 'process-smart'; readonly text: string }
  | {
      readonly type: 'insert';
      readonly text: string;
      readonly activationContext: Readonly<HelperActivationContext>;
    }
  | { readonly type: 'teardown' };

export interface EchoTransition {
  readonly state: EchoSessionState;
  readonly effects: readonly EchoSessionEffect[];
}
