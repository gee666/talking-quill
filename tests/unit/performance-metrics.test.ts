import { describe, expect, it } from 'vitest';
import {
  classifyWindowsProcess,
  describeBytes,
  range,
  summarizeProcesses,
} from '../../scripts/performance-metrics.mjs';

describe('packaged performance metrics', () => {
  it('classifies Electron and native child roles without treating renderers as known windows', () => {
    expect(classifyWindowsProcess('Talking Quill.exe', 'app')).toBe('electron-main');
    expect(classifyWindowsProcess('Talking Quill.exe', '--type=renderer')).toBe('renderer');
    expect(classifyWindowsProcess('Talking Quill.exe', '--type=gpu-process')).toBe('gpu');
    expect(
      classifyWindowsProcess(
        'Talking Quill.exe',
        '--type=utility --utility-sub-type=network.mojom.NetworkService',
      ),
    ).toBe('network-service');
    expect(classifyWindowsProcess('talking-quill-helper.exe')).toBe('native-helper');
    expect(classifyWindowsProcess('conhost.exe')).toBe('helper-console-host');
  });

  it('sums private working set independently from gross working set and private commit', () => {
    const summary = summarizeProcesses([
      {
        role: 'renderer',
        privateWorkingSetBytes: 10,
        grossWorkingSetBytes: 30,
        privateBytes: 20,
      },
      {
        role: 'renderer',
        privateWorkingSetBytes: 11,
        grossWorkingSetBytes: 31,
        privateBytes: 21,
      },
      {
        role: 'gpu',
        privateWorkingSetBytes: 12,
        grossWorkingSetBytes: 32,
        privateBytes: 22,
      },
    ]);

    expect(summary).toMatchObject({
      processCount: 3,
      privateWorkingSetBytes: 33,
      grossWorkingSetBytes: 93,
      privateBytes: 63,
      roles: {
        gpu: { count: 1, privateWorkingSetBytes: 12 },
        renderer: { count: 2, privateWorkingSetBytes: 21 },
      },
    });
  });

  it('reports deterministic min, median, max and MiB values', () => {
    expect(range([9, 2, 5])).toEqual({ min: 2, median: 5, max: 9 });
    expect(describeBytes(1.25 * 1024 * 1024)).toBe(1.3);
    expect(() => range([])).toThrow('empty measurement set');
  });
});
