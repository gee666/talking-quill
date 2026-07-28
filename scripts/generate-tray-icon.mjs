// Generates the tray icon PNGs that are embedded as base64 constants in
// app/src/main/app/tray-icon.ts.
//
// Electron's nativeImage cannot decode SVG, so the tray artwork has to ship as
// real PNG bytes. No image library or browser is available here, therefore the
// artwork is authored as a pixel grid and encoded with Node built-ins only
// (zlib deflate + hand-rolled IHDR/IDAT/IEND chunks with CRC32).
//
// Usage: node scripts/generate-tray-icon.mjs
//        node scripts/generate-tray-icon.mjs --write-dir tmp

import { deflateSync } from 'node:zlib';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';

// '.' transparent, '+' navy tile, '#' navy tile shadow, 'o' cream quill.
const PALETTE = {
  '.': [0, 0, 0, 0],
  '+': [0x2a, 0x35, 0x74, 0xff],
  '#': [0x18, 0x21, 0x48, 0xff],
  o: [0xf6, 0xeb, 0xd6, 0xff],
};

// Colour artwork (Windows / Linux): a filled navy tile with a 1px inset and a
// cream quill stroke that tapers into a short nib.
const GRID_16 = [
  '................',
  '..++++++++++++..',
  '.++++++++++++++.',
  '.++++++++++oo++.',
  '.+++++++++ooo++.',
  '.++++++++ooo+++.',
  '.+++++++ooo++++.',
  '.++++++ooo+++++.',
  '.+++++ooo++++++.',
  '.+++++oo+++++++.',
  '.++++oo++++++++.',
  '.++++o+++++++++.',
  '.+++o++++++++++.',
  '.++++++++++++++.',
  '..++++++++++++..',
  '................',
];

const GRID_32 = [
  '................................',
  '................................',
  '.....++++++++++++++++++++++.....',
  '...++++++++++++++++++++++++++...',
  '..++++++++++++++++++++++++++++..',
  '..++++++++++++++++++++++++++++..',
  '..++++++++++++++++++++oooo++++..',
  '..+++++++++++++++++++ooooo++++..',
  '..++++++++++++++++++oooooo++++..',
  '..+++++++++++++++++oooooo+++++..',
  '..++++++++++++++++oooooo++++++..',
  '..+++++++++++++++oooooo+++++++..',
  '..++++++++++++++oooooo++++++++..',
  '..+++++++++++++oooooo+++++++++..',
  '..++++++++++++oooooo++++++++++..',
  '..+++++++++++oooooo+++++++++++..',
  '..++++++++++oooooo++++++++++++..',
  '..++++++++++oooo++++++++++++++..',
  '..+++++++++oooo+++++++++++++++..',
  '..+++++++++ooo++++++++++++++++..',
  '..++++++++ooo+++++++++++++++++..',
  '..++++++++ooo+++++++++++++++++..',
  '..+++++++ooo++++++++++++++++++..',
  '..+++++++oo+++++++++++++++++++..',
  '..++++++ooo+++++++++++++++++++..',
  '..++++++oo++++++++++++++++++++..',
  '..++++++o+++++++++++++++++++++..',
  '..++++++++++++++++++++++++++++..',
  '...++++++++++++++++++++++++++...',
  '.....++++++++++++++++++++++.....',
  '................................',
  '................................',
];

// Last-resort artwork: a plain opaque navy square. It has no transparency and no
// detail, so it is the safest possible thing to hand to Tray if anything else
// ever decodes to an empty image.
const GRID_FALLBACK_16 = Array.from({ length: 16 }, () => '+'.repeat(16));

// macOS last-resort artwork: every pixel is quill-coloured so the template
// encoder turns it into a fully opaque black mask (a transparent mask would
// decode to an empty image, i.e. an invisible tray icon).
const GRID_FALLBACK_TEMPLATE_16 = Array.from({ length: 16 }, () => 'o'.repeat(16));

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'latin1'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

