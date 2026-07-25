import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { dirname, resolve } from 'node:path';

if (process.platform !== 'win32') throw new Error('NSIS lifecycle evidence requires Windows');
const preservationOnly = process.argv.includes('--preservation-only');
const [oldInput, currentInput, rootInput = 'tmp/nsis-lifecycle'] = process.argv
  .slice(2)
  .filter((value) => value !== '--' && value !== '--preservation-only');
if (oldInput === undefined || currentInput === undefined)
  throw new Error('Expected old and current installer paths');
const oldInstaller = resolve(oldInput);
const currentInstaller = resolve(currentInput);
const root = resolve(rootInput);
const install = resolve(root, 'install');
const profile = resolve(root, 'profile');
const appData = resolve(profile, 'AppData/Roaming');
const localAppData = resolve(profile, 'AppData/Local');
const data = resolve(appData, 'Talking Quill');
const external = resolve(root, 'external-owned-by-other-app.marker');
const environment = {
  ...process.env,
  APPDATA: appData,
  LOCALAPPDATA: localAppData,
  USERPROFILE: profile,
  HOME: profile,
  TALKING_QUILL_NSIS_EVIDENCE_ROOT: root,
};
rmSync(root, { recursive: true, force: true });
mkdirSync(appData, { recursive: true });
mkdirSync(localAppData, { recursive: true });

installWith(oldInstaller);
writeFileSync(
  resolve(data, '.talking-quill-owner.json'),
  `${JSON.stringify(
    {
      schemaVersion: 1,
      appId: 'com.talkingquill.app',
      rootIdentity: createHash('sha256')
        .update(`com.talkingquill.app\0${realCanonical(data)}`)
        .digest('hex'),
    },
    null,
    2,
  )}\n`,
);
const markers = {
  'settings.json': '{"schemaVersion":1,"marker":"upgrade-settings"}',
  'history.marker': 'upgrade-history',
  'models/marker-model/model.marker': 'upgrade-model',
};
for (const [name, value] of Object.entries(markers)) {
  const path = resolve(data, name);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, value);
}
const before = Object.fromEntries(
  Object.keys(markers).map((name) => [name, hash(resolve(data, name))]),
);
installWith(currentInstaller);
assertMarkers(before, 'upgrade');
uninstall(['/S']);
waitForRemoval(install);
assertMarkers(before, 'default uninstall');

if (!preservationOnly) {
  installWith(currentInstaller);
  writeFileSync(resolve(data, 'destructive.marker'), 'delete-owned-data');
  writeFileSync(external, 'preserve-external-data');
  invokeResetHelper();
  waitForRemoval(data);
  uninstall(['/S']);
  waitForRemoval(install);
  if (!existsSync(external)) throw new Error('Destructive uninstall removed external test data');
}

const evidence = {
  schemaVersion: 1,
  historicalCommit: '0e82f3ce30906851679924a29a736eae386fb225',
  oldInstaller: identity(oldInstaller),
  currentInstaller: identity(currentInstaller),
  isolatedRoot: root,
  markers: before,
  upgradePreserved: true,
  defaultUninstallPreserved: true,
  destructiveUninstallRemovedOwnedData: preservationOnly ? 'not-run' : true,
  destructiveUninstallPreservedExternalData: preservationOnly ? 'not-run' : true,
};
writeFileSync(resolve(root, 'evidence.json'), `${JSON.stringify(evidence, null, 2)}\n`);
console.log(JSON.stringify(evidence, null, 2));

function installWith(installer) {
  run(installer, ['/S', `/D=${install}`]);
}
function invokeResetHelper() {
  const challengeDirectory = resolve(root, 'reset-challenge');
  rmSync(challengeDirectory, { recursive: true, force: true });
  mkdirSync(challengeDirectory, { recursive: true });
  const challenge = resolve(challengeDirectory, 'talking-quill-reset.challenge');
  writeFileSync(challenge, challengeDirectory);
  run(
    resolve(install, 'Talking Quill.exe'),
    [
      `--talking-quill-reset-owned-data-and-exit=${challenge}`,
      `--talking-quill-nsis-evidence-root=${root}`,
    ],
    { TALKING_QUILL_UNINSTALL_RESET_CHALLENGE: challengeDirectory },
  );
  rmSync(challengeDirectory, { recursive: true, force: true });
}
function uninstall(args) {
  const name = readdirSync(install).find((entry) => /^Uninstall.*\.exe$/iu.test(entry));
  if (name === undefined) throw new Error('Installed uninstaller is missing');
  run(resolve(install, name), args);
}
function run(executable, args, extraEnvironment = {}) {
  const result = spawnSync(executable, args, {
    env: { ...environment, ...extraEnvironment },
    stdio: 'inherit',
    timeout: 180_000,
  });
  if (result.status !== 0)
    throw new Error(
      `${executable} failed with ${String(result.status)}: ${String(result.error ?? '')}`,
    );
}
function waitForRemoval(path) {
  const deadline = Date.now() + 30_000;
  while (existsSync(path) && Date.now() < deadline)
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 100);
  if (existsSync(path)) throw new Error(`Timed out waiting for removal: ${path}`);
}
function assertMarkers(expected, stage) {
  for (const [name, digest] of Object.entries(expected))
    if (hash(resolve(data, name)) !== digest) throw new Error(`${stage} changed ${name}`);
}
function realCanonical(path) {
  mkdirSync(path, { recursive: true });
  return realpathSync.native(path);
}
function hash(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}
function identity(path) {
  const bytes = readFileSync(path);
  return { size: statSync(path).size, sha256: createHash('sha256').update(bytes).digest('hex') };
}
