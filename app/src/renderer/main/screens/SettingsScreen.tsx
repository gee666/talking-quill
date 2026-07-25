import { useEffect, useRef, useState, type RefObject, type ReactNode } from 'react';
import type { Settings } from '../../../shared/schemas/settings';
import { EmptyState, Icon, Input, Status, Toast, type IconName } from '../../design';
import { SmartProcessingSection } from '../SmartProcessingSection';
import { RecordingSection } from '../settings/RecordingSection';
import { GeneralSection } from '../settings/GeneralSection';
import { PrivacySection } from '../settings/PrivacySection';
import { VoiceCommandsSection } from '../settings/VoiceCommandsSection';
import { CustomVocabularySection } from '../settings/CustomVocabularySection';
import { TranscriptionModelSection } from '../settings/TranscriptionModelSection';
import { DictationProfilesSection } from '../settings/DictationProfilesSection';
interface Notice {
  readonly tone: 'success' | 'error';
  readonly message: string;
}
const SECTION_ICONS: Record<string, IconName> = {
  General: 'general',
  'Dictation profiles': 'profiles',
  Recording: 'recording',
  'Transcription model': 'model',
  'Privacy & data': 'privacy',
  'Smart processing': 'smart',
  'Voice Commands': 'commands',
  'Custom Vocabulary': 'vocabulary',
};
export function SettingsScreen({
  headingRef,
  settings,
  platform,
  onSettingsSaved,
}: {
  readonly headingRef: RefObject<HTMLHeadingElement | null>;
  readonly settings: Settings;
  readonly platform: string;
  readonly onSettingsSaved: (settings: Settings) => void;
}) {
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState('General');
  const latestSettings = useRef(settings);
  useEffect(() => {
    latestSettings.current = settings;
  }, [settings]);
  const saveGeneral = async (
    patch: Parameters<typeof window.talkingQuill.settings.update>[0],
    success: string,
  ) => {
    setSaving(true);
    setNotice(null);
    try {
      onSettingsSaved(await window.talkingQuill.settings.update(patch));
      setNotice({ tone: 'success', message: success });
    } catch {
      const observedBeforeReload = latestSettings.current;
      try {
        const authoritative = (await window.talkingQuill.app.getBootstrap()).settings;
        onSettingsSaved(
          latestSettings.current === observedBeforeReload ? authoritative : latestSettings.current,
        );
      } catch {
        // The settings event stream remains authoritative if shutdown prevents a reload.
      }
      setNotice({ tone: 'error', message: 'The setting could not be saved.' });
    } finally {
      setSaving(false);
    }
  };
  const queryTokens = normalizeSearch(query).split(' ').filter(Boolean);
  const sections: readonly {
    readonly title: string;
    readonly keywords: string;
    readonly node: ReactNode;
  }[] = [
    {
      title: 'General',
      keywords: 'Enable Talking Quill launch at login close to tray widget size sounds startup',
      node: <GeneralSection settings={settings} disabled={saving} onSave={saveGeneral} />,
    },
    {
      title: 'Dictation profiles',
      keywords:
        'Dictation profiles shortcut binding Alt Option Shift General Prompt custom raw smart processing prompt reset',
      node: <DictationProfilesSection settings={settings} onSettingsSaved={onSettingsSaved} />,
    },
    {
      title: 'Recording',
      keywords:
        'Recording microphone preferred device audio live level test permission silence detection aggressive average relaxed system default disconnected',
      node: <RecordingSection settings={settings} platform={platform} />,
    },
    {
      title: 'Transcription model',
      keywords:
        'Transcription model Whisper small large selected status download progress pause cancel retry delete redownload repair corrupt offline cache location language auto detect',
      node: <TranscriptionModelSection settings={settings} onSettingsSaved={onSettingsSaved} />,
    },
    {
      title: 'Privacy & data',
      keywords:
        'Privacy data Past Echoes history enabled screenshots retention duration delete all reset diagnostic logging',
      node: <PrivacySection settings={settings} disabled={saving} onSave={saveGeneral} />,
    },
    {
      title: 'Smart processing',
      keywords:
        'Smart processing provider model discovery connection test Local LAN Cloud cloud cost Ollama OpenAI Anthropic Gemini Azure AWS Bedrock Cohere on-screen awareness screenshot vision override credentials API key endpoint',
      node: <SmartProcessingSection settings={settings} onSettingsSaved={onSettingsSaved} />,
    },
    {
      title: 'Voice Commands',
      keywords: 'Voice Commands command trigger phrase snippet create edit delete match preview',
      node: <VoiceCommandsSection commands={settings.voiceCommands} />,
    },
    {
      title: 'Custom Vocabulary',
      keywords:
        'Custom Vocabulary words phrases entries import export text applies to Smart only create edit delete',
      node: <CustomVocabularySection entries={settings.customVocabulary} />,
    },
  ];
  const visible = sections.filter(({ title, keywords }) => {
    const index = normalizeSearch(`${title} ${keywords}`);
    return queryTokens.every((token) => index.includes(token));
  });
  const active = visible.find((section) => section.title === selected) ?? visible[0];
  return (
    <div className="screen screen--settings">
      <header className="screen__header">
        <div>
          <p className="eyebrow">Settings</p>
          <h1 ref={headingRef} tabIndex={-1}>
            General settings
          </h1>
          <p>Search and manage validated local preferences.</p>
        </div>
        <Status tone={saving ? 'info' : notice?.tone === 'error' ? 'error' : 'success'} live>
          {saving ? 'Saving' : notice?.tone === 'error' ? 'Save failed' : 'Saved locally'}
        </Status>
      </header>
      <p className="sr-only" role="status" aria-live="polite">
        {visible.length === 0
          ? 'No matching settings.'
          : `${String(visible.length)} settings sections shown.`}
      </p>
      <div className="settings-layout">
        <div className="settings-rail">
          <Input
            type="search"
            label="Search settings"
            value={query}
            placeholder="Search…"
            onChange={(event) => setQuery(event.currentTarget.value)}
          />
          <nav aria-label="Settings sections">
            {visible.map(({ title }) => (
              <button
                key={title}
                className="nav-item"
                aria-current={active?.title === title ? 'page' : undefined}
                onClick={() => setSelected(title)}
              >
                <Icon name={SECTION_ICONS[title] ?? 'general'} />
                <span>{title}</span>
              </button>
            ))}
          </nav>
        </div>
        <div className="settings-panel">
          {active === undefined ? (
            <EmptyState
              title="No matching settings"
              description="Try a broader word or clear the search."
            />
          ) : (
            <section key={active.title} aria-label={active.title}>
              {active.node}
            </section>
          )}
        </div>
      </div>
      {notice ? (
        <Toast tone={notice.tone} message={notice.message} onDismiss={() => setNotice(null)} />
      ) : null}
    </div>
  );
}

function normalizeSearch(value: string): string {
  return value
    .normalize('NFKD')
    .replace(/\p{Mark}/gu, '')
    .toLocaleLowerCase()
    .replace(/[^\p{Letter}\p{Number}]+/gu, ' ')
    .trim();
}
