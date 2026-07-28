import { useCallback, useEffect, useRef, useState } from 'react';
import type { HistoryCursor, HistoryListItem } from '../../../shared/schemas/history';
import { Button, Card, Dialog, EmptyState, Icon, Toast } from '../../design';

export function DictationHistory({
  showHeading = true,
  showDescription = true,
}: {
  readonly showHeading?: boolean;
  readonly showDescription?: boolean;
} = {}) {
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
      if (sequence === requestSequence.current)
        setError('Your dictation history could not be loaded.');
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
      setNotice({ message: 'Copied to your clipboard.', tone: 'success' });
    } catch {
      setError('That transcript could not be copied.');
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
          message: 'Deleted. The screenshot is taking a moment to clear and will go shortly.',
          tone: 'warning',
        });
      }
    } catch {
      setError('That entry could not be deleted.');
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
          ? { message: 'Your dictation history is now empty.', tone: 'success' }
          : {
              message:
                'Your dictation history is now empty. The screenshots are taking a moment to clear and will go shortly.',
              tone: 'warning',
            },
      );
    } catch {
      setError('Your dictation history could not be deleted.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      {...(showHeading ? { title: 'Dictation history' } : {})}
      {...(showDescription
        ? { description: 'Everything you have dictated, kept only on your computer.' }
        : {})}
    >
      {items.length === 0 ? null : (
        <div className="history-heading-actions">
          <Button variant="danger" onClick={() => setConfirmDeleteAll(true)} disabled={busy}>
            Delete all history
          </Button>
        </div>
      )}
      {loading && items.length === 0 ? (
        <p className="body-copy" aria-live="polite">
          Loading your history…
        </p>
      ) : error !== null && items.length === 0 ? (
        <EmptyState title="History could not be loaded" description={error} />
      ) : items.length === 0 ? (
        <EmptyState
          title="Nothing here yet"
          description="Once you dictate something it shows up here, so you can copy it again or delete it."
        />
      ) : (
        <ol className="history-list" aria-label="Dictation history">
          {items.map((item) => (
            <li key={item.id} className="history-entry">
              <div className="history-entry__meta">
                <time dateTime={new Date(item.createdAt).toISOString()}>
                  {new Date(item.createdAt).toLocaleString()}
                </time>
                <span className="history-entry__chips">
                  <span>{item.dictationMode === 'quick' ? 'Quick' : 'Extended'}</span>{' '}
                  <span aria-hidden="true">·</span>{' '}
                  <span>{item.processingMode === 'raw' ? 'Raw' : 'Smart'}</span>
                </span>
              </div>
              <p className="history-entry__text">
                {historyDisplayText(item) ?? 'No words were captured'}
              </p>
              {item.outcome === 'voice-command' && item.voiceTrigger !== null ? (
                <p className="history-entry__detail">You said “{item.voiceTrigger}”</p>
              ) : null}
              {item.processingMode === 'smart' &&
              (item.providerId !== null || item.modelId !== null) ? (
                <p className="history-entry__detail hint" aria-label="Smart provider and model">
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
        <div className="history-more">
          <Button variant="secondary" busy={busy} onClick={() => void load(cursor)}>
            Load more
          </Button>
        </div>
      )}
      {error === null || items.length === 0 ? null : (
        <Toast tone="error" message={error} onDismiss={() => setError(null)} />
      )}
      {notice === null ? null : (
        <Toast tone={notice.tone} message={notice.message} onDismiss={() => setNotice(null)} />
      )}
      <Dialog
        open={confirmDeleteAll}
        title="Delete all history?"
        description="Every entry below is removed for good. There is no undo."
        onClose={() => setConfirmDeleteAll(false)}
        actions={
          <>
            <Button variant="secondary" onClick={() => setConfirmDeleteAll(false)}>
              Keep history
            </Button>
            <Button variant="danger" busy={busy} onClick={() => void removeAll()} data-autofocus>
              Delete all
            </Button>
          </>
        }
      >
        <p>Your settings, your API keys, the speech model and the logs are all left alone.</p>
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
    let cancelFallbackLoad: () => void = () => undefined;
    const load = () => {
      if (!active || started) return;
      started = true;
      cancelFallbackLoad();
      observer?.disconnect();
      observer = null;
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
            if (entries.some((entry) => entry.isIntersecting)) load();
          },
          { rootMargin: '200px 0px' },
        );
        if (container.current === null) load();
        else {
          observer.observe(container.current);
          // Hidden/below-fold history still becomes available after the initial interaction has
          // settled. This bounds lazy loading without making screenshots depend on scrolling.
          cancelFallbackLoad = scheduleDeferredTask(load);
        }
      }
    }
    return () => {
      active = false;
      cancelFallbackLoad();
      observer?.disconnect();
      if (createdUrl !== null) URL.revokeObjectURL(createdUrl);
    };
  }, [item.hasScreenshot, item.id]);
  return (
    <div ref={container} className="history-entry__screenshot" aria-label="Screenshot attachment">
      {objectUrl === null ? (
        <>
          <Icon name="profiles" />
          {item.hasScreenshot ? 'Screenshot unavailable' : 'No screenshot kept'}
        </>
      ) : (
        <img src={objectUrl} alt="On-Screen Awareness context thumbnail" />
      )}
    </div>
  );
}

function scheduleDeferredTask(task: () => void): () => void {
  let pending = true;
  const run = () => {
    if (!pending) return;
    pending = false;
    task();
  };
  if (typeof window.requestIdleCallback === 'function') {
    const handle = window.requestIdleCallback(run, { timeout: 250 });
    return () => {
      if (!pending) return;
      pending = false;
      window.cancelIdleCallback(handle);
    };
  }
  const handle = window.setTimeout(run, 0);
  return () => {
    if (!pending) return;
    pending = false;
    window.clearTimeout(handle);
  };
}

function fallbackDescription(category: string | null): string {
  if (category === 'pi-authentication-failed') {
    return 'The AI service could not sign in, so your words were inserted exactly as you said them.';
  }
  if (category === 'pi-model-not-found' || category === 'pi-no-models') {
    return 'The AI model was not available, so your words were inserted exactly as you said them.';
  }
  if (category?.startsWith('pi-') === true) {
    return 'The AI clean-up did not work, so your words were inserted exactly as you said them.';
  }
  return 'The AI clean-up did not run, so your words were inserted as you said them. Check Smart processing in Settings.';
}

function historyDisplayText(item: HistoryListItem): string | null {
  return item.processedText ?? item.voiceSnippet ?? item.rawText;
}

function historyActionLabel(action: 'Copy' | 'Delete', item: HistoryListItem): string {
  const normalized = (historyDisplayText(item) ?? '').replaceAll(/\s+/g, ' ').trim().slice(0, 80);
  const preview = normalized.length === 0 ? 'empty transcript' : normalized;
  return `${action} transcript: ${preview}`;
}
