import type { AppState, AppStatus } from '../shared/schemas/app-state';
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

export function exactAppStatusLabel(state: AppState, platform: string): string {
  if (state.status !== 'needs-setup') return presentAppStatus(state.status).label;
  if (!state.modelReady) return 'Install speech model';
  const ownerName = platform === 'darwin' ? 'keyboard service' : 'local keyboard owner';
  switch (state.helper.reason) {
    case 'input-monitoring-required':
      return 'Allow Input Monitoring';
    case 'accessibility-required':
    case 'event-post-required':
      return 'Allow Accessibility';
    case 'owner-busy':
      return 'Close the other controller';
    case 'owner-draining':
      return 'Release shortcut keys';
    case 'owner-maintenance':
      return 'Wait for update';
    case 'capture-disabled':
    case 'owner-rollback':
      return 'Shortcuts disabled';
    case 'binary-missing':
    case 'protocol-mismatch':
    case 'owner-missing':
    case 'owner-auth-failed':
    case 'owner-security-fault':
    case 'owner-incompatible':
    case 'owner-degraded':
    case 'owner-indeterminate':
      return `Repair ${ownerName}`;
    case 'crash-loop':
      return platform === 'darwin'
        ? 'Keyboard service restarting'
        : 'Local keyboard owner restarting';
    case null:
      return state.helper.status === 'starting'
        ? `Starting ${ownerName}`
        : `${platform === 'darwin' ? 'Keyboard service' : 'Local keyboard owner'} unavailable`;
    default:
      return `${platform === 'darwin' ? 'Keyboard service' : 'Local keyboard owner'} unavailable`;
  }
}
