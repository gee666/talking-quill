import type { HelperNotification } from '../../shared/helper/protocol';
import type { ActivationTestState } from '../../shared/schemas/activation-test';
import {
  type DictationProfile,
  type DictationProfileCreate,
  type DictationProfilePatch,
} from '../../shared/schemas/dictation-profiles';
import {
  type EchoAbortReason,
  type EchoSessionSnapshot,
  type PiFallbackCategory,
} from '../../shared/schemas/echo-session';
import type { PublicSettingsPatch, Settings } from '../../shared/schemas/settings';
import { type ShortcutCaptureLeaseId } from '../../shared/schemas/shortcut-capture';
import { acceptHelperNotification } from './echo-session-activation';
import { EchoSessionContext, type EchoSessionContextOptions } from './echo-session-context';
import { initialize, shutdown } from './echo-session-lifecycle';
import { getSnapshot } from './echo-session-presentation';
import { abortSession } from './echo-session-operation';
import { cancel, stop } from './echo-session-transitions';
import { startShortcutCapture, stopShortcutCapture } from './echo-shortcut-leases';

export type {
  EchoHelperPort,
  EchoHistoryPort,
  EchoInsertionPort,
  EchoModelUseGrant,
  EchoRecordingPort,
  EchoWhisperPort,
  LegacySmartTranscriptProcessor,
  SmartTranscriptProcessor,
  VoiceCommandMatcherPort,
} from './echo-session-ports';
export { discardChunkPrefix } from './pcm-buffer';

export class EchoSessionController {
  readonly #context: EchoSessionContext;
  constructor(options: EchoSessionContextOptions) {
    this.#context = new EchoSessionContext(options);
  }
  get activationTestState(): ActivationTestState {
    return this.#context.activationTest.state;
  }
  get systemWakeRevalidationSafe(): boolean {
    return (
      !this.#context.disposed &&
      this.#context.state.phase === 'idle' &&
      !this.#context.activationTest.state.active &&
      !this.#context.profiles.shortcutCaptureActive
    );
  }
  get snapshot(): EchoSessionSnapshot {
    return getSnapshot(this.#context);
  }
  subscribe(listener: (snapshot: EchoSessionSnapshot) => void): () => void {
    this.#context.listeners.add(listener);
    return () => this.#context.listeners.delete(listener);
  }
  initialize(): Promise<void> {
    return initialize(this.#context);
  }
  startActivationTest(
    ownerWebContentsId: number,
    onDestroyed: (listener: () => void) => () => void,
  ): ActivationTestState {
    const unavailableReason = !this.#context.settings.get().app.enabled
      ? 'app-disabled'
      : this.#context.helper.readiness.status !== 'ready'
        ? 'helper-unavailable'
        : this.#context.state.phase !== 'idle'
          ? 'session-active'
          : null;
    return this.#context.activationTest.start(ownerWebContentsId, onDestroyed, unavailableReason);
  }
  stopActivationTest(ownerWebContentsId?: number): ActivationTestState {
    return this.#context.activationTest.stop(ownerWebContentsId);
  }
  startShortcutCapture(
    ownerWebContentsId: number,
    onDestroyed: (listener: () => void) => () => void,
  ): Promise<ShortcutCaptureLeaseId> {
    return startShortcutCapture(this.#context, ownerWebContentsId, onDestroyed);
  }
  stopShortcutCapture(ownerWebContentsId: number, leaseId: ShortcutCaptureLeaseId): Promise<void> {
    return stopShortcutCapture(this.#context, ownerWebContentsId, leaseId);
  }
  acceptHelperNotification(notification: HelperNotification): void {
    return acceptHelperNotification(this.#context, notification);
  }
  stop(): void {
    return stop(this.#context);
  }
  cancel(): void {
    return cancel(this.#context);
  }
  abort(reason: EchoAbortReason, fallbackCategory?: PiFallbackCategory): void {
    return abortSession(this.#context, reason, fallbackCategory);
  }
  readinessChanged(): void {
    this.#context.profiles.requestSync();
  }
  updateGeneral(patch: PublicSettingsPatch): Promise<Settings> {
    const nextEnabled = patch.app?.enabled ?? this.#context.settings.get().app.enabled;
    if (!nextEnabled) {
      if (this.#context.activationTest.state.active) this.#context.activationTest.stop();
      if (this.#context.state.phase !== 'idle') this.cancel();
    }
    return this.#context.profiles.updateGeneral(patch);
  }
  createProfile(input: DictationProfileCreate): Promise<Settings> {
    return this.#context.profiles.createProfile(input);
  }
  updateProfile(id: string, patch: DictationProfilePatch): Promise<Settings> {
    return this.#context.profiles.updateProfile(id, patch);
  }
  deleteProfile(id: string): Promise<Settings> {
    return this.#context.profiles.deleteProfile(id);
  }
  resetProfile(id: string): Promise<Settings> {
    return this.#context.profiles.resetProfile(id);
  }
  replaceProfiles(profiles: readonly DictationProfile[]): Promise<Settings> {
    return this.#context.profiles.replaceProfiles(profiles);
  }
  get dictationProfiles(): readonly DictationProfile[] {
    return this.#context.settings.get().dictationProfiles;
  }
  shutdown(): Promise<void> {
    return shutdown(this.#context);
  }
}
