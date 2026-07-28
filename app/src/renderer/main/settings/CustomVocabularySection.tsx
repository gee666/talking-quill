import { useState, type SyntheticEvent } from 'react';
import type { VocabularyEntry } from '../../../shared/schemas/vocabulary';
import { Button, Card, EmptyState, Input } from '../../design';
import { publicErrorMessage } from '../public-error';

export function CustomVocabularySection({
  entries,
  heading = 'Custom Vocabulary',
}: {
  readonly entries: readonly VocabularyEntry[];
  readonly heading?: string | null;
}) {
  const [value, setValue] = useState('');
  const [editing, setEditing] = useState<VocabularyEntry | null>(null);
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);

  const submit = async (event: SyntheticEvent<HTMLFormElement>) => {
    event.preventDefault();
    setBusy(true);
    setMessage('');
    try {
      if (editing === null) await window.talkingQuill.vocabulary.create(value);
      else await window.talkingQuill.vocabulary.update(editing.id, value);
      setMessage(editing === null ? 'Vocabulary entry added.' : 'Vocabulary entry updated.');
      setEditing(null);
      setValue('');
    } catch (error: unknown) {
      setMessage(publicErrorMessage(error, 'That word couldn’t be saved.'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      {...(heading === null ? {} : { title: heading })}
      description="Names, brands and words that keep coming out wrong. Add “Kubernetes” once and it stops being misspelled."
    >
      <div className="settings-domain">
        <p className="body-copy">
          This list is used when Smart mode cleans up your words. Raw dictation and voice commands
          don’t use it — they type exactly what you said.
        </p>
        <form className="settings-domain__form" onSubmit={submit}>
          <div className="inline-field-action">
            <Input
              label="Word or phrase"
              hint="One name or phrase at a time, spelled the way you want it written."
              value={value}
              maxLength={200}
              required
              disabled={busy}
              onChange={(event) => setValue(event.target.value)}
            />
            <div className="provider-actions inline-field-action__actions">
              <Button type="submit" disabled={busy}>
                {editing === null ? 'Add to vocabulary' : 'Save vocabulary entry'}
              </Button>
              {editing === null ? null : (
                <Button
                  variant="secondary"
                  disabled={busy}
                  onClick={() => {
                    setEditing(null);
                    setValue('');
                  }}
                >
                  Cancel editing
                </Button>
              )}
            </div>
          </div>
        </form>
        <div className="provider-actions">
          <Button
            variant="secondary"
            onClick={async () => {
              try {
                const result = await window.talkingQuill.vocabulary.importFile();
                setMessage(
                  result.status === 'cancelled'
                    ? 'Import cancelled.'
                    : `Added ${String(result.count)} words.`,
                );
              } catch (error: unknown) {
                setMessage(publicErrorMessage(error, 'Those words couldn’t be imported.'));
              }
            }}
          >
            Import from a text file
          </Button>
          <Button
            variant="secondary"
            onClick={async () => {
              try {
                const result = await window.talkingQuill.vocabulary.exportFile();
                setMessage(
                  result.status === 'cancelled'
                    ? 'Export cancelled.'
                    : `Saved ${String(result.count)} words to a file.`,
                );
              } catch (error: unknown) {
                setMessage(publicErrorMessage(error, 'Those words couldn’t be exported.'));
              }
            }}
          >
            Save to a text file
          </Button>
          <span className="body-copy">
            {entries.length} {entries.length === 1 ? 'entry' : 'entries'}
          </span>
        </div>
        {entries.length === 0 ? (
          <EmptyState
            title="No words yet"
            description="Add one word or phrase at a time, or import a plain text file with one per line."
          />
        ) : (
          <ul className="settings-list" aria-label="Custom vocabulary">
            {entries.map((entry) => (
              <li key={entry.id}>
                <span>{entry.value}</span>
                <div className="provider-actions">
                  <Button
                    variant="secondary"
                    disabled={busy}
                    aria-label={`Edit ${entry.value}`}
                    onClick={() => {
                      setEditing(entry);
                      setValue(entry.value);
                    }}
                  >
                    Edit
                  </Button>
                  <Button
                    variant="danger"
                    disabled={busy}
                    aria-label={`Delete ${entry.value}`}
                    onClick={async () => {
                      try {
                        await window.talkingQuill.vocabulary.delete(entry.id);
                        setMessage('Vocabulary entry deleted.');
                      } catch (error: unknown) {
                        setMessage(publicErrorMessage(error, 'That word couldn’t be deleted.'));
                      }
                    }}
                  >
                    Delete
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
        <p className="operation-message" role="status" aria-live="polite">
          {message}
        </p>
      </div>
    </Card>
  );
}
