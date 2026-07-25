const { app, nativeImage } = require('electron');
const { readFileSync, readdirSync } = require('node:fs');
const { resolve } = require('node:path');

app.whenReady().then(() => {
  const directory = resolve(process.argv[2]);
  const failures = [];
  const names = readdirSync(directory).sort();
  for (const name of names) {
    const path = resolve(directory, name);
    const bytes = readFileSync(path);
    const png = bytes.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'));
    const jpeg =
      bytes.length >= 4 &&
      bytes[0] === 0xff &&
      bytes[1] === 0xd8 &&
      bytes.at(-2) === 0xff &&
      bytes.at(-1) === 0xd9;
    const extensionMatches = (name.endsWith('.png') && png) || (name.endsWith('.jpeg') && jpeg);
    const image = nativeImage.createFromPath(path);
    const size = image.getSize();
    if (!extensionMatches || image.isEmpty() || size.width < 1 || size.height < 1) {
      failures.push({ name, extensionMatches, empty: image.isEmpty(), size });
    }
  }
  process.stdout.write(JSON.stringify({ count: names.length, failures }));
  app.exit(failures.length === 0 ? 0 : 1);
});
