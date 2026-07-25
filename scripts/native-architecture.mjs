import { open, lstat, readdir } from 'node:fs/promises';
import { relative, resolve } from 'node:path';

const HEADER_READ_LIMIT = 64 * 1024;
const PE_MAGIC = 0x5a4d;
const PE_SIGNATURE = 0x00004550;
const MACH_32_MAGIC = 0xfeedface;
const MACH_64_MAGIC = 0xfeedfacf;
const FAT_MAGIC = 0xcafebabe;
const FAT_64_MAGIC = 0xcafebabf;
const CPU_X64 = 0x01000007;
const CPU_ARM64 = 0x0100000c;

export async function readNativeArchitectures(path) {
  const handle = await open(path, 'r');
  try {
    const metadata = await handle.stat();
    const bytes = Buffer.alloc(Math.min(metadata.size, HEADER_READ_LIMIT));
    await handle.read(bytes, 0, bytes.length, 0);
    return parseNativeArchitectures(bytes, path);
  } finally {
    await handle.close();
  }
}

/** Backward-compatible single-architecture probe for callers that require a thin image. */
export async function readNativeArchitecture(path, mac) {
  const parsed = await readNativeArchitectures(path);
  if (parsed === null || (mac && parsed.format === 'pe') || (!mac && parsed.format !== 'pe')) {
    throw new Error(`Expected a ${mac ? 'Mach-O' : 'PE'} executable: ${path}`);
  }
  if (parsed.architectures.length !== 1) {
    throw new Error(`Expected a single native executable architecture: ${path}`);
  }
  return parsed.architectures[0];
}

export function parseNativeArchitecture(bytes, path, mac) {
  const parsed = parseNativeArchitectures(bytes, path);
  if (parsed === null || (mac && parsed.format === 'pe') || (!mac && parsed.format !== 'pe')) {
    throw new Error(`Expected a ${mac ? 'Mach-O' : 'PE'} executable: ${path}`);
  }
  if (parsed.architectures.length !== 1) {
    throw new Error(`Expected a single native executable architecture: ${path}`);
  }
  return parsed.architectures[0];
}

/** Returns null for non-native data and a complete architecture set for PE/Mach-O images. */
export function parseNativeArchitectures(bytes, path) {
  if (bytes.length >= 2 && bytes.readUInt16LE(0) === PE_MAGIC) return parsePe(bytes, path);
  if (bytes.length < 4) return null;

  if (bytes.readUInt32LE(0) === MACH_32_MAGIC || bytes.readUInt32BE(0) === MACH_32_MAGIC) {
    throw new Error(`Unsupported 32-bit Mach-O executable: ${path}`);
  }
  if (bytes.readUInt32LE(0) === MACH_64_MAGIC) {
    return parseThinMach(bytes, path, true);
  }
  if (bytes.readUInt32BE(0) === MACH_64_MAGIC) {
    return parseThinMach(bytes, path, false);
  }
  if (bytes.readUInt32BE(0) === FAT_MAGIC) return parseFatMach(bytes, path, false, false);
  if (bytes.readUInt32LE(0) === FAT_MAGIC) return parseFatMach(bytes, path, true, false);
  if (bytes.readUInt32BE(0) === FAT_64_MAGIC) return parseFatMach(bytes, path, false, true);
  if (bytes.readUInt32LE(0) === FAT_64_MAGIC) return parseFatMach(bytes, path, true, true);
  return null;
}

/** Recursively verifies every regular file whose bytes identify it as PE or Mach-O. */
export async function inspectNativeTree(root, expectedArchitecture, exceptions = {}) {
  if (!['x64', 'arm64'].includes(expectedArchitecture)) {
    throw new Error(`Invalid expected native architecture: ${String(expectedArchitecture)}`);
  }
  const exceptionEntries = Object.entries(exceptions);
  for (const [path, architecture] of exceptionEntries) {
    if (
      path.length === 0 ||
      path.includes('\\') ||
      path.startsWith('/') ||
      path.split('/').includes('..') ||
      !['x86', 'x64', 'arm64'].includes(architecture)
    ) {
      throw new Error(`Invalid native architecture exception: ${path}`);
    }
  }
  const results = [];
  async function walk(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const absolute = resolve(directory, entry.name);
      const metadata = await lstat(absolute);
      if (metadata.isSymbolicLink()) continue;
      if (metadata.isDirectory()) {
        await walk(absolute);
        continue;
      }
      if (!metadata.isFile()) continue;
      const parsed = await readNativeArchitectures(absolute);
      if (parsed === null) continue;
      const name = relative(root, absolute).replaceAll('\\', '/');
      if (!parsed.architectures.includes(expectedArchitecture)) {
        const exceptionArchitecture = exceptions[name];
        if (
          exceptionArchitecture === undefined ||
          parsed.architectures.length !== 1 ||
          parsed.architectures[0] !== exceptionArchitecture
        ) {
          throw new Error(
            `Native architecture mismatch: ${name} is ${parsed.architectures.join('+')}, expected ${expectedArchitecture}`,
          );
        }
      }
      results.push({ path: name, format: parsed.format, architectures: parsed.architectures });
    }
  }
  await walk(resolve(root));
  return results.sort((left, right) => left.path.localeCompare(right.path));
}

function parsePe(bytes, path) {
  if (bytes.length < 64) throw new Error(`Truncated PE executable: ${path}`);
  const peOffset = bytes.readUInt32LE(0x3c);
  if (peOffset > HEADER_READ_LIMIT - 6 || peOffset + 6 > bytes.length) {
    throw new Error(`Invalid PE header offset: ${path}`);
  }
  if (bytes.readUInt32LE(peOffset) !== PE_SIGNATURE) throw new Error(`Invalid PE header: ${path}`);
  return { format: 'pe', architectures: [cpuArchitecture(bytes.readUInt16LE(peOffset + 4), path)] };
}

function parseThinMach(bytes, path, littleEndian) {
  if (bytes.length < 8) throw new Error(`Truncated 64-bit Mach-O executable: ${path}`);
  const cpu = littleEndian ? bytes.readUInt32LE(4) : bytes.readUInt32BE(4);
  return { format: 'mach-o-thin', architectures: [cpuArchitecture(cpu, path)] };
}

function parseFatMach(bytes, path, littleEndian, fat64) {
  if (bytes.length < 8) throw new Error(`Truncated fat Mach-O executable: ${path}`);
  const read32 = littleEndian
    ? (offset) => bytes.readUInt32LE(offset)
    : (offset) => bytes.readUInt32BE(offset);
  const count = read32(4);
  const entrySize = fat64 ? 32 : 20;
  if (count < 1 || count > 64 || 8 + count * entrySize > bytes.length) {
    throw new Error(`Invalid fat Mach-O architecture table: ${path}`);
  }
  const architectures = [];
  for (let index = 0; index < count; index += 1) {
    const architecture = cpuArchitecture(read32(8 + index * entrySize), path);
    if (!architectures.includes(architecture)) architectures.push(architecture);
  }
  return { format: 'mach-o-fat', architectures: architectures.sort() };
}

function cpuArchitecture(cpu, path) {
  if (cpu === 0x014c) return 'x86';
  if (cpu === CPU_X64 || cpu === 0x8664) return 'x64';
  if (cpu === CPU_ARM64 || cpu === 0xaa64) return 'arm64';
  throw new Error(`Unsupported native executable architecture 0x${cpu.toString(16)}: ${path}`);
}
