import { useCallback, useEffect, useRef, useState } from 'react';
import type { ModelStatus } from '../../../shared/schemas/transcription';
import type { Settings } from '../../../shared/schemas/settings';
import { Button, Progress, Select, Status, Toast } from '../../design';

type WhisperModelId = Settings['transcription']['modelId'];

const MODEL_ENTRIES = {
  'onnx-community/whisper-large-v3-turbo': {
    name: 'Whisper Large v3 Turbo (Recommended)',
    size: 'about 1.09 GB',
  },
  'Xenova/whisper-small': { name: 'Whisper Small (Lower quality)', size: 'about 250 MB' },
} as const satisfies Record<WhisperModelId, { readonly name: string; readonly size: string }>;

function modelOptionLabel(id: WhisperModelId): string {
  return `${MODEL_ENTRIES[id].name} — ${MODEL_ENTRIES[id].size}`;
}

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

  const refresh = useCallback(async (): Promise<ModelStatus | null> => {
    const generation = ++refreshGeneration.current;
    try {
      const next = await window.talkingQuill.models.status(modelId);
      if (generation === refreshGeneration.current && next.modelId === modelId) setStatus(next);
      return next.modelId === modelId ? next : null;
    } catch {
      if (generation === refreshGeneration.current) {
        setError('The local model status could not be read.');
      }
      return null;
    }
  }, [modelId]);

  useEffect(() => {
    const refreshTimer = setTimeout(() => void refresh(), 0);
    const unsubscribe = window.talkingQuill.models.onProgress((progress) => {
      if (progress.modelId !== modelId) return;
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

  const perform = async (action: () => Promise<ModelStatus>) => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await action());
    } catch (cause: unknown) {
      const refreshed = await refresh();
      setError(refreshed?.detail ?? modelActionError(cause));
    } finally {
      setBusy(false);
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
              ? { label: 'Retry download', run: () => window.talkingQuill.models.retry(modelId) }
              : {
                  label: 'Download model',
                  run: () => window.talkingQuill.models.download(modelId),
                };

  return (
    <div className="model-setup">
      <Select
        label="Transcription model"
        hint={
          modelId === 'onnx-community/whisper-large-v3-turbo' ? '(Recommended)' : '(Lower quality)'
        }
        value={modelId}
        disabled={
          busy || state === 'downloading' || state === 'verifying' || state === 'installing'
        }
        onChange={(event) => {
          setError(null);
          void window.talkingQuill.settings
            .update({
              transcription: {
                modelId: event.currentTarget.value as WhisperModelId,
              },
            })
            .then(onSettingsSaved)
            .catch(() => setError('The selected model could not be saved.'));
        }}
      >
        <option value="onnx-community/whisper-large-v3-turbo">
          {modelOptionLabel('onnx-community/whisper-large-v3-turbo')}
        </option>
        <option value="Xenova/whisper-small">{modelOptionLabel('Xenova/whisper-small')}</option>
      </Select>
      <div className="group">
        <div className="model-setup__row">
          <span className="model-setup__identity">
            <span className="model-setup__name">{entry.name}</span>
            <span className="model-setup__size">{entry.size}</span>
          </span>
          <div className="model-setup__state">
            {visibleStatus === null ? (
              <p role="status" className="model-setup__checking">
                Checking model…
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
                  onClick={() => void perform(() => window.talkingQuill.models.cancel(modelId))}
                >
                  Cancel download
                </Button>
              ) : state === 'verifying' || state === 'installing' ? (
                <Button
                  variant="secondary"
                  onClick={() => void perform(() => window.talkingQuill.models.pause(modelId))}
                >
                  Pause model setup
                </Button>
              ) : null}
              {state === 'ready' ? (
                <Button
                  variant="danger"
                  disabled={busy}
                  onClick={() =>
                    void window.talkingQuill.models
                      .delete(modelId)
                      .then((result) => setStatus(result.status))
                      .catch(() =>
                        setError('The model is currently in use or could not be deleted.'),
                      )
                  }
                >
                  Delete model
                </Button>
              ) : null}
            </div>
          </div>
        </div>
        {visibleStatus === null ? null : (
          <div className="model-setup__row model-setup__row--progress">
            <Progress
              label="Model files"
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
      return 'Model download required';
    case 'checking':
      return 'Checking model integrity';
    case 'downloading':
      return 'Downloading model';
    case 'verifying':
      return 'Verifying downloaded model';
    case 'installing':
      return 'Installing verified model';
    case 'paused':
      return 'Download paused';
    case 'ready':
      return 'Model ready for offline transcription';
    case 'corrupt':
      return 'Model files are corrupt and need repair';
    case 'offline':
      return 'Offline — reconnect to finish the download';
    case 'error':
      return 'Model setup failed';
  }
}

function modelActionError(cause: unknown): string {
  if (cause instanceof Error && cause.message.trim() !== '') return cause.message;
  return 'The model action could not be completed. Retry to resume from safe existing files.';
}
