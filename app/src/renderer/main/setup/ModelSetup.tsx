import { useCallback, useEffect, useRef, useState } from 'react';
import type { ModelStatus } from '../../../shared/schemas/transcription';
import type { Settings } from '../../../shared/schemas/settings';
import { Button, Progress, Select, Status, Toast } from '../../design';

type WhisperModelId = Settings['transcription']['modelId'];

const MODEL_ENTRIES = {
  'Xenova/whisper-small': { name: 'Faster — good for everyday dictation', size: 'about 250 MB' },
  'onnx-community/whisper-large-v3-turbo': {
    name: 'More accurate — takes a little longer',
    size: 'about 1.09 GB',
  },
} as const satisfies Record<WhisperModelId, { readonly name: string; readonly size: string }>;

export function ModelSetup({
  settings,
  onSettingsSaved,
}: {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
}) {
  const modelId = settings.transcription.modelId;
  const [status, setStatus] = useState<ModelStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const refreshGeneration = useRef(0);
  const selectionGeneration = useRef(0);
  const mounted = useRef(true);

  const refresh = useCallback(async (): Promise<ModelStatus | null> => {
    const generation = ++refreshGeneration.current;
    try {
      const next = await window.talkingQuill.models.status(modelId);
      if (generation === refreshGeneration.current && next.modelId === modelId) {
        setStatus(next);
        setError(null);
      }
      return next.modelId === modelId ? next : null;
    } catch {
      if (generation === refreshGeneration.current) {
        setError('Talking Quill couldn’t check the speech model on this computer.');
      }
      return null;
    }
  }, [modelId]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      selectionGeneration.current += 1;
    };
  }, []);

  useEffect(() => {
    const refreshTimer = setTimeout(() => void refresh(), 0);
    const unsubscribe = window.talkingQuill.models.onProgress((progress) => {
      if (progress.modelId !== modelId) return;
      setError(null);
      setStatus((current) => ({
        modelId,
        state: progress.state,
        downloadedBytes: progress.total.downloadedBytes,
        totalBytes: progress.total.totalBytes,
        detail:
          progress.state === 'downloading' ||
          progress.state === 'verifying' ||
          progress.state === 'installing'
            ? null
            : (current?.detail ?? null),
        repairable: progress.state === 'corrupt' || progress.state === 'error',
      }));
    });
    return () => {
      clearTimeout(refreshTimer);
      refreshGeneration.current += 1;
      unsubscribe();
    };
  }, [modelId, refresh]);

  const perform = async (
    action: () => Promise<ModelStatus>,
    fallback = 'That didn’t work. Try again — whatever was already downloaded is kept.',
  ) => {
    setBusy(true);
    setError(null);
    try {
      const next = await action();
      if (mounted.current) setStatus(next);
    } catch {
      const refreshed = await refresh();
      if (mounted.current) setError(refreshed?.detail ?? fallback);
    } finally {
      if (mounted.current) setBusy(false);
    }
  };

  const saveModelSelection = async (nextModelId: WhisperModelId) => {
    const generation = ++selectionGeneration.current;
    setBusy(true);
    setError(null);
    try {
      const saved = await window.talkingQuill.settings.update({
        transcription: { modelId: nextModelId },
      });
      if (mounted.current && generation === selectionGeneration.current) onSettingsSaved(saved);
    } catch {
      if (mounted.current && generation === selectionGeneration.current) {
        setError('That choice couldn’t be saved. Please try again.');
      }
    } finally {
      if (mounted.current && generation === selectionGeneration.current) setBusy(false);
    }
  };

  const visibleStatus = status?.modelId === modelId ? status : null;
  const entry = MODEL_ENTRIES[modelId];
  const detail = visibleStatus?.detail ?? null;
  const state = visibleStatus?.state ?? 'checking';
  const action =
    state === 'downloading'
      ? { label: 'Pause download', run: () => window.talkingQuill.models.pause(modelId) }
      : state === 'verifying' || state === 'installing'
        ? null
        : state === 'paused'
          ? { label: 'Resume download', run: () => window.talkingQuill.models.download(modelId) }
          : state === 'ready'
            ? null
            : state === 'offline' || state === 'error' || state === 'corrupt'
              ? { label: 'Try again', run: () => window.talkingQuill.models.retry(modelId) }
              : {
                  label: 'Download',
                  run: () => window.talkingQuill.models.download(modelId),
                };

  return (
    <div className="model-setup">
      <Select
        label="Which model to use"
        hint={`${modelId} (${entry.size})`}
        value={modelId}
        disabled={
          busy || state === 'downloading' || state === 'verifying' || state === 'installing'
        }
        onChange={(event) => void saveModelSelection(event.currentTarget.value as WhisperModelId)}
      >
        <option value="Xenova/whisper-small">{MODEL_ENTRIES['Xenova/whisper-small'].name}</option>
        <option value="onnx-community/whisper-large-v3-turbo">
          {MODEL_ENTRIES['onnx-community/whisper-large-v3-turbo'].name}
        </option>
      </Select>
      <div className="group">
        <div className="model-setup__row model-setup__row--state-only">
          <div className="model-setup__state">
            {visibleStatus === null ? (
              <p role="status" className="model-setup__checking">
                Checking…
              </p>
            ) : (
              <Status
                tone={
                  state === 'ready'
                    ? 'success'
                    : state === 'corrupt' || state === 'error'
                      ? 'error'
                      : state === 'offline'
                        ? 'warning'
                        : 'info'
                }
                live
              >
                {modelStateLabel(visibleStatus)}
              </Status>
            )}
            <div className="provider-actions">
              {action === null ? null : (
                <Button busy={busy} onClick={() => void perform(action.run)}>
                  {action.label}
                </Button>
              )}
              {state === 'downloading' ? (
                <Button
                  variant="secondary"
                  disabled={busy}
                  onClick={() => void perform(() => window.talkingQuill.models.cancel(modelId))}
                >
                  Cancel download
                </Button>
              ) : state === 'verifying' || state === 'installing' ? (
                <Button
                  variant="secondary"
                  disabled={busy}
                  onClick={() => void perform(() => window.talkingQuill.models.pause(modelId))}
                >
                  Pause
                </Button>
              ) : null}
              {state === 'ready' ? (
                <Button
                  variant="danger"
                  disabled={busy}
                  onClick={() =>
                    void perform(
                      () =>
                        window.talkingQuill.models.delete(modelId).then((result) => result.status),
                      'The model couldn’t be deleted — it may be in use right now. Try again in a moment.',
                    )
                  }
                >
                  Delete
                </Button>
              ) : null}
            </div>
          </div>
        </div>
        {visibleStatus === null ? null : (
          <div className="model-setup__row model-setup__row--progress">
            <Progress
              label="Download progress"
              value={visibleStatus.downloadedBytes}
              max={visibleStatus.totalBytes}
              disabled={state === 'missing'}
            />
          </div>
        )}
        {detail === null ? null : <p className="body-copy model-setup__detail">{detail}</p>}
      </div>
      {error === null ? null : (
        <Toast tone="error" message={error} onDismiss={() => setError(null)} />
      )}
    </div>
  );
}

function modelStateLabel(status: ModelStatus): string {
  switch (status.state) {
    case 'missing':
      return 'Not downloaded yet';
    case 'checking':
      return 'Checking the download';
    case 'downloading':
      return 'Downloading';
    case 'verifying':
      return 'Checking everything arrived';
    case 'installing':
      return 'Almost ready';
    case 'paused':
      return 'Download paused';
    case 'ready':
      return 'Ready — works without internet';
    case 'corrupt':
      return 'The download is damaged — download it again';
    case 'offline':
      return 'No internet — reconnect to finish the download';
    case 'error':
      return 'The download didn’t finish';
  }
}
