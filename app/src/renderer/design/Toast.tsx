import { Button } from './Button';

export interface ToastProps {
  readonly tone?: 'info' | 'success' | 'warning' | 'error';
  readonly message: string;
  readonly onDismiss?: () => void;
}

export function Toast({ tone = 'info', message, onDismiss }: ToastProps) {
  const urgent = tone === 'error';
  return (
    <div
      className={`me-toast me-toast--${tone}`}
      role={urgent ? 'alert' : 'status'}
      aria-live={urgent ? 'assertive' : 'polite'}
    >
      <p className="me-toast__message">{message}</p>
      {onDismiss === undefined ? null : (
        <Button variant="quiet" aria-label="Dismiss notification" onClick={onDismiss}>
          ×
        </Button>
      )}
    </div>
  );
}
