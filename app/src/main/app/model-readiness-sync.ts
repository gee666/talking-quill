import type { ModelProgress } from '../../shared/schemas/transcription';

export interface ModelReadinessStateTarget {
  setModelReady(ready: boolean): void;
}

export interface ModelReadinessEchoTarget {
  readinessChanged(): void;
}

export function applySelectedModelProgress(
  progress: ModelProgress,
  selectedModelId: string,
  state: ModelReadinessStateTarget | null,
  echo: ModelReadinessEchoTarget | null,
): void {
  if (progress.modelId !== selectedModelId) return;
  state?.setModelReady(progress.state === 'ready');
  echo?.readinessChanged();
}