/** Maps a pixel grid to RGBA rows, optionally as a black-on-alpha template mask. */
function toRgba(grid, { template }) {
  const size = grid.length;
  for (const [index, row] of grid.entries()) {
    if (row.length !== size) throw new Error(`Row ${String(index)} is not ${String(size)} wide`);
  }
  // One filter byte (0 = None) plus 4 bytes per pixel per scanline.
  const raw = Buffer.alloc(size * (1 + size * 4));
  let offset = 0;
  for (const row of grid) {
    raw[offset] = 0;
    offset += 1;
    for (const character of row) {
      const colour = PALETTE[character];
      if (!colour) throw new Error(`Unknown pixel '${character}'`);
      const [r, g, b, a] = template ? [0, 0, 0, character === 'o' ? 0xff : 0] : colour;
      raw[offset] = r;
      raw[offset + 1] = g;
      raw[offset + 2] = b;
      raw[offset + 3] = a;
      offset += 4;
    }
  }
  return { raw, size };
}

function encodePng(grid, options = { template: false }) {
  const { raw, size } = toRgba(grid, options);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // colour type: RGBA
  ihdr[10] = 0; // deflate
  ihdr[11] = 0; // adaptive filtering
  ihdr[12] = 0; // no interlace
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/** Decodes the container back so a corrupt buffer can never be pasted into source. */
function verifyPng(buffer) {
  const magic = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
  if (!buffer.subarray(0, 8).equals(magic)) throw new Error('Missing PNG magic');
  let offset = 8;
  const types = [];
  let header;
  while (offset < buffer.length) {
    const length = buffer.readUInt32BE(offset);
    const type = buffer.toString('latin1', offset + 4, offset + 8);
    const body = buffer.subarray(offset + 4, offset + 8 + length);
    const expected = buffer.readUInt32BE(offset + 8 + length);
    if (crc32(body) !== expected) throw new Error(`Bad CRC in ${type} chunk`);
    if (type === 'IHDR') {
      header = {
        width: buffer.readUInt32BE(offset + 8),
        height: buffer.readUInt32BE(offset + 12),
        depth: buffer[offset + 16],
        colourType: buffer[offset + 17],
      };
    }
    types.push(type);
    offset += 12 + length;
  }
  if (offset !== buffer.length) throw new Error('Trailing bytes after IEND');
  if (!header) throw new Error('No IHDR chunk');
  if (types.at(0) !== 'IHDR' || types.at(-1) !== 'IEND') throw new Error('Chunk order invalid');
  return header;
}

const outputs = [
  { name: 'TRAY_ICON_16_PNG_BASE64', grid: GRID_16, template: false, file: 'tray-16.png' },
  { name: 'TRAY_ICON_32_PNG_BASE64', grid: GRID_32, template: false, file: 'tray-32.png' },
  {
    name: 'TRAY_TEMPLATE_16_PNG_BASE64',
    grid: GRID_16,
    template: true,
    file: 'tray-16-template.png',
  },
  {
    name: 'TRAY_TEMPLATE_32_PNG_BASE64',
    grid: GRID_32,
    template: true,
    file: 'tray-32-template.png',
  },
  {
    name: 'TRAY_FALLBACK_16_PNG_BASE64',
    grid: GRID_FALLBACK_16,
    template: false,
    file: 'tray-16-fallback.png',
  },
  {
    name: 'TRAY_FALLBACK_32_PNG_BASE64',
    grid: Array.from({ length: 32 }, () => '+'.repeat(32)),
    template: false,
    file: 'tray-32-fallback.png',
  },
  {
    name: 'TRAY_FALLBACK_TEMPLATE_16_PNG_BASE64',
    grid: GRID_FALLBACK_TEMPLATE_16,
    template: true,
    file: 'tray-16-fallback-template.png',
  },
  {
    name: 'TRAY_FALLBACK_TEMPLATE_32_PNG_BASE64',
    grid: Array.from({ length: 32 }, () => 'o'.repeat(32)),
    template: true,
    file: 'tray-32-fallback-template.png',
  },
];

const writeDirIndex = process.argv.indexOf('--write-dir');
const writeDir = writeDirIndex === -1 ? null : process.argv[writeDirIndex + 1];
if (writeDir) await mkdir(resolve(writeDir), { recursive: true });

for (const { name, grid, template, file } of outputs) {
  const png = encodePng(grid, { template });
  const { width, height, depth, colourType } = verifyPng(png);
  console.log(
    `${name}: valid PNG ${String(width)}x${String(height)} depth=${String(depth)} colourType=${String(colourType)} bytes=${String(png.length)} base64=${String(png.toString('base64').length)}`,
  );
  console.log(`const ${name} =\n  '${png.toString('base64')}';`);
  if (writeDir) await writeFile(resolve(writeDir, file), png);
}
