import { useEffect, useRef, useState } from 'react';
import type { Settings } from '../../../shared/schemas/settings';
import {
  WHISPER_AUTO_LANGUAGE,
  WHISPER_SOURCE_LANGUAGES,
  type WhisperLanguage,
} from '../../../shared/schemas/whisper-languages';
import { Button, Card, Select, Status } from '../../design';
import { ModelSetup } from '../setup/ModelSetup';

export function TranscriptionModelSection({
  settings,
  onSettingsSaved,
  heading = 'Transcription model',
}: {
  readonly settings: Settings;
  readonly onSettingsSaved: (settings: Settings) => void;
  readonly heading?: string | null;
}) {
  return (
    <Card
      {...(heading === null ? {} : { title: heading })}
      description="This is what turns your voice into text. You download it once, then it works on this computer without internet."
    >
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
  const authoritative = settings.transcription.language;
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
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      saveSequence.current += 1;
    };
  }, []);

  const saveLanguage = async () => {
    const operation = ++saveSequence.current;
    const submittedDraft = draft;
    setSaving(true);
    setMessage(null);
    try {
      const saved = await window.talkingQuill.settings.update({
        transcription: { language: submittedDraft },
      });
      if (!mounted.current || operation !== saveSequence.current) return;
      onSettingsSaved(saved);
      setDraft(saved.transcription.language);
      setMessage('Language saved.');
    } catch {
      if (!mounted.current || operation !== saveSequence.current) return;
      // The parent settings event/bootstrap remains authoritative after a rejected write.
      setDraft(authoritative);
      setMessage('That language couldn’t be saved, so your previous choice is still in use.');
    } finally {
      if (mounted.current && operation === saveSequence.current) setSaving(false);
    }
  };

  return (
    <>
      <Select
        label="Language you speak"
        value={draft}
        disabled={saving}
        onChange={(event) => {
          setDraft(event.currentTarget.value as WhisperLanguage);
          setMessage(null);
        }}
      >
        <option value={WHISPER_AUTO_LANGUAGE}>Detect it automatically (recommended)</option>
        {WHISPER_SOURCE_LANGUAGES.map(([code, name]) => (
          <option key={code} value={code}>
            {name} ({code})
          </option>
        ))}
      </Select>
      <Button
        className="language-save-button"
        disabled={saving || draft === authoritative}
        onClick={() => void saveLanguage()}
      >
        {saving ? 'Saving language' : 'Save language'}
      </Button>
      {message === null ? null : (
        <Status tone={message.includes('couldn’t') ? 'error' : 'success'} live>
          {message}
        </Status>
      )}
    </>
  );
}
