const { spawn } = require('node:child_process');
const { writeFileSync } = require('node:fs');

const receipt = process.env.TALKING_QUILL_PI_TREE_RECEIPT;
if (!receipt) process.exit(2);
const descendant = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
  detached: false,
  stdio: 'ignore',
});
writeFileSync(receipt, JSON.stringify({ root: process.pid, descendant: descendant.pid }));
setInterval(() => undefined, 1000);
