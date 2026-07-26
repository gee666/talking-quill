import { forwardRef, useId, type TextareaHTMLAttributes } from 'react';

export interface TextAreaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  readonly label: string;
  readonly hint?: string;
  readonly error?: string;
}

export const TextArea = forwardRef<HTMLTextAreaElement, TextAreaProps>(function TextArea(
  { label, hint, error, id, required, className = '', ...props },
  ref,
) {
  const generatedId = useId();
  const controlId = id ?? generatedId;
  const hintId = hint === undefined ? undefined : `${controlId}-hint`;
  const errorId = error === undefined ? undefined : `${controlId}-error`;
  return (
    <div className="me-field">
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
