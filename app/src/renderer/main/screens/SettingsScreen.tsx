import { useEffect, useRef, useState, type RefObject, type ReactNode } from 'react';
import type { Settings } from '../../../shared/schemas/settings';
import { Button, EmptyState, Icon, Input, Status, Toast, type IconName } from '../../design';
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
  onOpenWelcome,
}: {
  readonly headingRef: RefObject<HTMLHeadingElement | null>;
  readonly settings: Settings;
  readonly platform: string;
  readonly onSettingsSaved: (settings: Settings) => void;
  readonly onOpenWelcome: () => void;
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
      setNotice({ tone: 'error', message: 'That change didn’t save. Please try again.' });
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
      keywords:
        'Enable Talking Quill turn on off launch at login start with computer close to tray widget size sounds startup try your shortcut safely test activation shortcut',
      node: (
        <GeneralSection
          settings={settings}
          platform={platform}
          disabled={saving}
          onSave={saveGeneral}
          heading={null}
        />
      ),
    },
    {
      title: 'Dictation profiles',
      keywords:
        'Dictation profiles shortcuts shortcut chord binding keyboard Ctrl Control Alt Option Shift Win Command General Prompt Markdown Translate English custom add create delete raw smart processing what happens to your words type it exactly clean it up extra instructions prompt reset formatting',
      node: (
        <DictationProfilesSection
          settings={settings}
          platform={platform}
          onSettingsSaved={onSettingsSaved}
          heading={null}
        />
      ),
    },
    {
      title: 'Recording',
      keywords:
        'Recording microphone preferred device audio system sounds loopback calls meetings music live level test my microphone permission automatic manual finish Enter pause silence detection how long a pause aggressive average relaxed short medium long system default disconnected',
      node: <RecordingSection settings={settings} platform={platform} heading={null} />,
    },
    {
      title: 'Transcription model',
      keywords:
        'Transcription model speech model Whisper small large faster more accurate offline selected status download progress pause cancel retry delete redownload repair corrupt offline cache location spoken source language transcription auto detect',
      node: (
        <TranscriptionModelSection
          settings={settings}
          onSettingsSaved={onSettingsSaved}
          heading={null}
        />
      ),
    },
    {
      title: 'Privacy & data',
      keywords:
        'Privacy data dictation history enabled keep a list of what you dictated screenshots picture of your screen retention how long duration delete everything delete all reset start over diagnostic logging technical event names',
      node: (
        <PrivacySection settings={settings} disabled={saving} onSave={saveGeneral} heading={null} />
      ),
    },
    {
      title: 'Smart processing',
      keywords:
        'Smart processing provider model discovery connection test Local LAN Cloud cloud cost Ollama OpenAI Anthropic Gemini Azure AWS Bedrock Cohere on-screen awareness screenshot vision override credentials API key endpoint',
      node: (
        <SmartProcessingSection
          settings={settings}
          onSettingsSaved={onSettingsSaved}
          heading={null}
          // A search match can make this the rendered section without the user choosing it; only an
          // explicit selection may let it contact the configured AI service.
          autoDiscover={selected === 'Smart processing'}
        />
      ),
    },
    {
      title: 'Voice Commands',
      keywords:
        'Voice Commands command trigger phrase snippet shortcut for text say my address create edit delete match preview',
      node: <VoiceCommandsSection commands={settings.voiceCommands} heading={null} />,
    },
    {
      title: 'Custom Vocabulary',
      keywords:
        'Custom Vocabulary words names phrases spelling entries import export text applies to Smart only create edit delete',
      node: <CustomVocabularySection entries={settings.customVocabulary} heading={null} />,
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
            {active?.title ?? 'Settings'}
          </h1>
          <p className="body-copy">
            Everything you change here is saved on this computer right away.
          </p>
        </div>
        <div className="provider-actions">
          <Button id="reopen-welcome" variant="secondary" onClick={onOpenWelcome}>
            Run setup again
          </Button>
          <Status tone={saving ? 'info' : notice?.tone === 'error' ? 'error' : 'neutral'} live>
            {saving ? 'Saving' : notice?.tone === 'error' ? 'Not saved' : 'Saved on this computer'}
          </Status>
        </div>
      </header>
      <p className="sr-only" role="status" aria-live="polite">
        {visible.length === 0
          ? 'Nothing matches your search.'
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
              title="Nothing matches your search"
              description="Try a simpler word, or clear the search box to see everything again."
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
