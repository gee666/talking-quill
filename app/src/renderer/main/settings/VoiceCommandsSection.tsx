import { useRef, useState, type SyntheticEvent } from 'react';
import type { VoiceCommand } from '../../../shared/schemas/commands';
import { Button, Card, EmptyState, Input, TextArea } from '../../design';

export function VoiceCommandsSection({ commands }: { readonly commands: readonly VoiceCommand[] }) {
  const [editing, setEditing] = useState<VoiceCommand | null>(null);
  const [trigger, setTrigger] = useState('');
  const [snippet, setSnippet] = useState('');
  const [message, setMessage] = useState('');
  const [preview, setPreview] = useState('');
  const [busy, setBusy] = useState(false);
  const triggerRef = useRef<HTMLInputElement>(null);

  const reset = () => {
    setEditing(null);
    setTrigger('');
    setSnippet('');
  };
  const submit = async (event: SyntheticEvent<HTMLFormElement>) => {
    event.preventDefault();
    setBusy(true);
    setMessage('');
    try {
      if (editing === null) await window.talkingQuill.commands.create({ trigger, snippet });
      else await window.talkingQuill.commands.update(editing.id, { trigger, snippet });
      setMessage(editing === null ? 'Voice command added.' : 'Voice command updated.');
      reset();
      queueMicrotask(() => triggerRef.current?.focus());
    } catch (error: unknown) {
      setMessage(error instanceof Error ? error.message : 'The voice command could not be saved.');
    } finally {
      setBusy(false);
    }
  };
  const runPreview = async () => {
    setMessage('');
    try {
      const result = await window.talkingQuill.commands.preview(preview);
      setMessage(
        result === null
          ? 'No voice command matches the full transcript.'
          : `${result.kind === 'exact' ? 'Exact' : 'Fuzzy'} match: say “${result.command.trigger}” → get “${result.command.snippet}”.`,
      );
    } catch (error: unknown) {
      setMessage(publicMessage(error, 'The match preview could not be completed.'));
    }
  };

  return (
    <Card
      title="Voice Commands"
      description="Say a complete trigger phrase to insert its snippet before Smart processing runs."
    >
      <div className="settings-domain">
        <form className="settings-domain__form" onSubmit={submit}>
          <Input
            ref={triggerRef}
            label="Trigger phrase"
            value={trigger}
            maxLength={200}
            required
            onChange={(event) => setTrigger(event.target.value)}
          />
          <TextArea
            label="Snippet"
            value={snippet}
            maxLength={100000}
            required
            rows={4}
            onChange={(event) => setSnippet(event.target.value)}
          />
          <div className="provider-actions">
            <Button type="submit" disabled={busy}>
              {editing === null ? 'Add voice command' : 'Save voice command'}
            </Button>
            {editing === null ? null : (
              <Button type="button" variant="secondary" onClick={reset}>
                Cancel editing
              </Button>
            )}
          </div>
        </form>
        {commands.length === 0 ? (
          <EmptyState
            title="No voice commands"
            description="Add a trigger and the text it should insert."
          />
        ) : (
          <ul className="settings-list" aria-label="Saved voice commands">
            {commands.map((command) => (
              <li key={command.id}>
                <div>
                  <strong>Say “{command.trigger}”</strong>
                  <span aria-hidden="true"> → </span>
                  <span>Get “{command.snippet}”</span>
                </div>
                <div className="provider-actions">
                  <Button
                    variant="secondary"
                    onClick={() => {
                      setEditing(command);
                      setTrigger(command.trigger);
                      setSnippet(command.snippet);
                      queueMicrotask(() => triggerRef.current?.focus());
                    }}
                    aria-label={`Edit ${command.trigger}`}
                  >
                    Edit
                  </Button>
                  <Button
                    variant="danger"
                    onClick={async () => {
                      if (window.confirm(`Delete “${command.trigger}”?`)) {
                        try {
                          await window.talkingQuill.commands.delete(command.id);
                          setMessage('Voice command deleted.');
                        } catch (error: unknown) {
                          setMessage(
                            publicMessage(error, 'The voice command could not be deleted.'),
                          );
                        }
                      }
                    }}
                    aria-label={`Delete ${command.trigger}`}
                  >
                    Delete
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
        <div className="settings-domain__preview">
          <Input
            label="Match preview transcript"
            value={preview}
            maxLength={10000}
            hint="Matching always uses the full transcript."
            onChange={(event) => setPreview(event.target.value)}
          />
          <Button
            variant="secondary"
            disabled={preview.length === 0}
            onClick={() => void runPreview()}
          >
            Preview match
          </Button>
        </div>
        <p className="operation-message" role="status" aria-live="polite">
          {message}
        </p>
      </div>
    </Card>
  );
}

function publicMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message.length > 0 ? error.message : fallback;
}
