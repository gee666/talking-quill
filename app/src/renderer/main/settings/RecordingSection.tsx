import { useEffect, useRef, useState } from 'react';
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
}: {
  readonly settings: Settings;
  readonly platform: string;
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
  const testing = testState.status === 'starting' || testState.status === 'active';
  const isCurrentOperation = (value: number) => mounted.current && operation.current === value;

  useEffect(() => {
    mounted.current = true;
    const applyDevices = (next: MicrophoneDeviceList) => {
      if (!mounted.current) return;
      setDevices(next);
      const permission = next.permission;
      if (permission === 'denied' || permission === 'restricted') {
        setTestState((current) =>
          current.status === 'active' || current.status === 'starting'
            ? current
            : {
                status: 'blocked',
                permission,
                reason: 'microphone-permission',
              },
        );
      }
    };
    const removeDevices = window.talkingQuill.recording.onDevicesChanged(applyDevices);
    const removeLevel = window.talkingQuill.recording.onTestLevel((next) => {
      if (mounted.current) setLevel(next.rms);
    });
    const removeState = window.talkingQuill.recording.onTestStateChanged((next) => {
      if (!mounted.current) return;
      setTestState(next);
      if (next.status !== 'active') setLevel(0);
    });
    void window.talkingQuill.recording.getDevices().then(
      (next) => applyDevices(next),
      () => {
        if (mounted.current) {
          setNotice({ tone: 'error', message: 'Microphones could not be listed.' });
        }
      },
    );
    return () => {
      mounted.current = false;
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
      if (mounted.current) setTestState(next);
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
        setTestState(stopped);
        setLevel(0);
      }
      await window.talkingQuill.settings.update({
        recording: { preferredMicrophoneId: value.length === 0 ? null : value },
      });
      if (!isCurrentOperation(currentOperation)) return;
      setNotice({ tone: 'success', message: 'Preferred microphone saved.' });
      if (restart) {
        const restarted = await window.talkingQuill.recording.startTest();
        if (isCurrentOperation(currentOperation)) {
          setTestState(restarted);
        } else {
          await window.talkingQuill.recording.stopTest();
        }
      }
    } catch {
      if (isCurrentOperation(currentOperation)) {
        setNotice({ tone: 'error', message: 'The preferred microphone could not be saved.' });
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
        setNotice({ tone: 'error', message: 'Microphone settings could not be opened.' });
      }
    }
  };

  const savePreset = async (value: Settings['recording']['silencePreset']) => {
    setSaving(true);
    setNotice(null);
    try {
      await window.talkingQuill.settings.update({ recording: { silencePreset: value } });
      setNotice({ tone: 'success', message: 'Silence detection preset saved.' });
    } catch {
      setNotice({ tone: 'error', message: 'The silence detection preset could not be saved.' });
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
        ? 'No microphone was detected. Connect or enable an input device, then test again.'
        : testState.reason === 'device-unavailable'
          ? 'The selected microphone is disconnected, busy, or does not support the requested format.'
          : testState.reason === 'permission-unavailable'
            ? 'Talking Quill could not complete Electron’s microphone authorization. Restart Talking Quill and test again; do not change Windows privacy settings unless Windows reports access is blocked.'
            : testState.reason === 'unsupported-audio-format'
              ? 'This microphone does not provide a supported audio format.'
              : 'The dedicated microphone capture window is unavailable. Restart Talking Quill and test again.';

  return (
    <Card title="Recording" description="Microphone input stays on this device.">
      <Select
        label="Microphone"
        hint="A disconnected preference is retained and used again when it returns."
        value={preferred ?? ''}
        disabled={saving || devices === null}
        onChange={(event) => void saveMicrophone(event.currentTarget.value)}
      >
        <option value="">System default</option>
        {preferredMissing ? (
          <option value={preferred}>Preferred microphone (disconnected)</option>
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
        <Progress label="Microphone level" value={level} max={1} disabled={!testing} />
        <div className="recording-test__actions">
          <Button disabled={saving} onClick={() => void (testing ? stopTest() : startTest())}>
            {testState.status === 'starting'
              ? 'Cancel microphone test'
              : testing
                ? 'Stop microphone test'
                : 'Test microphone'}
          </Button>
          <Status tone={blocked || unavailable ? 'error' : testing ? 'success' : 'neutral'} live>
            {testState.status === 'starting'
              ? 'Requesting microphone access'
              : testState.status === 'active'
                ? 'Microphone active'
                : blocked
                  ? 'Microphone access denied'
                  : unavailable
                    ? 'Microphone unavailable'
                    : 'Test stopped'}
          </Status>
        </div>
      </div>
      {blocked ? (
        <div className="permission-guidance" role="alert">
          <p>
            The operating system reports that microphone access is blocked. Allow Talking Quill in{' '}
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
        label="Silence detection"
        hint="Quick Dictation can submit only after at least 300 ms of detected speech."
        value={settings.recording.silencePreset}
        disabled={saving}
        onChange={(event) =>
          void savePreset(event.currentTarget.value as Settings['recording']['silencePreset'])
        }
      >
        <option value="aggressive">Aggressive — 1.0 seconds</option>
        <option value="average">Average — 1.8 seconds</option>
        <option value="relaxed">Relaxed — 3.0 seconds</option>
      </Select>
      {notice === null ? null : (
        <Toast tone={notice.tone} message={notice.message} onDismiss={() => setNotice(null)} />
      )}
    </Card>
  );
}
