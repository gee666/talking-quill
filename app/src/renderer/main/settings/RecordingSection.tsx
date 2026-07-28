import { useEffect, useRef, useState } from 'react';
import { SILENCE_PRESET_MS, SPEECH_ARMING_MS } from '../../../shared/constants/audio';
import type { MicrophoneDeviceList, MicrophoneTestState } from '../../../shared/schemas/audio';
import type { Settings } from '../../../shared/schemas/settings';
import { Button, Card, Progress, Select, Status, Toast } from '../../design';

interface Notice {
  readonly tone: 'success' | 'error';
  readonly message: string;
}

export function RecordingSection({
  settings,
  platform,
  heading = 'Recording',
}: {
  readonly settings: Settings;
  readonly platform: string;
  readonly heading?: string | null;
}) {
  const [devices, setDevices] = useState<MicrophoneDeviceList | null>(null);
  const [testState, setTestState] = useState<MicrophoneTestState>({
    status: 'idle',
    permission: 'not-determined',
  });
  const [level, setLevel] = useState(0);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const mounted = useRef(true);
  const operation = useRef(0);
  const activeCaptureId = useRef<string | null>(null);
  const testing = testState.status === 'starting' || testState.status === 'active';
  const isCurrentOperation = (value: number) => mounted.current && operation.current === value;

  useEffect(() => {
    mounted.current = true;
    const applyDevices = (next: MicrophoneDeviceList) => {
      if (!mounted.current) return;
      setDevices(next);
      const permission = next.permission;
      if (permission === 'denied' || permission === 'restricted') {
        setTestState((current) => {
          if (current.status === 'active' || current.status === 'starting') return current;
          activeCaptureId.current = null;
          return {
            status: 'blocked',
            permission,
            reason: 'microphone-permission',
          };
        });
      }
    };
    const removeDevices = window.talkingQuill.recording.onDevicesChanged(applyDevices);
    const removeLevel = window.talkingQuill.recording.onTestLevel((next) => {
      if (mounted.current && activeCaptureId.current === next.captureId) setLevel(next.rms);
    });
    const removeState = window.talkingQuill.recording.onTestStateChanged((next) => {
      if (!mounted.current) return;
      activeCaptureId.current = next.status === 'active' ? next.captureId : null;
      setTestState(next);
      if (next.status !== 'active') setLevel(0);
    });
    void window.talkingQuill.recording.getDevices().then(
      (next) => applyDevices(next),
      () => {
        if (mounted.current) {
          setNotice({ tone: 'error', message: 'Talking Quill couldn’t find your microphones.' });
        }
      },
    );
    return () => {
      mounted.current = false;
      activeCaptureId.current = null;
      operation.current += 1;
      removeDevices();
      removeLevel();
      removeState();
      void window.talkingQuill.recording.stopTest();
    };
  }, []);

  const startTest = async () => {
    const currentOperation = ++operation.current;
    setNotice(null);
    setTestState({ status: 'starting', permission: testState.permission });
    try {
      const next = await window.talkingQuill.recording.startTest();
      if (mounted.current && currentOperation === operation.current) {
        activeCaptureId.current = next.status === 'active' ? next.captureId : null;
        setTestState(next);
      } else {
        await window.talkingQuill.recording.stopTest();
      }
    } catch {
      if (mounted.current && currentOperation === operation.current) {
        setTestState({
          status: 'unavailable',
          permission: testState.permission,
          reason: 'capture-unavailable',
        });
      }
    }
  };

  const stopTest = async () => {
    ++operation.current;
    try {
      const next = await window.talkingQuill.recording.stopTest();
      if (mounted.current) {
        activeCaptureId.current = null;
        setTestState(next);
      }
    } finally {
      if (mounted.current) setLevel(0);
    }
  };

  const saveMicrophone = async (value: string) => {
    const currentOperation = ++operation.current;
    const restart = testing;
    setSaving(true);
    setNotice(null);
    try {
      if (restart) {
        const stopped = await window.talkingQuill.recording.stopTest();
        if (!isCurrentOperation(currentOperation)) return;
        activeCaptureId.current = null;
        setTestState(stopped);
        setLevel(0);
      }
      await window.talkingQuill.settings.update({
        recording: { preferredMicrophoneId: value.length === 0 ? null : value },
      });
      if (!isCurrentOperation(currentOperation)) return;
      setNotice({ tone: 'success', message: 'Microphone saved.' });
      if (restart) {
        const restarted = await window.talkingQuill.recording.startTest();
        if (isCurrentOperation(currentOperation)) {
          activeCaptureId.current = restarted.status === 'active' ? restarted.captureId : null;
          setTestState(restarted);
        } else {
          await window.talkingQuill.recording.stopTest();
        }
      }
    } catch {
      if (isCurrentOperation(currentOperation)) {
        setNotice({
          tone: 'error',
          message: 'That microphone couldn’t be saved. Please try again.',
        });
      }
    } finally {
      if (isCurrentOperation(currentOperation)) setSaving(false);
    }
  };

  const openMicrophoneSettings = async () => {
    setNotice(null);
    try {
      await window.talkingQuill.recording.openMicrophoneSettings();
    } catch {
      if (mounted.current) {
        setNotice({
          tone: 'error',
          message: 'Talking Quill couldn’t open your microphone settings.',
        });
      }
    }
  };

  const savePreset = async (value: Settings['recording']['silencePreset']) => {
    setSaving(true);
    setNotice(null);
    try {
      await window.talkingQuill.settings.update({ recording: { silencePreset: value } });
      setNotice({ tone: 'success', message: 'Pause length saved.' });
    } catch {
      setNotice({
        tone: 'error',
        message: 'That pause length couldn’t be saved. Please try again.',
      });
    } finally {
      if (mounted.current) setSaving(false);
    }
  };

  const preferred = settings.recording.preferredMicrophoneId;
  const preferredMissing =
    preferred !== null &&
    devices !== null &&
    !devices.devices.some((device) => device.deviceId === preferred);
  const blocked = testState.status === 'blocked';
  const unavailable = testState.status === 'unavailable';
  const unavailableGuidance =
    testState.status !== 'unavailable'
      ? null
      : testState.reason === 'no-device'
        ? 'No microphone found. Plug one in or turn it on, then test again.'
        : testState.reason === 'device-unavailable'
          ? 'That microphone isn’t available. It may be unplugged or in use by another app. Pick a different one, or close the app using it.'
          : testState.reason === 'permission-unavailable'
            ? 'Talking Quill couldn’t ask for microphone access. Restart Talking Quill and test again.'
            : testState.reason === 'unsupported-audio-format'
              ? 'Talking Quill can’t record from this microphone. Try another one.'
              : 'Talking Quill couldn’t start recording. Restart it and test again.';

  return (
    <Card
      {...(heading === null ? {} : { title: heading })}
      description="Choose which microphone to listen with and how long a pause should end a dictation. Your audio never leaves this computer."
    >
      <Select
        label="Microphone"
        hint="If you unplug the one you picked, Talking Quill remembers it and uses it again when it comes back."
        value={preferred ?? ''}
        disabled={saving || devices === null}
        onChange={(event) => void saveMicrophone(event.currentTarget.value)}
      >
        <option value="">Whatever my computer normally uses</option>
        {preferredMissing ? (
          <option value={preferred}>Your chosen microphone (not connected)</option>
        ) : null}
        {devices?.devices
          .filter((device) => device.deviceId !== 'default')
          .map((device) => (
            <option key={device.deviceId} value={device.deviceId}>
              {device.label}
            </option>
          ))}
      </Select>
      <div className="recording-test">
        <Progress label="How loud you are" value={level} max={1} disabled={!testing} />
        <div className="recording-test__actions">
          <Button disabled={saving} onClick={() => void (testing ? stopTest() : startTest())}>
            {testState.status === 'starting'
              ? 'Cancel'
              : testing
                ? 'Stop test'
                : 'Test my microphone'}
          </Button>
          <Status tone={blocked || unavailable ? 'error' : testing ? 'success' : 'neutral'} live>
            {testState.status === 'starting'
              ? 'Asking for permission'
              : testState.status === 'active'
                ? 'Listening — say something'
                : blocked
                  ? 'Microphone blocked'
                  : unavailable
                    ? 'Microphone not available'
                    : 'Not testing'}
          </Status>
        </div>
      </div>
      {blocked ? (
        <div className="permission-guidance" role="alert">
          <p>
            Your computer is blocking microphone access. Allow Talking Quill in{' '}
            {platform === 'darwin'
              ? 'System Settings → Privacy & Security → Microphone.'
              : 'Settings → Privacy & security → Microphone.'}
          </p>
          <Button variant="secondary" onClick={() => void openMicrophoneSettings()}>
            Open microphone settings
          </Button>
        </div>
      ) : unavailableGuidance === null ? null : (
        <div className="permission-guidance" role="alert">
          <p>{unavailableGuidance}</p>
        </div>
      )}
      <div className="setting-divider" />
      <Select
        label="How long a pause ends a dictation"
        hint={`When you stop talking for this long, Talking Quill decides you are done. Quick dictation waits until it has heard at least ${formatSeconds(SPEECH_ARMING_MS)} of speech first.`}
        value={settings.recording.silencePreset}
        disabled={saving}
        onChange={(event) =>
          void savePreset(event.currentTarget.value as Settings['recording']['silencePreset'])
        }
      >
        <option value="aggressive">
          Short pause — {formatSeconds(SILENCE_PRESET_MS.aggressive)}
        </option>
        <option value="average">Normal pause — {formatSeconds(SILENCE_PRESET_MS.average)}</option>
        <option value="relaxed">Long pause — {formatSeconds(SILENCE_PRESET_MS.relaxed)}</option>
      </Select>
      {notice === null ? null : (
        <Toast tone={notice.tone} message={notice.message} onDismiss={() => setNotice(null)} />
      )}
    </Card>
  );
}

function formatSeconds(milliseconds: number): string {
  return `${(milliseconds / 1_000).toFixed(1)} seconds`;
}
