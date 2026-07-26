import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  macTcpdumpArguments,
  parseCaptureInvocation,
  parseTcpdumpInterfaces,
  selectMacCaptureInterface,
  validatePcapBytes,
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

  it('parses wrapper options only before the command separator and contains output under tmp', () => {
    const root = resolve('.');
    expect(
      parseCaptureInvocation(
        ['--output=tmp/security/review.pcapng', '--', 'captured-tool', '--output=outside.pcap'],
        root,
      ),
    ).toMatchObject({
      command: 'captured-tool',
      commandArgs: ['--output=outside.pcap'],
      output: resolve(root, 'tmp/security/review.pcapng'),
      requestedInterface: null,
      scope: 'all',
    });
    expect(() => parseCaptureInvocation(['--output=../outside.pcap', '--', 'tool'], root)).toThrow(
      'under tmp',
    );
    expect(() => parseCaptureInvocation(['--output=tmp/capture.etl', '--', 'tool'], root)).toThrow(
      '.pcap or .pcapng',
    );
    expect(() =>
      parseCaptureInvocation(['--scope=all', '--scope=loopback', '--', 'tool'], root),
    ).toThrow('Duplicate capture option');
    expect(() => parseCaptureInvocation(['--unexpected', '--', 'tool'], root)).toThrow(
      'Unknown capture option',
    );
    if (process.platform === 'win32') {
      expect(() =>
        parseCaptureInvocation(['--output=C:/outside.pcap', '--', 'tool'], root),
      ).toThrow('under tmp');
    }
  });

  it('validates complete pcapng section headers instead of magic bytes alone', () => {
    const pcap = Buffer.alloc(24);
    pcap.writeUInt32BE(0xa1b2c3d4, 0);
    expect(() => validatePcapBytes(pcap)).not.toThrow();

    const pcapng = Buffer.alloc(28, 0xff);
    pcapng.writeUInt32BE(0x0a0d0d0a, 0);
    pcapng.writeUInt32LE(28, 4);
    pcapng.writeUInt32LE(0x1a2b3c4d, 8);
    pcapng.writeUInt16LE(1, 12);
    pcapng.writeUInt16LE(0, 14);
    pcapng.writeUInt32LE(28, 24);
    expect(() => validatePcapBytes(pcapng)).not.toThrow();
    pcapng.writeUInt32LE(24, 24);
    expect(() => validatePcapBytes(pcapng)).toThrow('inconsistent pcapng block lengths');
    pcapng.writeUInt32LE(28, 24);
    pcapng.writeUInt16LE(2, 12);
    expect(() => validatePcapBytes(pcapng)).toThrow('unsupported pcapng version 2.0');
    expect(() => validatePcapBytes(Buffer.alloc(24))).toThrow('not pcap/pcapng');
  });

  it('constructs macOS capture commands without advertising unsupported Linux capture', () => {
    expect(macTcpdumpArguments('capture.pcap', ['pktap', 'lo0'], 'pktap', 'all')).toEqual([
      '-i',
      'pktap',
      '-U',
      '-w',
      'capture.pcap',
    ]);
  });
});
