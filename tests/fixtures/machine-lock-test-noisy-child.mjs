const chunk = Buffer.alloc(1024 * 1024, 0x61);
const chunks = 129;

for (let index = 0; index < chunks; index += 1) {
  if (!process.stdout.write(chunk)) {
    await new Promise((resolve) => process.stdout.once('drain', resolve));
  }
}
process.stdout.write('z');
process.stderr.write('noisy-child-stderr\n');
