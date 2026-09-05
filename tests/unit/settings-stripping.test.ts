import { describe, expect, it } from 'vitest';
import {
  stripDiagnosticLoggingField,
  stripDictationProfiles,
  stripPiInstallationPath,
  stripRecordingOptions,
  stripTask12Fields,
} from '../../app/src/main/persistence/settings-migrations/transforms';

const strippers = [
  stripRecordingOptions,
  stripDictationProfiles,
  stripTask12Fields,
  stripPiInstallationPath,
  stripDiagnosticLoggingField,
];

const source = {
  app: { launchAtLogin: true, enabled: false },
  recording: { autoSubmitOnSilence: true, includeSystemAudio: true, silencePreset: 'average' },
  smartProcessing: { piInstallationPath: '/pi', piExtensionsEnabled: true, providers: {} },
  privacy: { diagnosticLoggingEnabled: true, historyEnabled: false },
  dictationProfiles: [{ id: 'general' }],
  welcome: { completedAt: 1 },
  unknownField: { retained: true },
};

describe('legacy settings field stripping', () => {
  it.each(strippers)('leaves non-record inputs untouched in %s', (strip) => {
    for (const value of [null, undefined, false, 27, 'settings', [], [{ app: {} }]]) {
      expect(strip(value)).toBe(value);
    }
  });

  it.each(strippers)('clones without mutating its source in %s', (strip) => {
    const input = structuredClone(source);
    const result = strip(input);
    expect(input).toEqual(source);
    expect(result).not.toBe(input);
    expect(result).toMatchObject({ unknownField: { retained: true } });
  });

  it.each(strippers)('does not reinterpret malformed nested records in %s', (strip) => {
    for (const value of [null, false, 'invalid', ['retained']]) {
      const input = { app: value, recording: value, privacy: value, smartProcessing: value };
      expect(strip(input)).toEqual(input);
    }
  });

  it('preserves the historical composition of each stripping stage', () => {
    const recording = { silencePreset: 'average' };
    expect(stripRecordingOptions(source)).toEqual({ ...source, recording });
    const { dictationProfiles: profiles, ...withoutProfiles } = source;
    expect(profiles).toEqual([{ id: 'general' }]);
    expect(stripDictationProfiles(source)).toEqual({ ...withoutProfiles, recording });
    const smartProcessing = { providers: {} };
    expect(stripPiInstallationPath(source)).toEqual({ ...source, smartProcessing });
    const diagnostic = {
      ...withoutProfiles,
      recording,
      smartProcessing,
      privacy: { historyEnabled: false },
    };
    expect(stripDiagnosticLoggingField(source)).toEqual(diagnostic);
    const { welcome, ...withoutWelcome } = diagnostic;
    expect(welcome).toEqual({ completedAt: 1 });
    expect(stripTask12Fields(source)).toEqual({ ...withoutWelcome, app: { enabled: false } });
  });
});
