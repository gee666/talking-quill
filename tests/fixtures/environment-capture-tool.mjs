import { appendFileSync } from 'node:fs';

const [capturePath, mode, ...arguments_] = process.argv.slice(2);
if (capturePath === undefined || mode === undefined) process.exit(64);
appendFileSync(
  capturePath,
  `${JSON.stringify({ mode, arguments: arguments_, environment: process.env })}\n`,
);

if (mode === 'git') {
  if (arguments_[0] === 'rev-parse' && arguments_[1] === 'HEAD^{commit}') {
    process.stdout.write(`${'a'.repeat(40)}\n`);
  } else if (arguments_[0] === 'rev-parse' && arguments_[1] === 'HEAD^{tree}') {
    process.stdout.write(`${'b'.repeat(40)}\n`);
  }
} else if (mode === 'sysctl') {
  process.stdout.write('1\n');
} else if (mode === 'uname') {
  process.stdout.write('arm64\n');
} else {
  process.exit(64);
}
