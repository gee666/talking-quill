import { useEffect, useRef, useState } from 'react';
import type { Settings } from '../../../shared/schemas/settings';
import { Button, Card, Input, Status } from '../../design';
import { ModelSetup } from '../setup/ModelSetup';

export function TranscriptionModelSection({
  settings,
  onSettingsSaved,
}: {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
}) {
  return (
    <Card title="Transcription model" description="Download and manage the local Whisper model.">
      <ModelSetup settings={settings} onSettingsSaved={onSettingsSaved} />
      <div className="setting-divider" />
      <TranscriptionLanguageSetting settings={settings} onSettingsSaved={onSettingsSaved} />
    </Card>
  );
}

export function TranscriptionLanguageSetting({
  settings,
  onSettingsSaved,
}: {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
}) {
  const authoritative = settings.transcription.language ?? '';
  const [draft, setDraft] = useState(authoritative);
  const [observedAuthoritative, setObservedAuthoritative] = useState(authoritative);
  const [message, setMessage] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const saveSequence = useRef(0);
  const mounted = useRef(true);

  if (authoritative !== observedAuthoritative) {
    setObservedAuthoritative(authoritative);
    setDraft(authoritative);
    setMessage(null);
  }
  useEffect(
    () => () => {
      mounted.current = false;
      saveSequence.current += 1;
    },
    [],
  );

  const saveLanguage = async () => {
    const operation = ++saveSequence.current;
    const submittedDraft = draft;
    const normalized = submittedDraft.trim();
    setSaving(true);
    setMessage(null);
    try {
      const saved = await window.talkingQuill.settings.update({
        transcription: { language: normalized.length === 0 ? null : normalized },
      });
      if (!mounted.current || operation !== saveSequence.current) return;
      onSettingsSaved(saved);
      setDraft(saved.transcription.language ?? '');
      setMessage('Transcription language saved.');
    } catch {
      if (!mounted.current || operation !== saveSequence.current) return;
      // The parent settings event/bootstrap remains authoritative after a rejected write.
      setDraft(authoritative);
      setMessage(
        'The transcription language could not be saved. Your previous value was restored.',
      );
    } finally {
      if (mounted.current && operation === saveSequence.current) setSaving(false);
    }
  };

  return (
    <>
      <Input
        label="Transcription language"
        value={draft}
        placeholder="Auto-detect"
        hint="Leave empty to detect automatically, or enter a language name or code."
        onChange={(event) => {
          setDraft(event.currentTarget.value);
          setMessage(null);
        }}
        onBlur={() => void saveLanguage()}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            void saveLanguage();
          }
        }}
      />
      <Button disabled={saving || draft === authoritative} onClick={() => void saveLanguage()}>
        {saving ? 'Saving language' : 'Save language'}
      </Button>
      {message === null ? null : (
        <Status tone={message.includes('could not') ? 'error' : 'success'} live>
          {message}
        </Status>
      )}
    </>
  );
}
