import { StrictMode, useCallback, useEffect, useRef, useState, type CSSProperties } from 'react';
import { createRoot } from 'react-dom/client';
import { WIDGET_DIMENSIONS } from '../../shared/constants/echo-session';
import type { EchoSessionSnapshot } from '../../shared/schemas/echo-session';
import '../design/global.css';
import { applyTheme, Button, resolveInitialTheme, Status } from '../design';
import './widget.css';
import { isWidgetPointerCancelable, widgetPresentation } from './fallback-label';
import { subscribeToWidgetSession } from './session-subscription';

const EMPTY: EchoSessionSnapshot = {
  sessionId: null,
  phase: 'idle',
  dictationMode: null,
  processingMode: null,
  alternate: false,
  rms: 0,
  elapsedMs: 0,
  transcript: null,
  abortReason: null,
  fallbackCategory: null,
  completion: null,
  message: null,
};

export function WidgetShell() {
  const [session, setSession] = useState<EchoSessionSnapshot>(EMPTY);
  const [viewport, setViewport] = useState(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
  }));
  const setInteractive = useCallback((next: boolean) => {
    // Main may independently reset native hit testing when hiding/restoring the widget, so each
    // forwarded pointer observation must resynchronize rather than relying on renderer-only state.
    void window.talkingQuillWidget.setInteractive(next);
  }, []);
  useEffect(() => {
    // The widget is a separate renderer window, so it mirrors the main window's stored theme.
    const sync = () => applyTheme(resolveInitialTheme());
    sync();
    window.addEventListener('storage', sync);
    return () => window.removeEventListener('storage', sync);
  }, []);
  useEffect(() => {
    const update = () => setViewport({ width: window.innerWidth, height: window.innerHeight });
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, []);
  useEffect(() => {
    const unsubscribe = subscribeToWidgetSession(window.talkingQuillWidget, setSession);
    return () => {
      setInteractive(false);
      unsubscribe();
    };
  }, [setInteractive]);

  const extended = session.dictationMode === 'extended';
  const recording = session.phase === 'arming' || session.phase.startsWith('recording');
  const level = usePerceptualMicrophoneLevel(session.rms, recording);
  const presentation = widgetPresentation(session);
  const cancelable = isWidgetPointerCancelable(session.phase);
  useEffect(() => setInteractive(false), [cancelable, recording, setInteractive]);
  const scale = Math.max(
    0.01,
    Math.min(
      viewport.width / WIDGET_DIMENSIONS.default.width,
      viewport.height / WIDGET_DIMENSIONS.default.height,
    ),
  );
  const layoutStyle = {
    '--widget-scale': String(scale),
    '--widget-layout-width': `${String(viewport.width / scale)}px`,
    '--widget-layout-height': `${String(viewport.height / scale)}px`,
  } as CSSProperties;
  const updatePointerMode = (target: EventTarget | null) => {
    const button = target instanceof Element ? target.closest('button') : null;
    setInteractive(button !== null && !button.hasAttribute('disabled'));
  };
  return (
    <main
      className="widget-shell"
      style={layoutStyle}
      aria-label="Talking Quill dictation status"
      onPointerMove={(event) => updatePointerMode(event.target)}
      onPointerLeave={() => setInteractive(false)}
    >
      <p className="widget-live" role="status" aria-live="polite" aria-atomic="true">
        {presentation.heading}. {presentation.secondary}
      </p>
      <p id="widget-keyboard-equivalents" className="widget-live">
        This status window never takes keyboard focus. Use global Enter to submit, Escape to cancel,
        or the full activation shortcut chord to stop. Pointer Stop and Cancel controls are also
        available.
      </p>
      <div className="widget-pill">
        <div
          className="widget-level"
          aria-label="Microphone level"
          role="meter"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={level}
          aria-valuetext={`${String(level)} percent, ${microphoneLevelState(level)}`}
        >
          {Array.from({ length: 5 }, (_value, index) => (
            <i key={index} className={level >= (index + 1) * 16 ? 'active' : ''} />
          ))}
          <span className="widget-live">
            Microphone level {level} percent, {microphoneLevelState(level)}
          </span>
        </div>
        <div className="widget-copy">
          <div className="widget-heading">
            <strong>{presentation.heading}</strong>
            <Status
              tone={
                session.phase === 'error'
                  ? 'error'
                  : session.phase === 'completed'
                    ? 'success'
                    : 'info'
              }
            >
              {presentation.badge}
            </Status>
          </div>
          <span>{presentation.secondary}</span>
        </div>
        {extended && recording ? <time>{formatElapsed(session.elapsedMs)}</time> : null}
        {cancelable ? (
          <div className="widget-actions">
            {recording && extended ? (
              <Button
                aria-label="Stop Extended Dictation"
                aria-describedby="widget-keyboard-equivalents"
                onClick={() => {
                  setInteractive(false);
                  void window.talkingQuillWidget.stop();
                }}
              >
                Stop
              </Button>
            ) : null}
            <Button
              variant="quiet"
              aria-label="Cancel dictation"
              aria-describedby="widget-keyboard-equivalents"
              onClick={() => {
                setInteractive(false);
                void window.talkingQuillWidget.cancel();
              }}
            >
              Cancel
            </Button>
          </div>
        ) : null}
      </div>
    </main>
  );
}

function usePerceptualMicrophoneLevel(rms: number, active: boolean): number {
  const previous = useRef(0);
  const [level, setLevel] = useState(0);
  useEffect(() => {
    let next = 0;
    if (active) {
      const dbfs = 20 * Math.log10(Math.max(rms, 0.000_01));
      const perceptual = Math.round(Math.max(0, Math.min(100, ((dbfs + 60) / 48) * 100)));
      const coefficient = perceptual > previous.current ? 0.65 : 0.22;
      next = Math.round(previous.current + (perceptual - previous.current) * coefficient);
    }
    previous.current = next;
    const frame = requestAnimationFrame(() => setLevel(next));
    return () => cancelAnimationFrame(frame);
  }, [active, rms]);
  return level;
}

function microphoneLevelState(level: number): string {
  if (level === 0) return 'silent';
  if (level < 25) return 'quiet';
  if (level < 65) return 'speaking';
  return 'loud';
}

function formatElapsed(milliseconds: number): string {
  const seconds = Math.floor(milliseconds / 1_000);
  return `${String(Math.floor(seconds / 60)).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}`;
}

const root = document.querySelector('#root');
if (root === null) throw new Error('Widget root is missing');
createRoot(root).render(
  <StrictMode>
    <WidgetShell />
  </StrictMode>,
);
