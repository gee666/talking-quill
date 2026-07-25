import { useId, type InputHTMLAttributes } from 'react';

export interface ToggleProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type'> {
  readonly label: string;
  readonly hint?: string;
}

export function Toggle({ label, hint, id, disabled, className = '', ...props }: ToggleProps) {
  const generatedId = useId();
  const controlId = id ?? generatedId;
  const hintId = hint === undefined ? undefined : `${controlId}-hint`;
  return (
    <div className={`me-toggle ${disabled ? 'me-toggle--disabled' : ''} ${className}`.trim()}>
      <span className="me-toggle__copy">
        <label className="me-toggle__label" htmlFor={controlId}>
          {label}
        </label>
        {hint === undefined ? null : (
          <span id={hintId} className="me-toggle__hint">
            {hint}
          </span>
        )}
      </span>
      <label className="me-toggle__control" htmlFor={controlId}>
        <input
          {...props}
          id={controlId}
          className="me-toggle__input"
          type="checkbox"
          disabled={disabled}
          aria-describedby={hintId}
        />
        <span className="me-toggle__track" aria-hidden="true" />
      </label>
    </div>
  );
}
