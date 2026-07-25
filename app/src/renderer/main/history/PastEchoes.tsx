import { useCallback, useEffect, useRef, useState } from 'react';
import type { HistoryCursor, HistoryListItem } from '../../../shared/schemas/history';
import { Button, Card, Dialog, EmptyState, Toast } from '../../design';

export function PastEchoes() {
  const [items, setItems] = useState<readonly HistoryListItem[]>([]);
  const [cursor, setCursor] = useState<HistoryCursor | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<{
    readonly message: string;
    readonly tone: 'success' | 'warning';
  } | null>(null);
  const [confirmDeleteAll, setConfirmDeleteAll] = useState(false);
  const requestSequence = useRef(0);

  const load = useCallback(async (nextCursor: HistoryCursor | null = null) => {
    const sequence = ++requestSequence.current;
    if (nextCursor === null) setLoading(true);
    else setBusy(true);
    setError(null);
    try {
      const page = await window.talkingQuill.history.list(25, nextCursor);
      if (sequence !== requestSequence.current) return;
      setItems((current) => (nextCursor === null ? page.items : [...current, ...page.items]));
      setCursor(page.nextCursor);
    } catch {
      if (sequence === requestSequence.current) setError('Past Echoes could not be loaded.');
    } finally {
      if (sequence === requestSequence.current) {
        setLoading(false);
        setBusy(false);
      }
    }
  }, []);

  useEffect(() => {
    const cancelInitialLoad = scheduleDeferredTask(() => void load());
    const unsubscribe = window.talkingQuill.history.onChanged(() => {
      cancelInitialLoad();
      void load();
    });
    return () => {
      requestSequence.current += 1;
      cancelInitialLoad();
      unsubscribe();
    };
  }, [load]);

  const copy = async (id: string) => {
    setError(null);
    setNotice(null);
    try {
      await window.talkingQuill.history.copy(id);
      setNotice({ message: 'Echo copied to the clipboard.', tone: 'success' });
    } catch {
      setError('The Echo could not be copied.');
    }
  };
  const remove = async (id: string) => {
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const result = await window.talkingQuill.history.delete(id);
      if (result.screenshotCleanup !== 'complete') {
        setNotice({
          message:
            'The Echo was deleted, but screenshot cleanup is incomplete and will be retried.',
          tone: 'warning',
        });
      }
    } catch {
      setError('The Echo could not be deleted.');
    } finally {
      setBusy(false);
    }
  };
  const removeAll = async () => {
    setError(null);
    setNotice(null);
    setBusy(true);
    try {
      const result = await window.talkingQuill.history.deleteAll();
      setConfirmDeleteAll(false);
      setNotice(
        result.screenshotCleanup === 'complete'
          ? { message: 'All Past Echoes were deleted.', tone: 'success' }
          : {
              message:
                'All Past Echoes were deleted, but screenshot cleanup is incomplete and will be retried.',
              tone: 'warning',
            },
      );
    } catch {
      setError('Past Echoes could not be deleted.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card title="Past Echoes" description="Completed dictations stored locally on this device.">
      {items.length === 0 ? null : (
        <div className="history-heading-actions">
          <Button variant="danger" onClick={() => setConfirmDeleteAll(true)} disabled={busy}>
            Delete all Past Echoes
          </Button>
        </div>
      )}
      {loading ? (
        <p aria-live="polite">Loading Past Echoes…</p>
      ) : error !== null && items.length === 0 ? (
        <EmptyState title="Unable to load Past Echoes" description={error} />
      ) : items.length === 0 ? (
        <EmptyState
          title="No Past Echoes yet"
          description="Completed dictations will appear here when history is enabled."
        />
      ) : (
        <ol className="history-list" aria-label="Past Echoes">
          {items.map((item) => (
            <li key={item.id} className="history-entry">
              <div className="history-entry__meta">
                <time dateTime={new Date(item.createdAt).toISOString()}>
                  {new Date(item.createdAt).toLocaleString()}
                </time>
                <span>
                  {item.dictationMode === 'quick' ? 'Quick' : 'Extended'} ·{' '}
                  {item.processingMode === 'raw' ? 'Raw' : 'Smart'}
                </span>
              </div>
              <p>{historyDisplayText(item) ?? 'No usable transcript'}</p>
              {item.outcome === 'voice-command' && item.voiceTrigger !== null ? (
                <p className="history-entry__detail">Command trigger: “{item.voiceTrigger}”</p>
              ) : null}
              {item.processingMode === 'smart' &&
              (item.providerId !== null || item.modelId !== null) ? (
                <p className="history-entry__detail" aria-label="Smart provider and model">
                  Provider: {item.providerId ?? 'Unknown'} · Model: {item.modelId ?? 'Unknown'}
                </p>
              ) : null}
              {item.fellBack ? (
                <p className="history-entry__detail history-entry__fallback" role="status">
                  {fallbackDescription(item.errorCategory)}
                </p>
              ) : null}
              <HistoryThumbnail item={item} />
              <div className="history-entry__actions">
                <Button
                  variant="secondary"
                  aria-label={historyActionLabel('Copy', item)}
                  onClick={() => void copy(item.id)}
                  disabled={busy || historyDisplayText(item) === null}
                >
                  Copy
                </Button>
                <Button
                  variant="quiet"
                  aria-label={historyActionLabel('Delete', item)}
                  onClick={() => void remove(item.id)}
                  disabled={busy}
                >
                  Delete
                </Button>
              </div>
            </li>
          ))}
        </ol>
      )}
      {cursor === null || loading ? null : (
        <Button variant="secondary" busy={busy} onClick={() => void load(cursor)}>
          Load more
        </Button>
      )}
      {error === null || items.length === 0 ? null : (
        <Toast tone="error" message={error} onDismiss={() => setError(null)} />
      )}
      {notice === null ? null : (
        <Toast tone={notice.tone} message={notice.message} onDismiss={() => setNotice(null)} />
      )}
      <Dialog
        open={confirmDeleteAll}
        title="Delete all Past Echoes?"
        description="This permanently removes all locally stored history entries."
        onClose={() => setConfirmDeleteAll(false)}
        actions={
          <>
            <Button variant="secondary" onClick={() => setConfirmDeleteAll(false)}>
              Keep Past Echoes
            </Button>
            <Button variant="danger" busy={busy} onClick={() => void removeAll()} data-autofocus>
              Delete all
            </Button>
          </>
        }
      >
        <p>This does not remove settings, credentials, transcription models, or logs.</p>
      </Dialog>
    </Card>
  );
}

function HistoryThumbnail({ item }: { readonly item: HistoryListItem }) {
  const container = useRef<HTMLDivElement>(null);
  const [thumbnail, setThumbnail] = useState<{
    readonly itemId: string;
    readonly objectUrl: string;
  } | null>(null);
  const objectUrl =
    item.hasScreenshot && thumbnail?.itemId === item.id ? thumbnail.objectUrl : null;
  useEffect(() => {
    let active = true;
    let started = false;
    let createdUrl: string | null = null;
    let observer: IntersectionObserver | null = null;
    const load = () => {
      if (!active || started) return;
      started = true;
      void window.talkingQuill.history
        .thumbnail(item.id)
        .then((value) => {
          if (!active || value === null) return;
          const bytes = Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
          createdUrl = URL.createObjectURL(new Blob([bytes], { type: 'image/jpeg' }));
          setThumbnail({ itemId: item.id, objectUrl: createdUrl });
        })
        .catch(() => {
          if (active) setThumbnail(null);
        });
    };
    if (item.hasScreenshot) {
      if (typeof IntersectionObserver === 'undefined') {
        load();
      } else {
        observer = new IntersectionObserver(
          (entries) => {
            if (!entries.some((entry) => entry.isIntersecting)) return;
            observer?.disconnect();
            observer = null;
            load();
          },
          { rootMargin: '200px 0px' },
        );
        if (container.current === null) load();
        else observer.observe(container.current);
      }
    }
    return () => {
      active = false;
      observer?.disconnect();
      if (createdUrl !== null) URL.revokeObjectURL(createdUrl);
    };
  }, [item.hasScreenshot, item.id]);
  return (
    <div ref={container} className="history-entry__screenshot" aria-label="Screenshot attachment">
      {objectUrl === null ? (
        <>
          <span aria-hidden="true">▧</span>
          {item.hasScreenshot ? 'Screenshot unavailable' : 'No screenshot retained'}
        </>
      ) : (
        <img src={objectUrl} alt="On-Screen Awareness context thumbnail" />
      )}
    </div>
  );
}

function scheduleDeferredTask(task: () => void): () => void {
  if (typeof window.requestIdleCallback === 'function') {
    const handle = window.requestIdleCallback(task, { timeout: 250 });
    return () => window.cancelIdleCallback(handle);
  }
  const handle = window.setTimeout(task, 0);
  return () => window.clearTimeout(handle);
}

function fallbackDescription(category: string | null): string {
  if (category === 'pi-authentication-failed') {
    return 'Fell back to Raw because Pi authentication failed.';
  }
  if (category === 'pi-model-not-found' || category === 'pi-no-models') {
    return 'Fell back to Raw because the Pi model was unavailable.';
  }
  if (category?.startsWith('pi-') === true) {
    return 'Fell back to Raw because Pi failed safely.';
  }
  return 'Fell back to Raw. Check the provider in Settings.';
}

function historyDisplayText(item: HistoryListItem): string | null {
  return item.processedText ?? item.voiceSnippet ?? item.rawText;
}

function historyActionLabel(action: 'Copy' | 'Delete', item: HistoryListItem): string {
  const normalized = (historyDisplayText(item) ?? '').replaceAll(/\s+/g, ' ').trim().slice(0, 80);
  const preview = normalized.length === 0 ? 'empty transcript' : normalized;
  return `${action} Echo: ${preview}`;
}
