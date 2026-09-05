import { Button, Status } from '../../design';

export function PermissionRow({
  label,
  value,
  open,
  onError,
}: {
  readonly label: string;
  readonly value: string;
  readonly open: () => Promise<void>;
  readonly onError: () => void;
}) {
  const ready = value === 'granted' || value === 'not_applicable';
  return (
    <div className="readiness-row">
      <span>{label}</span>
      <div className="info-actions provider-actions">
        <Status tone={ready ? 'success' : value === 'denied' ? 'error' : 'warning'}>
          {ready ? 'Allowed' : value === 'denied' ? 'Blocked' : 'Not decided yet'}
        </Status>
        {ready ? null : (
          <Button
            variant="quiet"
            aria-label={`Open ${label} settings`}
            onClick={() => {
              void open().catch(onError);
            }}
          >
            Open settings
          </Button>
        )}
      </div>
    </div>
  );
}
