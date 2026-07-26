import type { ProviderId } from './schemas/providers';

export type ProviderModelSelectionPolicy = 'required' | 'provider-managed';

export function providerModelSelectionPolicy(providerId: ProviderId): ProviderModelSelectionPolicy {
  return providerId === 'textgenwebui' ? 'provider-managed' : 'required';
}
