import { spawnSync } from 'node:child_process';
import { lstatSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';

const [appInput, dmgInput, zipInput, teamId, architecture, version] = process.argv
  .slice(2)
  .filter((value) => value !== '--');
if (![appInput, dmgInput, zipInput, teamId, architecture, version].every(Boolean))
  throw new Error('Usage: verify-macos-release <app> <dmg> <zip> <team-id> <arch> <version>.');
if (!/^[A-Z0-9]{10}$/u.test(teamId)) throw new Error('Expected Apple Team ID is invalid.');
if (!['x64', 'arm64'].includes(architecture))
  throw new Error('Expected macOS architecture is invalid.');
const app = resolve(appInput);
const dmg = resolve(dmgInput);
const zip = resolve(zipInput);
const tmp = resolve('tmp/macos-verification');
const mount = resolve(tmp, 'dmg');
const unzip = resolve(tmp, 'zip');
rmSync(tmp, { recursive: true, force: true });
mkdirSync(mount, { recursive: true });
mkdirSync(unzip, { recursive: true });
verifyBundle(app, 'staged');
run('spctl', ['--assess', '--type', 'execute', '--verbose=4', app]);
run('spctl', ['--assess', '--type', 'open', '--verbose=4', dmg]);
run('xcrun', ['stapler', 'validate', app]);
run('xcrun', ['stapler', 'validate', dmg]);
run('ditto', ['-x', '-k', zip, unzip]);
verifyBundle(resolve(unzip, 'Talking Quill.app'), 'zip');
run('hdiutil', ['attach', '-readonly', '-nobrowse', '-mountpoint', mount, dmg]);
try {
  verifyBundle(resolve(mount, 'Talking Quill.app'), 'dmg');
} finally {
  run('hdiutil', ['detach', mount]);
}
console.log(
  `Verified every staged, ZIP, and DMG Mach-O against Developer ID team ${teamId}, ${architecture}, exact entitlements, hardened runtime, Gatekeeper, and stapling.`,
);

function verifyBundle(bundle, label) {
  run('codesign', ['--verify', '--deep', '--strict', '--verbose=4', bundle]);
  const executable = resolve(bundle, 'Contents/MacOS/Talking Quill');
  const helper = resolve(bundle, 'Contents/Resources/helper/talking-quill-helper');
  const electronHelpers = ['', ' (GPU)', ' (Plugin)', ' (Renderer)'].map((suffix) =>
    resolve(
      bundle,
      `Contents/Frameworks/Talking Quill Helper${suffix}.app/Contents/MacOS/Talking Quill Helper${suffix}`,
    ),
  );
  for (const path of [bundle, executable, helper, ...electronHelpers]) assertIdentity(path, true);
  const appEntitlements = entitlements(bundle, `${label}-app`);
  if (
    JSON.stringify(Object.keys(appEntitlements).sort()) !==
      JSON.stringify([
        'com.apple.security.cs.allow-jit',
        'com.apple.security.device.audio-input',
      ]) ||
    Object.values(appEntitlements).some((value) => value !== true)
  )
    throw new Error(`${label} app entitlement allowlist mismatch.`);
  if (Object.keys(entitlements(helper, `${label}-helper`)).length !== 0)
    throw new Error(`${label} Rust helper inherited Electron entitlements.`);
  for (const [index, path] of electronHelpers.entries()) {
    if (
      JSON.stringify(entitlements(path, `${label}-electron-${String(index)}`)) !==
      JSON.stringify({ 'com.apple.security.cs.allow-jit': true })
    )
      throw new Error(`${label} Electron helper entitlement allowlist mismatch: ${path}`);
  }
  const info = resolve(bundle, 'Contents/Info.plist');
  assertPlist(info, 'CFBundleIdentifier', 'com.talkingquill.app');
  assertPlist(info, 'CFBundleShortVersionString', version);
  for (const key of ['NSMicrophoneUsageDescription', 'NSScreenCaptureUsageDescription'])
    if (run('plutil', ['-extract', key, 'raw', '-o', '-', info]).trim().length < 20)
      throw new Error(`${label} ${key} is missing.`);
  const machOFiles = inventory(bundle).filter((path) =>
    run('file', ['-b', path]).includes('Mach-O'),
  );
  if (machOFiles.length < 10) throw new Error(`${label} Mach-O inventory is unexpectedly small.`);
  for (const path of machOFiles) {
    run('codesign', ['--verify', '--strict', '--verbose=4', path]);
    assertIdentity(path, false);
    assertArchitecture(path);
  }
}
function inventory(directory) {
  const result = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isSymbolicLink()) continue;
    if (entry.isDirectory()) result.push(...inventory(path));
    else if (entry.isFile() && lstatSync(path).size > 0) result.push(path);
  }
  return result;
}
function assertIdentity(path, requireRuntime) {
  const details = run('codesign', ['-dv', '--verbose=4', path], true);
  if (
    (requireRuntime && !details.includes('flags=0x10000(runtime)')) ||
    !details.includes(`TeamIdentifier=${teamId}`) ||
    !details.includes('Authority=Developer ID Application:')
  )
    throw new Error(`Code signature identity or hardened-runtime mismatch: ${path}`);
}
function entitlements(path, label) {
  const plist = resolve(tmp, `${label}.plist`);
  const json = resolve(tmp, `${label}.json`);
  const result = spawnSync('codesign', ['-d', '--entitlements', plist, path], { encoding: 'utf8' });
  if (result.error !== undefined) throw result.error;
  if (result.status !== 0) {
    throw new Error(`Entitlement extraction failed for ${path}: ${result.stderr}`);
  }
  run('plutil', ['-convert', 'json', '-o', json, plist]);
  return JSON.parse(readFileSync(json, 'utf8'));
}
function assertArchitecture(path) {
  const expected = architecture === 'x64' ? 'x86_64' : 'arm64';
  const actual = run('lipo', ['-archs', path]).trim().split(/\s+/u);
  if (actual.length !== 1 || actual[0] !== expected)
    throw new Error(`Architecture mismatch for ${path}: ${actual.join(' ')}`);
}
function assertPlist(path, key, expected) {
  if (run('plutil', ['-extract', key, 'raw', '-o', '-', path]).trim() !== expected)
    throw new Error(`${key} mismatch.`);
}
function run(command, args, captureStderr = false) {
  const result = spawnSync(command, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  if (result.status !== 0) throw new Error(`${command} ${args.join(' ')} failed: ${result.stderr}`);
  if (!captureStderr && result.stderr.length > 0) process.stderr.write(result.stderr);
  return `${result.stdout}${captureStderr ? result.stderr : ''}`;
}
