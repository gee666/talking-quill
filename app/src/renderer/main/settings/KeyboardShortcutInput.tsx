import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from 'react';
import {
  shortcutModifiersEqual,
  type Shortcut,
  type ShortcutKey,
  type ShortcutModifiers,
} from '../../../shared/schemas/shortcut';
import { Input } from '../../design';
import { formatKeyboardShortcut } from '../format-keyboard-shortcut';

const ALT_X_P_EXAMPLE: Shortcut = {
  modifiers: { ctrl: false, alt: true, shift: false, meta: false },
  keys: ['X', 'P'],
};
const CTRL_SHIFT_P_EXAMPLE: Shortcut = {
  modifiers: { ctrl: true, alt: false, shift: true, meta: false },
  keys: ['P'],
};
const MODIFIER_MISMATCH_GUIDANCE =
  'Keep the exact same modifiers held for every letter. Release all letter keys and start again. Your saved shortcut is unchanged.';

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
  const [captureGuidance, setCaptureGuidance] = useState<string | undefined>();
  const [captureState, setCaptureState] = useState<'idle' | 'preparing' | 'ready'>('idle');
  const inputElement = useRef<HTMLInputElement>(null);
  const focused = useRef(false);
  const mounted = useRef(false);
  const captureRequested = useRef(false);
  const captureGeneration = useRef(0);
  const heldLetters = useRef<ShortcutKey[]>([]);
  const sequenceModifiers = useRef<ShortcutModifiers | null>(null);
  const sequenceFenced = useRef(false);
  const sequenceFenceGuidance = useRef<string | null>(null);

  const resetHeldSequence = useCallback(() => {
    heldLetters.current = [];
    sequenceModifiers.current = null;
    sequenceFenced.current = false;
    sequenceFenceGuidance.current = null;
  }, []);

  const resetTransient = useCallback(() => {
    resetHeldSequence();
    setCaptureGuidance(undefined);
    setCaptureError(undefined);
  }, [resetHeldSequence]);

  const releaseCapture = useCallback(
    (reportFailure: boolean) => {
      if (!captureRequested.current) return;
      captureRequested.current = false;
      void window.talkingQuill.shortcutCapture.stop().then(
        () => {
          if (mounted.current) onCaptureValidityChange(true);
        },
        () => {
          if (!reportFailure || !mounted.current) return;
          setCaptureError(
            'Global shortcuts could not be restored. Refocus this field, then Tab away to retry.',
          );
          onCaptureValidityChange(false);
        },
      );
    },
    [onCaptureValidityChange],
  );

  const exitCapture = useCallback(() => {
    focused.current = false;
    captureGeneration.current += 1;
    resetTransient();
    setCaptureState('idle');
    releaseCapture(true);
  }, [releaseCapture, resetTransient]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      focused.current = false;
      captureGeneration.current += 1;
      resetHeldSequence();
      releaseCapture(false);
    };
  }, [releaseCapture, resetHeldSequence]);

  useEffect(() => {
    const onWindowBlur = () => {
      if (!focused.current && !captureRequested.current) return;
      inputElement.current?.blur();
      if (focused.current || captureRequested.current) exitCapture();
    };
    window.addEventListener('blur', onWindowBlur);
    return () => window.removeEventListener('blur', onWindowBlur);
  }, [exitCapture]);

  useEffect(() => {
    if (!disabled) return;
    focused.current = false;
    captureGeneration.current += 1;
    resetHeldSequence();
    inputElement.current?.blur();
    releaseCapture(true);
    queueMicrotask(() => {
      if (!mounted.current) return;
      resetTransient();
      setCaptureState('idle');
    });
  }, [disabled, releaseCapture, resetHeldSequence, resetTransient]);

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
    sequenceFenced.current = true;
    sequenceFenceGuidance.current = MODIFIER_MISMATCH_GUIDANCE;
    setCaptureGuidance(MODIFIER_MISMATCH_GUIDANCE);
    return true;
  };

  const captureHint =
    captureState === 'preparing'
      ? 'Preparing shortcut capture…'
      : (captureGuidance ??
        `Focus this field, hold one or more modifiers unchanged, then press letters in order—for example ${formatKeyboardShortcut(ALT_X_P_EXAMPLE, platform)} or ${formatKeyboardShortcut(CTRL_SHIFT_P_EXAMPLE, platform)}. The final letter is the trigger. Tab moves away.`);

  return (
    <>
      <Input
        ref={inputElement}
        label={label}
        value={formatKeyboardShortcut(shortcut, platform)}
        disabled={disabled}
        readOnly
        aria-busy={captureState === 'preparing' ? true : undefined}
        spellCheck={false}
        hint={captureHint}
        error={captureError ?? error}
        onFocus={() => {
          if (disabled) return;
          focused.current = true;
          resetTransient();
          const generation = captureGeneration.current + 1;
          captureGeneration.current = generation;
          captureRequested.current = true;
          setCaptureState('preparing');
          void window.talkingQuill.shortcutCapture.start().then(
            () => {
              if (focused.current && captureGeneration.current === generation) {
                setCaptureState('ready');
                setCaptureError(undefined);
                onCaptureValidityChange(true);
              } else if (!focused.current && captureRequested.current) {
                releaseCapture(false);
              }
            },
            () => {
              if (!focused.current || captureGeneration.current !== generation) return;
              resetHeldSequence();
              setCaptureState('idle');
              setCaptureGuidance(undefined);
              setCaptureError('Shortcut capture is unavailable. Try again.');
              onCaptureValidityChange(false);
            },
          );
        }}
        onBlur={exitCapture}
        onKeyDownCapture={(event) => {
          if (!preventCommand(event)) return;
          if (captureState !== 'ready') {
            setCaptureGuidance('Shortcut capture is still preparing. Try again.');
            return;
          }
          if (event.repeat || event.nativeEvent.isComposing) return;
          if (isModifierOnly(event.code)) {
            if (fenceChangedModifiers(event) || sequenceFenced.current) {
              setCaptureGuidance(MODIFIER_MISMATCH_GUIDANCE);
            } else {
              setCaptureGuidance(
                'Keep at least one modifier held, then press one or more letters.',
              );
            }
            return;
          }
          const key = shortcutKeyFromCode(event.code);
          if (key === null) {
            setCaptureGuidance(
              'Only physical letter keys A–Z can be part of a shortcut chord. Your saved shortcut is unchanged.',
            );
            return;
          }
          if (heldLetters.current.includes(key)) return;
          heldLetters.current = [...heldLetters.current, key];
          const modifiers = modifiersFromEvent(event);
          if (sequenceFenced.current) {
            setCaptureGuidance(
              sequenceFenceGuidance.current ??
                'Release all letter keys, then start the shortcut chord again. Your saved shortcut is unchanged.',
            );
            return;
          }
          if (sequenceModifiers.current === null) {
            if (!hasModifier(modifiers)) {
              const guidance =
                'The first letter must be pressed with Ctrl, Alt, Shift, or Win on Windows—or Control, Option, Shift, or Command on macOS. Release all letter keys and start again. Your saved shortcut is unchanged.';
              sequenceFenced.current = true;
              sequenceFenceGuidance.current = guidance;
              setCaptureGuidance(guidance);
              return;
            }
            sequenceModifiers.current = modifiers;
          } else if (!shortcutModifiersEqual(sequenceModifiers.current, modifiers)) {
            sequenceFenced.current = true;
            sequenceFenceGuidance.current = MODIFIER_MISMATCH_GUIDANCE;
            setCaptureGuidance(MODIFIER_MISMATCH_GUIDANCE);
            return;
          }
          const candidate: Shortcut = {
            modifiers,
            keys: [...heldLetters.current],
          };
          setCaptureError(undefined);
          setCaptureGuidance(
            `${formatKeyboardShortcut(candidate, platform)} captured. ${key} is the final trigger.`,
          );
          onCaptureValidityChange(true);
          onChange(candidate);
        }}
        onKeyUpCapture={(event) => {
          if (!preventCommand(event)) return;
          if (isModifierOnly(event.code)) {
            fenceChangedModifiers(event);
            return;
          }
          const key = shortcutKeyFromCode(event.code);
          if (key !== null) {
            heldLetters.current = heldLetters.current.filter((held) => held !== key);
            if (heldLetters.current.length === 0) resetHeldSequence();
          }
        }}
      />
      <span className="sr-only" role="status" aria-live="polite" aria-atomic="true">
        {captureError ?? error ?? captureGuidance}
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
