export const MIB = 1024 * 1024;

export function classifyWindowsProcess(name, commandLine = '') {
  const executable = name.toLowerCase();
  if (executable === 'talking-quill-helper.exe') return 'native-helper';
  if (executable === 'conhost.exe') return 'helper-console-host';
  if (commandLine.includes('--type=gpu-process')) return 'gpu';
  if (commandLine.includes('--type=renderer')) return 'renderer';
  if (commandLine.includes('network.mojom.NetworkService')) return 'network-service';
  if (commandLine.includes('audio.mojom.AudioService')) return 'audio-service';
  if (commandLine.includes('video_capture.mojom.VideoCaptureService')) {
    return 'video-capture-service';
  }
  if (commandLine.includes('--type=utility')) return 'utility-other';
  return 'electron-main';
}

export function summarizeProcesses(processes) {
  const totals = processes.reduce(
    (result, process) => ({
      privateWorkingSetBytes: result.privateWorkingSetBytes + process.privateWorkingSetBytes,
      grossWorkingSetBytes: result.grossWorkingSetBytes + process.grossWorkingSetBytes,
      privateBytes: result.privateBytes + process.privateBytes,
    }),
    { privateWorkingSetBytes: 0, grossWorkingSetBytes: 0, privateBytes: 0 },
  );
  const roles = new Map();
  for (const process of processes) {
    const current = roles.get(process.role) ?? {
      count: 0,
      privateWorkingSetBytes: 0,
      grossWorkingSetBytes: 0,
      privateBytes: 0,
    };
    current.count += 1;
    current.privateWorkingSetBytes += process.privateWorkingSetBytes;
    current.grossWorkingSetBytes += process.grossWorkingSetBytes;
    current.privateBytes += process.privateBytes;
    roles.set(process.role, current);
  }
  return {
    processCount: processes.length,
    ...totals,
    roles: Object.fromEntries([...roles].sort(([left], [right]) => left.localeCompare(right))),
  };
}

export function describeBytes(bytes) {
  return Number((bytes / MIB).toFixed(1));
}

export function range(values) {
  if (values.length === 0) throw new Error('Cannot summarize an empty measurement set');
  const sorted = [...values].sort((left, right) => left - right);
  return {
    min: sorted[0],
    median: sorted[Math.floor(sorted.length / 2)],
    max: sorted.at(-1),
  };
}
