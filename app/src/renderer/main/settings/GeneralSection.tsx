import { useEffect, useState } from 'react';
import {
  IDLE_ACTIVATION_TEST,
  type ActivationTestState,
} from '../../../shared/schemas/activation-test';
import type { DictationProfileId } from '../../../shared/schemas/dictation-profiles';
import type { PublicSettingsPatch, Settings } from '../../../shared/schemas/settings';
import { Button, Card, Select, Status, Toggle } from '../../design';
import { formatKeyboardShortcutWithTrigger } from '../format-keyboard-shortcut';

function activationTestLabel(
  state: ActivationTestState,
  settings: Settings,
  platform: string,
  requiredProfileId?: DictationProfileId,
) {
  if (
    requiredProfileId !== undefined &&
    (state.phase === 'quick' || state.phase === 'extended') &&
    state.profileId !== requiredProfileId
  ) {
    const requiredName =
      settings.dictationProfiles.find((profile) => profile.id === requiredProfileId)?.name ??
      requiredProfileId;
    return `That was a different shortcut — try the one for ${requiredName}`;
  }
  if (state.unavailableReason === 'app-disabled') {
    return 'Turn Talking Quill on first, then try your shortcut';
  }
  if (state.unavailableReason === 'session-active') {
    return 'Finish or cancel the dictation you have running, then try again';
  }
  if (state.unavailableReason === 'helper-unavailable') {
    return 'Talking Quill is still getting ready to watch your keyboard';
  }
  switch (state.phase) {
    case 'waiting':
      return 'Go ahead — press your shortcut';
    case 'pressed':
      return 'Holding the last key';
    case 'quick':
      return recognizedActivationLabel('That was quick dictation', state, settings, platform);
    case 'extended':
      return recognizedActivationLabel('That was extended dictation', state, settings, platform);
    case 'idle':
      return 'Not testing right now';
  }
}

function recognizedActivationLabel(
  prefix: string,
  state: ActivationTestState,
  settings: Settings,
  platform: string,
): string {
  if (state.profileId === null || state.shortcut === null) return prefix;
  const profileName =
    settings.dictationProfiles.find((profile) => profile.id === state.profileId)?.name ??
    state.profileId;
  return `${prefix}: ${profileName}, ${formatKeyboardShortcutWithTrigger(state.shortcut, platform)}`;
}

export function GeneralSection({
  settings,
  platform,
  disabled,
  onSave,
  activationTestProfileId,
  heading = 'General',
}: {
  readonly settings: Settings;
  readonly platform: string;
  readonly disabled: boolean;
  readonly onSave: (patch: PublicSettingsPatch, success: string) => Promise<void>;
  readonly activationTestProfileId?: DictationProfileId;
  readonly heading?: string | null;
}) {
  const [gesture, setGesture] = useState(IDLE_ACTIVATION_TEST);
  useEffect(() => {
    const unsubscribe = window.talkingQuill.activationTest.onChanged(setGesture);
    return () => {
      unsubscribe();
      void window.talkingQuill.activationTest.stop();
    };
  }, []);
  const toggleGestureTest = async () => {
    setGesture(
      gesture.active
        ? await window.talkingQuill.activationTest.stop()
        : await window.talkingQuill.activationTest.start(),
    );
  };
  return (
    <Card
      {...(heading === null ? {} : { title: heading })}
      description="How Talking Quill behaves day to day — whether it is listening for your shortcut, how big the widget is, and what it does when you close it."
    >
      <Toggle
        checked={settings.app.enabled}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { app: { enabled: event.currentTarget.checked } },
            'Enabled setting saved on this device.',
          )
        }
        label="Turn Talking Quill on"
        hint="When this is on, your dictation shortcut works in any app. Turn it off to pause Talking Quill without quitting it."
      />
      <div className="gesture-test" aria-live="polite">
        <strong>Try your shortcut safely</strong>
        <span>
          Press your shortcut and Talking Quill will tell you what it saw. Nothing is recorded and
          no text is typed — how long you hold the last key decides quick or extended dictation.
        </span>
        <Status
          tone={
            (gesture.phase === 'quick' || gesture.phase === 'extended') &&
            (activationTestProfileId === undefined || gesture.profileId === activationTestProfileId)
              ? 'success'
              : gesture.active
                ? 'info'
                : 'neutral'
          }
        >
          {activationTestLabel(gesture, settings, platform, activationTestProfileId)}
        </Status>
        <Button
          disabled={disabled || !settings.app.enabled}
          onClick={() => void toggleGestureTest()}
        >
          {gesture.active ? 'Stop shortcut test' : 'Test activation shortcut'}
        </Button>
      </div>
      <Select
        label="Widget size"
        hint="The small window that appears while you dictate. Make it bigger if it is hard to see."
        value={settings.app.widgetSize}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { app: { widgetSize: event.currentTarget.value as Settings['app']['widgetSize'] } },
            'Widget size saved.',
          )
        }
      >
        <option value="default">Default</option>
        <option value="large">Large</option>
        <option value="huge">Huge</option>
        <option value="max">Max</option>
      </Select>
      <Toggle
        checked={settings.app.soundsEnabled}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { app: { soundsEnabled: event.currentTarget.checked } },
            'Sound preference saved.',
          )
        }
        label="Play a sound when recording starts and stops"
        hint="A short cue so you know Talking Quill is listening without looking at the screen."
      />
      <Toggle
        checked={settings.app.launchAtLogin}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { app: { launchAtLogin: event.currentTarget.checked } },
            'Launch at login preference saved.',
          )
        }
        label="Start Talking Quill when I sign in"
        hint="Turn this on if you dictate often, so your shortcut works right away."
      />
      <Toggle
        checked={settings.app.closeToTray}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { app: { closeToTray: event.currentTarget.checked } },
            'Close behavior saved on this device.',
          )
        }
        label="Keep running in the tray when I close the window"
        hint="Your shortcut keeps working after you close the window. Turn it off if you would rather closing the window quit the app."
      />
    </Card>
  );
}
