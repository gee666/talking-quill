import { describe, expect, it } from 'vitest';
import {
  parseTcpdumpInterfaces,
  selectMacCaptureInterface,
  tcpdumpArguments,
} from '../../scripts/os-packet-capture.mjs';

describe('OS packet capture policy', () => {
  it('enumerates and validates macOS pktap/loopback coverage', () => {
    const interfaces = parseTcpdumpInterfaces('1.en0 [Up]\n2.lo0 [Up, Loopback]\n3.pktap [none]\n');
    expect(interfaces).toEqual(['en0', 'lo0', 'pktap']);
    expect(selectMacCaptureInterface(interfaces)).toBe('pktap');
    expect(selectMacCaptureInterface(interfaces, 'lo0', 'loopback')).toBe('lo0');
    expect(() => selectMacCaptureInterface(interfaces, 'lo0', 'all')).toThrow('cannot prove');
    expect(() => selectMacCaptureInterface(['en0', 'lo0'])).toThrow('refusing incomplete');
    expect(() => selectMacCaptureInterface(interfaces, 'en0')).toThrow('pktap or lo0');
  });

  it('constructs platform-specific commands without the unsupported macOS any interface', () => {
    expect(tcpdumpArguments('darwin', 'capture.pcap', ['pktap', 'lo0'], 'pktap', 'all')).toEqual([
      '-i',
      'pktap',
      '-U',
      '-w',
      'capture.pcap',
    ]);
    expect(tcpdumpArguments('linux', 'capture.pcap', [], 'pktap', 'all')).toContain('any');
  });
});
