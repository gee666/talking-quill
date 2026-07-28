import { useId, useRef, useState, type SyntheticEvent } from 'react';
import type { VoiceCommand } from '../../../shared/schemas/commands';
import { Button, Card, EmptyState, Input, TextArea } from '../../design';
import { publicErrorMessage } from '../public-error';

export function VoiceCommandsSection({
  commands,
  heading = 'Voice Commands',
}: {
  readonly commands: readonly VoiceCommand[];
  readonly heading?: string | null;
}) {
  const [editing, setEditing] = useState<VoiceCommand | null>(null);
  const [trigger, setTrigger] = useState('');
  const [snippet, setSnippet] = useState('');
  const [message, setMessage] = useState('');
  const [preview, setPreview] = useState('');
  const [busy, setBusy] = useState(false);
  const triggerRef = useRef<HTMLInputElement>(null);
  const previewGeneration = useRef(0);
  const previewHeadingId = useId();

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
      setMessage(publicErrorMessage(error, 'That voice command couldn’t be saved.'));
    } finally {
      setBusy(false);
    }
  };
  const runPreview = async () => {
    const generation = ++previewGeneration.current;
    setMessage('');
    try {
      const result = await window.talkingQuill.commands.preview(preview);
      if (generation !== previewGeneration.current) return;
      setMessage(
        result === null
          ? 'Nothing matched — you need to say the whole phrase on its own.'
          : `${result.kind === 'exact' ? 'Exact' : 'Close'} match: say “${result.command.trigger}” → get “${result.command.snippet}”.`,
      );
    } catch (error: unknown) {
      if (generation === previewGeneration.current) {
        setMessage(publicErrorMessage(error, 'The preview couldn’t run. Please try again.'));
      }
    }
  };

  return (
    <Card
      {...(heading === null ? {} : { title: heading })}
      description="Shortcuts for text you type all the time. Say “my address” and Talking Quill types your address instead."
    >
      <div className="settings-domain">
        <p className="body-copy">
          Say the phrase on its own and it is swapped for the text you saved. This happens before
          any AI clean-up, so what you saved comes out exactly as you wrote it.
        </p>
        <section className="settings-preview-card" aria-labelledby={previewHeadingId}>
          <h3 id={previewHeadingId}>Try it out</h3>
          <div className="settings-domain__preview inline-field-action">
            <Input
              label="What would you say?"
              value={preview}
              maxLength={10000}
              hint="Type what you would say to check which command it matches."
              onChange={(event) => {
                previewGeneration.current += 1;
                setPreview(event.target.value);
              }}
            />
            <div className="provider-actions inline-field-action__actions">
              <Button
                variant="secondary"
                disabled={preview.length === 0}
                onClick={() => void runPreview()}
              >
                Check for a match
              </Button>
            </div>
          </div>
        </section>
        <form className="settings-domain__form" onSubmit={submit}>
          <Input
            ref={triggerRef}
            label="When I say"
            hint="For example: my address"
            value={trigger}
            maxLength={200}
            required
            disabled={busy}
            onChange={(event) => setTrigger(event.target.value)}
          />
          <TextArea
            label="Type this instead"
            value={snippet}
            maxLength={100000}
            required
            rows={4}
            disabled={busy}
            onChange={(event) => setSnippet(event.target.value)}
          />
          <div className="provider-actions">
            <Button type="submit" disabled={busy}>
              {editing === null ? 'Add voice command' : 'Save voice command'}
            </Button>
            {editing === null ? null : (
              <Button type="button" variant="secondary" disabled={busy} onClick={reset}>
                Cancel editing
              </Button>
            )}
          </div>
        </form>
        {commands.length === 0 ? (
          <EmptyState
            title="No voice commands yet"
            description="Add a phrase to say and the text it should type for you."
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
                    disabled={busy}
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
                    disabled={busy}
                    onClick={async () => {
                      if (window.confirm(`Delete “${command.trigger}”?`)) {
                        try {
                          await window.talkingQuill.commands.delete(command.id);
                          setMessage('Voice command deleted.');
                        } catch (error: unknown) {
                          setMessage(
                            publicErrorMessage(error, 'That voice command couldn’t be deleted.'),
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
        <p className="operation-message" role="status" aria-live="polite">
          {message}
        </p>
      </div>
    </Card>
  );
}
