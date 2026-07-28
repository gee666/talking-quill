import { forwardRef, useId, type InputHTMLAttributes } from 'react';

export interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
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
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { label, hint, error, id, required, layout = 'row', className = '', ...props },
  ref,
) {
  const generatedId = useId();
  const controlId = id ?? generatedId;
  const hintId = hint === undefined ? undefined : `${controlId}-hint`;
  const errorId = error === undefined ? undefined : `${controlId}-error`;
  const describedBy = [hintId, errorId].filter(Boolean).join(' ') || undefined;
  return (
    <div className={`me-field me-field--${layout}`}>
      <label className="me-field__label" htmlFor={controlId}>
        {label}{' '}
        {required ? (
          <span className="me-field__required" aria-hidden="true">
            *
          </span>
        ) : null}
      </label>
      <input
        {...props}
        ref={ref}
        id={controlId}
        required={required}
        className={`me-field__control ${className}`.trim()}
        aria-invalid={error === undefined ? undefined : true}
        aria-describedby={describedBy}
      />
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
});
