type SettingsRecord = Record<string, unknown>;

function isRecord(value: unknown): value is SettingsRecord {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function cloneAndStrip(input: unknown, strip: (record: SettingsRecord) => void): unknown {
  if (!isRecord(input)) return input;
  const clone = structuredClone(input);
  strip(clone);
  return clone;
}

function removeFields(value: unknown, fields: readonly string[]): void {
  if (!isRecord(value)) return;
  for (const field of fields) Reflect.deleteProperty(value, field);
}

export function stripRecordingOptions(input: unknown): unknown {
  return cloneAndStrip(input, (record) => {
    removeFields(record.recording, ['autoSubmitOnSilence', 'includeSystemAudio']);
  });
}

export function stripDictationProfiles(input: unknown): unknown {
  return cloneAndStrip(stripRecordingOptions(input), (record) => {
    delete record.dictationProfiles;
  });
}

export function stripTask12Fields(input: unknown): unknown {
  return cloneAndStrip(stripDiagnosticLoggingField(stripDictationProfiles(input)), (record) => {
    delete record.welcome;
    removeFields(record.app, ['launchAtLogin']);
  });
}

export function stripPiInstallationPath(input: unknown): unknown {
  return cloneAndStrip(input, (record) => {
    removeFields(record.smartProcessing, ['piInstallationPath', 'piExtensionsEnabled']);
  });
}

export function stripDiagnosticLoggingField(input: unknown): unknown {
  return cloneAndStrip(stripPiInstallationPath(stripDictationProfiles(input)), (record) => {
    removeFields(record.privacy, ['diagnosticLoggingEnabled']);
  });
}
