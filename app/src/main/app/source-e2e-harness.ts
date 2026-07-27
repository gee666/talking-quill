declare const __TALKING_QUILL_TASK6_TEST_HARNESS__: boolean;
declare const __TALKING_QUILL_VOCABULARY_TEST_HARNESS__: boolean;
declare const __TALKING_QUILL_PI_TEST_HARNESS__: boolean;

import type {
  EchoHelperPort,
  EchoInsertionPort,
  EchoRecordingPort,
  EchoSessionController,
  EchoWhisperPort,
} from '../echo/echo-session-controller';
import type { HistoryStore } from '../persistence/history-store';
import type { SettingsStore } from '../persistence/settings-store';
import type { PiProviderOptions } from '../providers/pi';
import type { RecordingService } from '../audio/recording-service';
import type { VocabularyDialogPort } from '../vocabulary/file-service';

export interface SourceTask6Composition {
  readonly helper: EchoHelperPort;
  readonly recording: EchoRecordingPort;
  readonly whisper: EchoWhisperPort;
  readonly insertion: EchoInsertionPort;
  readonly welcome: {
    readonly microphone: boolean;
    readonly model: boolean;
  };
  readonly driver: unknown;
  startPackagedMedia(): void;
  bind(echo: EchoSessionController): void;
}

/** Keeps source-only and instrumented-package harness policy outside the production composition. */
export class SourceE2EHarness {
  readonly #isPackaged: boolean;
  readonly #environment: NodeJS.ProcessEnv;
  readonly #argv: readonly string[];

  constructor(options: {
    readonly isPackaged: boolean;
    readonly environment: NodeJS.ProcessEnv;
    readonly argv: readonly string[];
  }) {
    this.#isPackaged = options.isPackaged;
    this.#environment = options.environment;
    this.#argv = options.argv;
  }

  loadVocabularyDialogs(root: string): Promise<VocabularyDialogPort> | undefined {
    if (!__TALKING_QUILL_VOCABULARY_TEST_HARNESS__ || !this.#vocabularyRuntimeEnabled()) {
      return undefined;
    }
    return import('../vocabulary/source-test-dialogs').then(
      ({ createSourceTestVocabularyDialogs }) => createSourceTestVocabularyDialogs(root),
    );
  }

  piResolverOverride(): PiProviderOptions['resolveCli'] | undefined {
    return __TALKING_QUILL_PI_TEST_HARNESS__ &&
      this.#environment.TALKING_QUILL_PI_TEST_UNAVAILABLE === '1'
      ? () => Promise.reject(new Error('Pi unavailable test resolver'))
      : undefined;
  }

  loadTask6(options: {
    readonly history: HistoryStore;
    readonly settings: SettingsStore;
    readonly recording: RecordingService;
  }): Promise<SourceTask6Composition> | null {
    if (!__TALKING_QUILL_TASK6_TEST_HARNESS__ || !this.#task6RuntimeEnabled()) return null;
    return import('../../../../tests/e2e/support/task6-test-composition').then(
      ({ createTask6TestComposition }) =>
        createTask6TestComposition(
          options.history,
          options.settings,
          this.#argv.includes('--talking-quill-task6-real-media') ? options.recording : undefined,
        ),
    );
  }

  testNow(): (() => number) | null {
    if (!__TALKING_QUILL_TASK6_TEST_HARNESS__ || !this.#task6RuntimeEnabled()) return null;
    const argument = this.#argv.find((value) => value.startsWith('--talking-quill-test-now='));
    if (argument === undefined) return null;
    const value = Number(argument.slice('--talking-quill-test-now='.length));
    if (!Number.isSafeInteger(value) || value < 0) throw new Error('Invalid test clock');
    return () => value;
  }

  bindAndExposeTask6(composition: SourceTask6Composition, echo: EchoSessionController): () => void {
    if (!__TALKING_QUILL_TASK6_TEST_HARNESS__) return () => undefined;
    composition.bind(echo);
    const testDriverSymbol = Symbol.for('talking-quill:task6-test-driver');
    Reflect.set(globalThis, testDriverSymbol, composition.driver);
    return () => {
      Reflect.deleteProperty(globalThis, testDriverSymbol);
    };
  }

  createPackagedMediaReady(
    composition: SourceTask6Composition | null,
  ): ((role: 'capture' | 'widget') => void) | undefined {
    if (!__TALKING_QUILL_TASK6_TEST_HARNESS__) return undefined;
    if (
      composition === null ||
      !this.#isPackaged ||
      this.#environment.TALKING_QUILL_PACKAGED_MEDIA_HARNESS !== '1' ||
      !this.#argv.includes('--talking-quill-task6-real-media')
    ) {
      return undefined;
    }
    const readyRoles = new Set<'capture' | 'widget'>();
    let activated = false;
    return (role) => {
      readyRoles.add(role);
      if (readyRoles.size < 2 || activated) return;
      activated = true;
      composition.startPackagedMedia();
    };
  }

  #vocabularyRuntimeEnabled(): boolean {
    return (
      !this.#isPackaged &&
      this.#environment.NODE_ENV === 'test' &&
      this.#argv.includes('--talking-quill-vocabulary-test')
    );
  }

  #task6RuntimeEnabled(): boolean {
    const requested =
      this.#argv.includes('--talking-quill-task6-test') ||
      this.#argv.includes('--talking-quill-task6-real-media');
    return (
      requested &&
      ((!this.#isPackaged && this.#environment.NODE_ENV === 'test') ||
        (this.#isPackaged && this.#environment.TALKING_QUILL_PACKAGED_MEDIA_HARNESS === '1'))
    );
  }
}
