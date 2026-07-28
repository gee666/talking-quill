import { useId, type ReactNode, type SelectHTMLAttributes } from 'react';

export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  readonly label: string;
  readonly hint?: string | undefined;
  readonly error?: string | undefined;
  /**
   * Fields are stacked (label above a full-width control) everywhere by default.
   * `row` opts the field into the two-column settings row (label left, control
   * right) — but only when it is a direct child of `.section__body` or `.rows`.
   * Pass `stacked` to keep a field stacked even inside a settings list.
   */
  readonly layout?: 'row' | 'stacked';
  readonly children: ReactNode;
}

export function Select({
  label,
  hint,
  error,
  id,
  required,
  layout = 'row',
  className = '',
  children,
  ...props
}: SelectProps) {
  const generatedId = useId();
  const controlId = id ?? generatedId;
  const hintId = hint === undefined ? undefined : `${controlId}-hint`;
  const errorId = error === undefined ? undefined : `${controlId}-error`;
  return (
    <div className={`me-field me-field--${layout}`}>
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
