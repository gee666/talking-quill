import type { ModelInfo } from '../../shared/schemas/providers';
import type { ProviderSettingsDraft } from '../../shared/schemas/settings';

export function reconcileDiscoveredModels(
  discovered: readonly ModelInfo[],
  currentDraft: ProviderSettingsDraft,
  piProvider: boolean,
): { readonly message: string | null } {
  const selectedModel = typeof currentDraft.modelId === 'string' ? currentDraft.modelId : null;
  if (discovered.length === 0) {
    return {
      message: piProvider
        ? 'Pi returned no models. The exact saved model is retained; verify it directly or update Pi authentication.'
        : 'No models were returned. The current manual model entry was retained.',
    };
  }
  if (selectedModel !== null && !discovered.some(({ id }) => id === selectedModel)) {
    return {
      message: piProvider
        ? 'The exact selected Pi model was not in this catalog and was retained. Test Connection verifies it directly.'
        : 'The selected manual model was not in this catalog and was retained.',
    };
  }
  return { message: null };
}
