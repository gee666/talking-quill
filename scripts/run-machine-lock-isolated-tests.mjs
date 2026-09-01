import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';

const separator = process.argv.indexOf('--');
if (separator === -1 || separator === process.argv.length - 1) {
  throw new Error('Usage: run-machine-lock-isolated-tests.mjs -- <command>');
}
const command = process.argv.slice(separator + 1).join(' ');

if (process.platform !== 'win32') {
  const result = spawnSync(command, { shell: true, stdio: 'inherit' });
  process.exit(result.status ?? 1);
}

const owner = spawn(
  'powershell.exe',
  [
    '-NoProfile',
    '-NonInteractive',
    '-Command',
    "$m=[Threading.Mutex]::new($false,'Global\\TalkingQuill.MachineLockTests.V1');try{if(-not $m.WaitOne(300000)){exit 2};[Console]::Out.WriteLine('ready');[Console]::Out.Flush();[Console]::In.ReadLine()|Out-Null}finally{try{$m.ReleaseMutex()}catch{};$m.Dispose()}",
  ],
  { stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true },
);

let output = '';
for await (const chunk of owner.stdout) {
  output += chunk;
  if (output.includes('ready')) break;
}
if (!output.includes('ready')) throw new Error('machine-lock test serializer did not start');

let status = 1;
const testNamespaceId = randomBytes(16).toString('hex');
process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID = testNamespaceId;
try {
  assertZeroProductionResidue('before');
  const result = spawnSync(command, { shell: true, stdio: 'inherit', windowsHide: true });
  status = result.status ?? 1;
} finally {
  try {
    removeTestNamespace(testNamespaceId);
    assertZeroProductionResidue('after');
  } finally {
    owner.stdin.end('\n');
  }
}
process.exit(status);

function removeTestNamespace(id) {
  if (!/^[0-9a-f]{32}$/u.test(id)) throw new Error('invalid machine-lock test namespace');
  for (const path of [
    resolve('tmp', 'machine-lock-tests', 'helper', id),
    resolve('tmp', 'machine-lock-tests', 'windows-setup', id),
  ]) {
    if (!existsSync(path)) continue;
    const owner = spawnSync('takeown.exe', ['/f', path, '/r', '/d', 'y'], {
      encoding: 'utf8',
      windowsHide: true,
    });
    const account = `${process.env.USERDOMAIN}\\${process.env.USERNAME}`;
    const access = spawnSync('icacls.exe', [path, '/grant', `${account}:(OI)(CI)F`, '/t', '/c'], {
      encoding: 'utf8',
      windowsHide: true,
    });
    if (owner.status !== 0 || access.status !== 0) {
      throw new Error(`cannot take ownership of machine-lock test namespace ${path}`);
    }
    rmSync(path, { recursive: true, force: true });
  }
  const result = spawnSync(
    'reg.exe',
    ['delete', `HKCU\\Software\\Talking Quill Tests\\${id}`, '/f'],
    { encoding: 'utf8', windowsHide: true },
  );
  if (![0, 1].includes(result.status ?? -1)) {
    throw new Error('cannot remove machine-lock test registry namespace');
  }
}

function assertZeroProductionResidue(phase) {
  const script = String.raw`
    $pd=[Environment]::GetFolderPath('CommonApplicationData')
    $paths=@(Get-ChildItem -LiteralPath $pd -Force -ErrorAction Stop | Where-Object {
      $_.Name -like '.Talking Quill.machine-lock-*' -or
      $_.Name -like '.Talking Quill.machine-lock-pending-*' -or
      $_.Name -like '.Talking Quill.machine-lifecycle-retained-*'
    } | ForEach-Object FullName)
    $key=Get-Item -LiteralPath 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill\RecoveryStateLockV1' -ErrorAction SilentlyContinue
    [pscustomobject]@{Paths=$paths;Registry=($null-ne$key)}|ConvertTo-Json -Compress
  `;
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', script],
    {
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (result.status !== 0)
    throw new Error(`cannot inspect production machine-lock residue ${phase}`);
  const residue = JSON.parse(result.stdout.trim());
  if (residue.Registry || residue.Paths.length !== 0) {
    throw new Error(
      `production machine-lock residue ${phase} isolated tests: ${JSON.stringify(residue)}`,
    );
  }
}
