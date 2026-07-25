import type { HTMLAttributes, ReactNode } from 'react';

export interface CardProps extends HTMLAttributes<HTMLDivElement> {
  readonly title?: string;
  readonly description?: string;
  readonly interactive?: boolean;
  readonly disabled?: boolean;
  readonly children: ReactNode;
}

export function Card({
  title,
  description,
  interactive = false,
  disabled = false,
  className = '',
  children,
  ...props
}: CardProps) {
  return (
    <div
      {...props}
      className={`me-card ${className}`.trim()}
      data-interactive={interactive}
      role={interactive ? 'button' : undefined}
      aria-disabled={interactive ? disabled : undefined}
      tabIndex={interactive && !disabled ? 0 : undefined}
      onClick={disabled ? undefined : props.onClick}
      onKeyDown={(event) => {
        props.onKeyDown?.(event);
        if (
          !event.defaultPrevented &&
          interactive &&
          !disabled &&
          (event.key === 'Enter' || event.key === ' ')
        ) {
          event.preventDefault();
          event.currentTarget.click();
        }
      }}
    >
      {title === undefined ? null : (
        <header>
          <h2 className="me-card__heading">{title}</h2>
          {description === undefined ? null : <p className="me-card__description">{description}</p>}
        </header>
      )}
      {children}
    </div>
  );
}
