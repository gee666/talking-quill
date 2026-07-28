import {
  ProviderBaseUrlSchema,
  RunnableProviderConfigSchema,
  type Destination,
  type ProviderCatalogEntry,
  type RunnableProviderConfig,
  type RunnableProviderId,
} from '../../../shared/schemas/providers';
import {
  ProviderSettingsDraftSchema,
  type ProviderSettingsDraft,
} from '../../../shared/schemas/settings';

export type RequestState = 'idle' | 'loading' | 'success' | 'empty' | 'error' | 'cancelled';
export type ConnectionState = 'idle' | 'loading' | 'success' | 'error' | 'cancelled';

export const ENDPOINT_REPAIR_MESSAGE =
  'This stored legacy endpoint is inactive. Replace it with an HTTP or HTTPS URL, then save.';

const ACTIONABLE_ERRORS: Readonly<Record<string, string>> = Object.freeze({
  INVALID_CONFIG: 'Check the endpoint and required configuration fields.',
  STALE_CONFIG: 'The provider configuration changed. Refresh it and enter the credential again.',
  MISSING_CREDENTIAL: 'Store an API key before connecting to this provider.',
  SECURITY_BLOCKED: 'This endpoint was blocked. Use HTTPS for credentialed LAN or cloud services.',
  UNAVAILABLE: 'The provider is unavailable. Check that it is running and reachable.',
  PI_NOT_FOUND: 'A compatible Pi executable was not found. Install Pi or choose its path.',
  PI_CONFIG_INVALID: 'The configured Pi executable or directory is invalid.',
  PI_INCOMPATIBLE:
    'This Pi version lacks required print, model, or isolation options. Update Pi and retry.',
  PI_LAUNCH_FAILED: 'Pi could not start or did not complete its bounded capability check.',
  AUTHENTICATION_FAILED: 'The provider rejected the API key. Replace it and try again.',
  RATE_LIMITED: 'The provider rate limit was reached. Wait, then retry.',
  MODEL_NOT_FOUND: 'The selected model is not installed or no longer available.',
  NO_MODELS: 'No compatible models were found. Install one or enter its model ID manually.',
  TIMEOUT: 'The provider did not respond before the timeout. Check the endpoint and retry.',
  CANCELLED: 'The provider request was cancelled.',
  REQUEST_TOO_LARGE: 'The transcript is too large for one provider request.',
  RESPONSE_TOO_LARGE: 'The provider returned more data than Talking Quill accepts.',
  INVALID_RESPONSE: 'The provider response was not valid. Check provider compatibility.',
  REMOTE_FAILURE: 'The provider reported an error. Check its status and configuration.',
});

export function createDraft(
  provider: ProviderCatalogEntry,
  stored: ProviderSettingsDraft | undefined,
): ProviderSettingsDraft {
  const defaults: Record<string, string | number> = {};
  for (const field of provider.fields) {
    if (field.secret) continue;
    if (field.key === 'modelId' && provider.defaultModel !== null) {
      defaults[field.key] = provider.defaultModel;
    } else if (field.defaultValue !== undefined) defaults[field.key] = field.defaultValue;
    else if (field.key === 'baseUrl' && field.placeholder !== undefined) {
      defaults[field.key] = field.placeholder;
    }
  }
  return ProviderSettingsDraftSchema.parse({ ...defaults, ...stored });
}

export function configFromDraft(
  providerId: RunnableProviderId,
  draft: ProviderSettingsDraft,
): RunnableProviderConfig {
  return RunnableProviderConfigSchema.parse({ providerId, ...draft });
}

export function requiresEndpointRepair(
  providerId: RunnableProviderId,
  draft: ProviderSettingsDraft,
): boolean {
  if (draft.baseUrl === undefined || ProviderBaseUrlSchema.safeParse(draft.baseUrl).success) {
    return false;
  }
  return RunnableProviderConfigSchema.safeParse({
    providerId,
    ...draft,
    baseUrl: 'https://repair.invalid',
  }).success;
}

export function credentialBindingKey(
  providerId: RunnableProviderId,
  config: Pick<ProviderSettingsDraft, 'baseUrl' | 'region'>,
): string {
  return `${config.baseUrl ?? 'fixed'}\u0000${providerId === 'bedrock' ? (config.region ?? '') : ''}`;
}

export function formatOperationElapsed(milliseconds: number): string {
  return `${(milliseconds / 1_000).toFixed(1)}s`;
}

export function draftsEqual(left: ProviderSettingsDraft, right: ProviderSettingsDraft): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
  for (const key of keys) {
    const field = key as keyof ProviderSettingsDraft;
    if (left[field] !== right[field]) return false;
  }
  return true;
}

export function providerFieldErrors(
  issues: readonly { readonly path: readonly PropertyKey[]; readonly message: string }[],
): Readonly<Record<string, string>> {
  const errors: Record<string, string> = {};
  for (const issue of issues) {
    const field = issue.path.at(-1);
    if (typeof field === 'string' && errors[field] === undefined) errors[field] = issue.message;
  }
  return errors;
}

export function errorCode(error: unknown): string | null {
  if (typeof error !== 'object' || error === null || !('code' in error)) return null;
  return typeof error.code === 'string' ? error.code : null;
}

export function piDiscoveryError(error: unknown): string {
  switch (errorCode(error)) {
    case 'CANCELLED':
      return 'Pi model discovery cancelled.';
    case 'AUTHENTICATION_FAILED':
      return 'Pi authentication failed. Sign in to the required provider with Pi, then retry discovery.';
    case 'INVALID_RESPONSE':
      return 'The installed Pi model list was malformed or incompatible. Update Pi, then retry.';
    case 'NO_MODELS':
      return 'Pi returned no models. Authenticate or enter a strict provider/model ID manually.';
    case 'PI_NOT_FOUND':
      return 'Pi was not found. Run npm install -g @earendil-works/pi-coding-agent, then use Auto-detect.';
    case 'PI_CONFIG_INVALID':
      return 'The configured Pi path is stale or invalid. Choose a valid path or explicitly use Auto-detect.';
    case 'PI_INCOMPATIBLE':
      return 'This Pi command is missing required CLI capabilities. Update Pi or choose another executable.';
    case 'PI_LAUNCH_FAILED':
      return 'Pi could not complete its bounded capability check. Retry or choose another executable.';
    default:
      return 'Pi is unavailable. Check the Pi installation path and retry.';
  }
}

export function actionableError(error: unknown, providerId?: RunnableProviderId): string {
  const code = errorCode(error);
  if (providerId === 'pi' && code === 'UNAVAILABLE') {
    return 'Pi could not be launched. Check the selected executable, restart Talking Quill, then retry.';
  }
  if (providerId === 'pi' && code === 'MODEL_NOT_FOUND') {
    return 'The selected Pi model disappeared. Open Settings and select another model.';
  }
  return (
    (code === null ? undefined : ACTIONABLE_ERRORS[code]) ??
    'The provider operation failed safely. Retry after checking the configuration.'
  );
}

export function destinationLabel(destination: Destination | null, providerName?: string): string {
  if (destination === 'local') return 'Runs on this computer';
  if (destination === 'lan') return 'Runs on your network';
  if (destination === 'cloud') return `Sends your text to ${providerName ?? 'a company online'}`;
  return 'We do not know yet where your text goes';
}

export function destinationTone(destination: Destination | null): 'success' | 'info' | 'warning' {
  if (destination === 'local') return 'success';
  if (destination === 'lan') return 'info';
  return 'warning';
}
