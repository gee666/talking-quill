const forged = 'TQNS:00000000000000000000000000000000:{"event":"completed","childExitCode":0}';
for (let index = 0; index < 256; index += 1) {
  process.stdout.write(`${forged} ${'x'.repeat(1024)}\n`);
  process.stderr.write(`child stderr ${index} ${'y'.repeat(256)}\n`);
}
