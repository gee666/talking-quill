import { SILENCE_PRESET_MS, SPEECH_ARMING_MS } from '../../../shared/constants/audio';
import type { Settings } from '../../../shared/schemas/settings';
import { Select, Toggle } from '../../design';

export type RecordingOptionPatch = Partial<
  Pick<Settings['recording'], 'silencePreset' | 'autoSubmitOnSilence' | 'includeSystemAudio'>
>;

export function RecordingOptions({
  recording,
  platform,
  disabled,
  onSave,
}: {
  readonly recording: Settings['recording'];
  readonly platform: string;
  readonly disabled: boolean;
  readonly onSave: (patch: RecordingOptionPatch, success: string, failure: string) => Promise<void>;
}) {
  return (
    <>
      <div className="setting-divider" />
      <Toggle
        label="Automatically finish after a pause"
        hint="Turn this off when background noise makes Talking Quill finish too early. You will need to press Enter or repeat your shortcut when you are done."
        checked={recording.autoSubmitOnSilence}
        disabled={disabled}
        onChange={(event) => {
          const enabled = event.currentTarget.checked;
          void onSave(
            { autoSubmitOnSilence: enabled },
            enabled ? 'Automatic finishing enabled.' : 'Manual finishing enabled.',
            'That finishing option couldn’t be saved.',
          );
        }}
      />
      <Select
        label="How long a pause ends a dictation"
        hint={`When you stop talking for this long, Talking Quill decides you are done. Quick dictation waits until it has heard at least ${formatSeconds(SPEECH_ARMING_MS)} of speech first.`}
        value={recording.silencePreset}
        disabled={disabled || !recording.autoSubmitOnSilence}
        onChange={(event) => {
          const silencePreset = event.currentTarget.value;
          if (
            silencePreset !== 'aggressive' &&
            silencePreset !== 'average' &&
            silencePreset !== 'relaxed'
          )
            return;
          void onSave(
            { silencePreset },
            'Pause length saved.',
            'That pause length couldn’t be saved. Please try again.',
          );
        }}
      >
        <option value="aggressive">
          Short pause — {formatSeconds(SILENCE_PRESET_MS.aggressive)}
        </option>
        <option value="average">Normal pause — {formatSeconds(SILENCE_PRESET_MS.average)}</option>
        <option value="relaxed">Long pause — {formatSeconds(SILENCE_PRESET_MS.relaxed)}</option>
      </Select>
      <div className="setting-divider" />
      <Toggle
        label="Include system audio"
        hint={
          platform === 'win32'
            ? 'Capture sounds from calls, videos, and other apps together with your microphone. Leave this off for microphone-only dictation, such as while listening to music.'
            : 'System-audio capture is currently available on Windows only. Microphone capture is unchanged.'
        }
        checked={recording.includeSystemAudio}
        disabled={disabled || (platform !== 'win32' && !recording.includeSystemAudio)}
        onChange={(event) => {
          const enabled = event.currentTarget.checked;
          void onSave(
            { includeSystemAudio: enabled },
            enabled ? 'System audio will be captured.' : 'Microphone-only capture enabled.',
            'That audio-source option couldn’t be saved.',
          );
        }}
      />
    </>
  );
}

function formatSeconds(milliseconds: number): string {
  return `${(milliseconds / 1_000).toFixed(1)} seconds`;
}
