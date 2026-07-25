import { useId, type ReactNode, type SelectHTMLAttributes } from 'react';

export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  readonly label: string;
  readonly hint?: string | undefined;
  readonly error?: string | undefined;
  readonly children: ReactNode;
}

export function Select({
  label,
  hint,
  error,
  id,
  required,
  className = '',
  children,
  ...props
}: SelectProps) {
  const generatedId = useId();
  const controlId = id ?? generatedId;
  const hintId = hint === undefined ? undefined : `${controlId}-hint`;
  const errorId = error === undefined ? undefined : `${controlId}-error`;
  return (
    <div className="me-field me-field--row">
      <label className="me-field__label" htmlFor={controlId}>
        {label}
        {required ? (
          <span className="me-field__required" aria-hidden="true">
            {' '}
            *
          </span>
        ) : null}
      </label>
      <select
        {...props}
        id={controlId}
        required={required}
        className={`me-field__control ${className}`.trim()}
        aria-invalid={error === undefined ? undefined : true}
        aria-describedby={[hintId, errorId].filter(Boolean).join(' ') || undefined}
      >
        {children}
      </select>
      {hint === undefined ? null : (
        <span id={hintId} className="me-field__hint">
          {hint}
        </span>
      )}
      {error === undefined ? null : (
        <span id={errorId} className="me-field__error">
          {error}
        </span>
      )}
    </div>
  );
}
