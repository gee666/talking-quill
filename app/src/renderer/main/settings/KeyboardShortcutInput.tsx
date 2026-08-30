import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { ShortcutCaptureLeaseId } from '../../../shared/schemas/shortcut-capture';
import { shortcutPlatformPolicy } from '../../../shared/schemas/shortcut-platform-policy';
import {
  ShortcutSchema,
  shortcutModifiersEqual,
  type Shortcut,
  type ShortcutKey,
  type ShortcutModifiers,
} from '../../../shared/schemas/shortcut';
import { Input } from '../../design';
import { formatKeyboardShortcut } from '../format-keyboard-shortcut';
import {
  restoreShortcutCaptureLease,
  retryShortcutCaptureRestorations,
} from './shortcut-capture-restoration';

const MODIFIER_MISMATCH_ERROR = 'Hold the same modifiers for the whole shortcut.';

interface CaptureAttempt {
  readonly generation: number;
  readonly start: Promise<ShortcutCaptureLeaseId>;
  released: boolean;
  refocusRequested: boolean;
  restoration: Promise<void> | null;
}

export function KeyboardShortcutInput({
  label,
  shortcut,
  platform,
  disabled,
  error,
  onChange,
  onCaptureValidityChange,
}: {
  readonly label: string;
  readonly shortcut: Shortcut;
  readonly platform: string;
  readonly disabled: boolean;
  readonly error?: string | undefined;
  readonly onChange: (shortcut: Shortcut) => void;
  readonly onCaptureValidityChange: (valid: boolean) => void;
}) {
  const [captureError, setCaptureError] = useState<string | undefined>();
  const [captureState, setCaptureState] = useState<'idle' | 'preparing' | 'ready' | 'restoring'>(
    'idle',
  );
  const inputElement = useRef<HTMLInputElement>(null);
  const onChangeRef = useRef(onChange);
  const onCaptureValidityChangeRef = useRef(onCaptureValidityChange);
  const focused = useRef(false);
  const mounted = useRef(false);
  const captureAttempt = useRef<CaptureAttempt | null>(null);
  const captureGeneration = useRef(0);
  const captureOriginal = useRef<Shortcut | null>(null);
  const acceptedCandidate = useRef(false);
  const heldLetters = useRef<ShortcutKey[]>([]);
  const sequenceKeys = useRef<ShortcutKey[]>([]);
  const sequenceModifiers = useRef<ShortcutModifiers | null>(null);
  const sequenceFenced = useRef(false);
  const sequenceClosed = useRef(false);
  const sequenceFenceGuidance = useRef<string | null>(null);

  useEffect(() => {
    onChangeRef.current = onChange;
    onCaptureValidityChangeRef.current = onCaptureValidityChange;
  }, [onCaptureValidityChange, onChange]);

  const resetHeldSequence = useCallback(() => {
    heldLetters.current = [];
    sequenceKeys.current = [];
    sequenceModifiers.current = null;
    sequenceFenced.current = false;
    sequenceClosed.current = false;
    sequenceFenceGuidance.current = null;
  }, []);

  const restoreOriginalShortcut = useCallback(() => {
    if (!acceptedCandidate.current || captureOriginal.current === null) return;
    acceptedCandidate.current = false;
    onChangeRef.current(structuredClone(captureOriginal.current));
  }, []);

  const resetTransient = useCallback(() => {
    resetHeldSequence();
    setCaptureError(undefined);
  }, [resetHeldSequence]);

  const releaseCapture = useCallback(
    (reportFailure: boolean, generation: number) => {
      const attempt = captureAttempt.current;
      if (attempt === null || attempt.released || attempt.generation !== generation) {
        return;
      }
      attempt.released = true;
      if (reportFailure) setCaptureState('restoring');
      const restoration = restoreShortcutCaptureLease(attempt.start);
      attempt.restoration = restoration;
      void restoration.then(
        () => {
          if (captureAttempt.current === attempt) captureAttempt.current = null;
          if (!mounted.current || captureGeneration.current !== generation) return;
          captureOriginal.current = null;
          acceptedCandidate.current = false;
          setCaptureState('idle');
          if (reportFailure) {
            setCaptureError(undefined);
            onCaptureValidityChangeRef.current(true);
          }
        },
        () => {
          attempt.released = false;
          attempt.refocusRequested = false;
          attempt.restoration = null;
          if (!reportFailure || !mounted.current || captureGeneration.current !== generation)
            return;
          restoreOriginalShortcut();
          setCaptureState('idle');
          setCaptureError(
            'Your shortcuts couldn’t be switched back on. Your previous shortcut was kept. Click this field again, then press Tab to retry the same capture lease.',
          );
          onCaptureValidityChangeRef.current(false);
        },
      );
    },
    [restoreOriginalShortcut],
  );

  const exitCapture = useCallback(() => {
    focused.current = false;
    const generation = captureGeneration.current;
    resetHeldSequence();
    releaseCapture(true, generation);
  }, [releaseCapture, resetHeldSequence]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      focused.current = false;
      const generation = captureGeneration.current;
      captureGeneration.current += 1;
      resetHeldSequence();
      releaseCapture(false, generation);
    };
  }, [releaseCapture, resetHeldSequence]);

  useEffect(() => {
    const onWindowBlur = () => {
      if (!focused.current && captureAttempt.current === null) return;
      inputElement.current?.blur();
      if (focused.current || captureAttempt.current !== null) exitCapture();
    };
    window.addEventListener('blur', onWindowBlur);
    return () => window.removeEventListener('blur', onWindowBlur);
  }, [exitCapture]);

  useEffect(() => {
    if (!disabled) return;
    focused.current = false;
    const generation = captureGeneration.current;
    resetHeldSequence();
    inputElement.current?.blur();
    queueMicrotask(() => {
      if (!mounted.current) return;
      releaseCapture(true, generation);
    });
  }, [disabled, releaseCapture, resetHeldSequence]);

  const rejectSequence = useCallback(
    (message: string) => {
      sequenceFenced.current = true;
      sequenceFenceGuidance.current = message;
      restoreOriginalShortcut();
      setCaptureError(message);
    },
    [restoreOriginalShortcut],
  );

  const preventCommand = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.code === 'Tab') return false;
    event.preventDefault();
    event.stopPropagation();
    return true;
  };

  const fenceChangedModifiers = (event: KeyboardEvent<HTMLInputElement>) => {
    if (
      sequenceModifiers.current === null ||
      heldLetters.current.length === 0 ||
      shortcutModifiersEqual(sequenceModifiers.current, modifiersFromEvent(event))
    ) {
      return false;
    }
    rejectSequence(MODIFIER_MISMATCH_ERROR);
    return true;
  };

  const acquireCapture = useCallback(() => {
    focused.current = true;
    resetTransient();
    captureOriginal.current = structuredClone(shortcut);
    acceptedCandidate.current = false;
    const generation = captureGeneration.current + 1;
    captureGeneration.current = generation;
    setCaptureState('preparing');
    onCaptureValidityChangeRef.current(false);
    const attempt: CaptureAttempt = {
      generation,
      start: retryShortcutCaptureRestorations().then(() =>
        window.talkingQuill.shortcutCapture.start(),
      ),
      released: false,
      refocusRequested: false,
      restoration: null,
    };
    captureAttempt.current = attempt;
    void attempt.start.then(
      () => {
        if (
          captureAttempt.current === attempt &&
          !attempt.released &&
          focused.current &&
          captureGeneration.current === attempt.generation
        ) {
          setCaptureState('ready');
          setCaptureError(undefined);
          onCaptureValidityChangeRef.current(true);
        }
      },
      () => {
        if (
          captureAttempt.current !== attempt ||
          attempt.released ||
          !focused.current ||
          captureGeneration.current !== attempt.generation
        ) {
          return;
        }
        attempt.released = true;
        captureAttempt.current = null;
        resetHeldSequence();
        setCaptureState('idle');
        setCaptureError(
          'Talking Quill can’t read your keys right now. Your previous shortcut was kept. Try again.',
        );
        onCaptureValidityChangeRef.current(false);
      },
    );
  }, [resetHeldSequence, resetTransient, shortcut]);

  const beginCapture = useCallback(() => {
    if (disabled) return;
    const pending = captureAttempt.current;
    if (pending === null) {
      acquireCapture();
      return;
    }
    focused.current = true;
    releaseCapture(true, captureGeneration.current);
    if (pending.refocusRequested || pending.restoration === null) return;
    pending.refocusRequested = true;
    void pending.restoration.then(
      () => {
        pending.refocusRequested = false;
        if (focused.current && captureAttempt.current === null) acquireCapture();
      },
      () => {
        pending.refocusRequested = false;
      },
    );
  }, [acquireCapture, disabled, releaseCapture]);

  const retryCapture = useCallback(() => {
    if (captureError === undefined) return;
    const attempt = captureAttempt.current;
    if (attempt === null || (!attempt.released && attempt.restoration === null)) beginCapture();
  }, [beginCapture, captureError]);

  return (
    <>
      <Input
        ref={inputElement}
        label={label}
        value={formatKeyboardShortcut(shortcut, platform)}
        disabled={disabled}
        readOnly
        aria-busy={captureState === 'preparing' || captureState === 'restoring' ? true : undefined}
        spellCheck={false}
        error={captureError ?? error}
        onFocus={beginCapture}
        onClick={retryCapture}
        onBlur={exitCapture}
        onKeyDownCapture={(event) => {
          if (!preventCommand(event)) return;
          if (captureState !== 'ready') return;
          if (event.repeat || event.nativeEvent.isComposing) return;
          if (isModifierOnly(event.code)) {
            if (heldLetters.current.length === 0 && sequenceFenced.current) resetHeldSequence();
            if (fenceChangedModifiers(event) || sequenceFenced.current) {
              setCaptureError(MODIFIER_MISMATCH_ERROR);
            }
            return;
          }
          const key = shortcutKeyFromCode(event.code);
          if (key === null) {
            rejectSequence('Use only the letters A to Z.');
            return;
          }
          if (event.getModifierState('AltGraph')) {
            rejectSequence('Use physical Ctrl and Alt instead of AltGr.');
            return;
          }
          if (heldLetters.current.includes(key)) return;
          if (sequenceKeys.current.includes(key)) {
            rejectSequence('Use each letter only once.');
            return;
          }
          if (sequenceClosed.current) {
            rejectSequence('Hold each earlier letter while pressing the next.');
            return;
          }

          heldLetters.current = [...heldLetters.current, key];
          sequenceKeys.current = [...sequenceKeys.current, key];
          const modifiers = modifiersFromEvent(event);
          if (sequenceFenced.current) {
            setCaptureError(sequenceFenceGuidance.current ?? 'Release the letters and try again.');
            return;
          }
          if (sequenceModifiers.current === null) {
            if (!hasModifier(modifiers)) {
              rejectSequence('Add Ctrl, Alt, Shift, or the Windows key.');
              return;
            }
            sequenceModifiers.current = modifiers;
          } else if (!shortcutModifiersEqual(sequenceModifiers.current, modifiers)) {
            rejectSequence(MODIFIER_MISMATCH_ERROR);
            return;
          }

          const parsed = ShortcutSchema.safeParse({
            modifiers: sequenceModifiers.current,
            keys: [...sequenceKeys.current],
          });
          if (!parsed.success) {
            rejectSequence('Enter a valid shortcut.');
            return;
          }
          const policy = shortcutPlatformPolicy(parsed.data, platform);
          if (policy.status === 'impossible') {
            rejectSequence(policy.message);
            return;
          }

          acceptedCandidate.current = true;
          setCaptureError(undefined);
          onCaptureValidityChange(true);
          onChange(parsed.data);
        }}
        onKeyUpCapture={(event) => {
          if (!preventCommand(event)) return;
          if (isModifierOnly(event.code)) {
            fenceChangedModifiers(event);
            return;
          }
          const key = shortcutKeyFromCode(event.code);
          if (key === null || !heldLetters.current.includes(key)) {
            if (sequenceFenced.current && heldLetters.current.length === 0) resetHeldSequence();
            return;
          }
          heldLetters.current = heldLetters.current.filter((held) => held !== key);
          if (heldLetters.current.length === 0) {
            resetHeldSequence();
          } else {
            sequenceClosed.current = true;
          }
        }}
      />
      <span className="sr-only" role="status" aria-live="polite" aria-atomic="true">
        {captureError ?? error}
      </span>
    </>
  );
}

function modifiersFromEvent(event: KeyboardEvent<HTMLInputElement>): ShortcutModifiers {
  return {
    ctrl: event.ctrlKey,
    alt: event.altKey,
    shift: event.shiftKey,
    meta: event.metaKey,
  };
}

function hasModifier(modifiers: ShortcutModifiers): boolean {
  return modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.meta;
}

function shortcutKeyFromCode(code: string): ShortcutKey | null {
  return /^Key[A-Z]$/.test(code) ? (code.slice(3) as ShortcutKey) : null;
}

function isModifierOnly(code: string): boolean {
  return [
    'AltLeft',
    'AltRight',
    'ShiftLeft',
    'ShiftRight',
    'ControlLeft',
    'ControlRight',
    'MetaLeft',
    'MetaRight',
  ].includes(code);
}
