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
    return `Use the ${requiredName} profile shortcut; a different shortcut was recognized`;
  }
  if (state.unavailableReason === 'app-disabled') {
    return 'Enable Talking Quill before testing the shortcut';
  }
  if (state.unavailableReason === 'session-active') {
    return 'Finish or cancel the active dictation, then try again';
  }
  if (state.unavailableReason === 'helper-unavailable') {
    return 'The native keyboard helper must be ready before testing';
  }
  switch (state.phase) {
    case 'waiting':
      return 'Waiting for shortcut';
    case 'pressed':
      return 'Shortcut final trigger held';
    case 'quick':
      return recognizedActivationLabel(
        'Quick Dictation gesture recognized',
        state,
        settings,
        platform,
      );
    case 'extended':
      return recognizedActivationLabel(
        'Extended Dictation gesture recognized',
        state,
        settings,
        platform,
      );
    case 'idle':
      return 'Shortcut test inactive';
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
}: {
  readonly settings: Settings;
  readonly platform: string;
  readonly disabled: boolean;
  readonly onSave: (patch: PublicSettingsPatch, success: string) => Promise<void>;
  readonly activationTestProfileId?: DictationProfileId;
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
    <Card title="General" description="Activation, processing, widget, and application behavior.">
      <Toggle
        checked={settings.app.enabled}
        disabled={disabled}
        onChange={(event) =>
          void onSave(
            { app: { enabled: event.currentTarget.checked } },
            'Enabled setting saved on this device.',
          )
        }
        label="Enable Talking Quill"
        hint="Makes the system-wide activation shortcut available."
      />
      <div className="setting-divider" />
      <div className="gesture-test" aria-live="polite">
        <strong>Live gesture test</strong>
        <span>
          This safely tests the helper chord without opening the microphone or inserting text. The
          final letter key down/up controls Quick versus Extended timing.
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
        label="Session sounds"
        hint="Play short local cues when recording starts and finishes."
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
        label="Launch at login"
        hint="Start Talking Quill after you sign in to this computer."
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
        label="Close to tray"
        hint="When off, closing the main window quits Talking Quill normally."
      />
    </Card>
  );
}
