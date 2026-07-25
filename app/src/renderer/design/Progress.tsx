export interface ProgressProps {
  readonly label: string;
  readonly value: number;
  readonly max?: number;
  readonly disabled?: boolean;
}

export function Progress({ label, value, max = 100, disabled = false }: ProgressProps) {
  const safeMax = Math.max(1, max);
  const safeValue = Math.min(Math.max(value, 0), safeMax);
  const percent = Math.round((safeValue / safeMax) * 100);
  return (
    <div
      className={`me-progress ${disabled ? 'me-progress--disabled' : ''}`}
      aria-disabled={disabled || undefined}
    >
      <div className="me-progress__header">
        <span className="me-progress__label">{label}</span>
        <span className="me-progress__value">{percent}%</span>
      </div>
      <progress className="me-progress__bar" value={safeValue} max={safeMax} aria-label={label}>
        {percent}%
      </progress>
    </div>
  );
}
