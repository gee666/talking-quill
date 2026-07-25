import type { AppStatus } from '../shared/schemas/app-state';
import type { StatusProps } from './design';

export interface StatusPresentation {
  readonly label:
    'Disabled' | 'Ready' | 'Recording' | 'Transcribing' | 'Processing' | 'Needs Setup';
  readonly tone: NonNullable<StatusProps['tone']>;
}

export const APP_STATUS_PRESENTATIONS = Object.freeze({
  disabled: Object.freeze({ label: 'Disabled', tone: 'neutral' }),
  ready: Object.freeze({ label: 'Ready', tone: 'success' }),
  recording: Object.freeze({ label: 'Recording', tone: 'info' }),
  transcribing: Object.freeze({ label: 'Transcribing', tone: 'info' }),
  processing: Object.freeze({ label: 'Processing', tone: 'info' }),
  'needs-setup': Object.freeze({ label: 'Needs Setup', tone: 'warning' }),
}) satisfies Readonly<Record<AppStatus, StatusPresentation>>;

export function presentAppStatus(status: AppStatus): StatusPresentation {
  return APP_STATUS_PRESENTATIONS[status];
}
