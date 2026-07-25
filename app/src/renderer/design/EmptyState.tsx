import { useId, type ReactNode } from 'react';
import { Icon } from './Icon';

export interface EmptyStateProps {
  readonly title: string;
  readonly description: string;
  readonly action?: ReactNode;
}

export function EmptyState({ title, description, action }: EmptyStateProps) {
  const titleId = useId();
  return (
    <section className="me-empty" aria-labelledby={titleId}>
      <span className="me-empty__icon">
        <Icon name="search" size={16} />
      </span>
      <h2 id={titleId} className="me-empty__title">
        {title}
      </h2>
      <p className="me-empty__description">{description}</p>
      {action}
    </section>
  );
}
