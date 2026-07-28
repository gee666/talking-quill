import { forwardRef, useId, type TextareaHTMLAttributes } from 'react';

export interface TextAreaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  readonly label: string;
  readonly hint?: string;
  readonly error?: string;
  /**
   * Fields are stacked (label above a full-width control) everywhere by default.
   * `row` opts the field into the two-column settings row (label left, control
   * right) — but only when it is a direct child of `.section__body` or `.rows`.
   * A multi-line control almost always wants the default `stacked`.
   */
  readonly layout?: 'row' | 'stacked';
}

export const TextArea = forwardRef<HTMLTextAreaElement, TextAreaProps>(function TextArea(
  { label, hint, error, id, required, layout = 'stacked', className = '', ...props },
  ref,
) {
  const generatedId = useId();
  const controlId = id ?? generatedId;
  const hintId = hint === undefined ? undefined : `${controlId}-hint`;
  const errorId = error === undefined ? undefined : `${controlId}-error`;
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
      <textarea
        {...props}
        ref={ref}
        id={controlId}
        required={required}
        className={`me-field__control ${className}`.trim()}
        aria-invalid={error === undefined ? undefined : true}
        aria-describedby={[hintId, errorId].filter(Boolean).join(' ') || undefined}
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
