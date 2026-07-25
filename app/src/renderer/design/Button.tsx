import type { ButtonHTMLAttributes, ReactNode } from 'react';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  readonly variant?: 'primary' | 'secondary' | 'danger' | 'quiet';
  readonly busy?: boolean;
  readonly children: ReactNode;
}

export function Button({
  variant = 'primary',
  busy = false,
  disabled,
  className = '',
  children,
  type = 'button',
  ...props
}: ButtonProps) {
  return (
    <button
      {...props}
      type={type}
      className={`me-button me-button--${variant} ${className}`.trim()}
      disabled={disabled === true || busy}
      aria-busy={busy}
    >
      {busy ? (
        <span className="me-button__busy" aria-hidden="true">
          •
        </span>
      ) : null}
      {children}
    </button>
  );
}
