import { useState, type SyntheticEvent } from 'react';
import type { VocabularyEntry } from '../../../shared/schemas/vocabulary';
import { Button, Card, EmptyState, Input } from '../../design';
import { publicErrorMessage } from '../public-error';

export function CustomVocabularySection({
  entries,
}: {
  readonly entries: readonly VocabularyEntry[];
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
      setMessage(publicErrorMessage(error, 'The vocabulary entry could not be saved.'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      title="Custom Vocabulary"
      description="Add names and phrases whose spelling Smart Transcription should preserve."
    >
      <div className="settings-domain">
        <p className="body-copy">
          <strong>Custom vocabulary applies to Smart Transcription only.</strong> Raw Transcription
          and Voice Commands never use this list.
        </p>
        <form className="settings-domain__form settings-domain__form--row" onSubmit={submit}>
          <Input
            label="Word or phrase"
            value={value}
            maxLength={200}
            required
            disabled={busy}
            onChange={(event) => setValue(event.target.value)}
          />
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
                    : `Imported ${String(result.count)} vocabulary entries.`,
                );
              } catch (error: unknown) {
                setMessage(publicErrorMessage(error, 'Vocabulary could not be imported.'));
              }
            }}
          >
            Import plain text
          </Button>
          <Button
            variant="secondary"
            onClick={async () => {
              try {
                const result = await window.talkingQuill.vocabulary.exportFile();
                setMessage(
                  result.status === 'cancelled'
                    ? 'Export cancelled.'
                    : `Exported ${String(result.count)} vocabulary entries.`,
                );
              } catch (error: unknown) {
                setMessage(publicErrorMessage(error, 'Vocabulary could not be exported.'));
              }
            }}
          >
            Export plain text
          </Button>
          <span className="body-copy">
            {entries.length} {entries.length === 1 ? 'entry' : 'entries'}
          </span>
        </div>
        {entries.length === 0 ? (
          <EmptyState
            title="Custom vocabulary is empty"
            description="Use one word or phrase per entry, or import a UTF-8 text file."
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
                        setMessage(
                          publicErrorMessage(error, 'The vocabulary entry could not be deleted.'),
                        );
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
