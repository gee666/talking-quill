import type { ReactNode } from 'react';

export interface StatusProps {
  readonly tone?: 'neutral' | 'info' | 'success' | 'warning' | 'error';
  readonly children: ReactNode;
  readonly live?: boolean;
}

const icons = { neutral: '●', info: 'i', success: '✓', warning: '!', error: '×' } as const;

export function Status({ tone = 'neutral', children, live = false }: StatusProps) {
  return (
    <span
      className={`me-status me-status--${tone}`}
      role={live ? 'status' : undefined}
      aria-live={live ? 'polite' : undefined}
    >
      <span className="me-status__icon" aria-hidden="true">
        {icons[tone]}
      </span>
      <span>{children}</span>
    </span>
  );
}
