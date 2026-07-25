import { describe, expect, it, vi } from 'vitest';
import { applySelectedModelProgress } from '../../app/src/main/app/model-readiness-sync';
import type { ModelProgress } from '../../app/src/shared/schemas/transcription';

const SELECTED = 'Xenova/whisper-small' as const;

function progress(
  state: ModelProgress['state'],
  modelId: ModelProgress['modelId'] = SELECTED,
): ModelProgress {
  return {
    modelId,
    state,
    file: null,
    total: { downloadedBytes: state === 'ready' ? 1 : 0, totalBytes: 1 },
  };
}

describe('selected model readiness synchronization', () => {
  it.each([
    { state: 'ready' as const, ready: true },
    { state: 'missing' as const, ready: false },
    { state: 'corrupt' as const, ready: false },
    { state: 'downloading' as const, ready: false },
  ])('updates app state and helper activation on $state progress', ({ state, ready }) => {
    const stateTarget = { setModelReady: vi.fn() };
    const echoTarget = { readinessChanged: vi.fn() };
    applySelectedModelProgress(progress(state), SELECTED, stateTarget, echoTarget);
    expect(stateTarget.setModelReady).toHaveBeenCalledWith(ready);
    expect(echoTarget.readinessChanged).toHaveBeenCalledOnce();
  });

  it('ignores another model and tolerates progress during initialization', () => {
    const stateTarget = { setModelReady: vi.fn() };
    const echoTarget = { readinessChanged: vi.fn() };
    applySelectedModelProgress(
      progress('ready', 'onnx-community/whisper-large-v3-turbo'),
      SELECTED,
      stateTarget,
      echoTarget,
    );
    expect(stateTarget.setModelReady).not.toHaveBeenCalled();
    expect(echoTarget.readinessChanged).not.toHaveBeenCalled();
    expect(() => applySelectedModelProgress(progress('ready'), SELECTED, null, null)).not.toThrow();
  });
});
