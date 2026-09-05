import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { classifyWindowsLoginStartArguments } from '../../app/src/main/app/launch-at-login-service';

describe('installed observation routing', () => {
  it('keeps installed observation code behind the noncanonical entry', () => {
    const index = readFileSync('app/src/main/index.ts', 'utf8');
    const bootstrap = readFileSync('app/src/main/bootstrap.ts', 'utf8');
    const application = readFileSync('app/src/main/app/application.ts', 'utf8');
    const entry = readFileSync('app/src/main/entries/windows-installed-acceptance.ts', 'utf8');
    const observation = [
      'installed-observation',
      'installed-readiness-observation',
      'installed-physical-observation',
    ]
      .map((name) => readFileSync(`app/src/main/acceptance/${name}.ts`, 'utf8'))
      .join('\n');
    for (const canonical of [index, bootstrap, application]) {
      expect(canonical).not.toContain('authorizeInstalledAcceptance');
      expect(canonical).not.toContain('runInstalledObservation');
      expect(canonical).not.toContain('runExtension');
      expect(canonical).not.toContain('ApplicationRuntimeExtension');
      expect(canonical).not.toContain('--talking-quill-installed-readiness-pipe=');
    }
    expect(index).toBe("import { startMain } from './bootstrap';\n\nstartMain();\n");
    expect(entry).toContain('authorizeInstalledAcceptanceRequest({');
    expect(entry).toContain('installedObservation: {');
    expect(entry).not.toContain('extension:');
    expect(observation).not.toContain('new BrowserWindow');
    expect(observation).toContain('context.showValidationWidget()');
    expect(observation).toContain("failureStage = 'user-data-root'");
    expect(observation).toContain('userDataRootSha256: createHash');
  });

  it('classifies only one exact packaged Windows login-start marker', () => {
    expect(
      classifyWindowsLoginStartArguments(
        ['Talking Quill.exe', '--talking-quill-login-start'],
        true,
        'win32',
      ),
    ).toBe('login-start');
    expect(
      classifyWindowsLoginStartArguments(
        ['Talking Quill.exe', '--talking-quill-login-start', '--talking-quill-login-start'],
        true,
        'win32',
      ),
    ).toBe('invalid');
    expect(
      classifyWindowsLoginStartArguments(
        ['Talking Quill.exe', '--talking-quill-login-start'],
        false,
        'win32',
      ),
    ).toBe('invalid');
  });

  it('launches unpacked and installed lifecycle through Talking Quill.exe only', () => {
    const source = readFileSync('scripts/windows-package-lifecycle.mjs', 'utf8');
    expect(source).not.toContain('new Gateway(helper)');
    expect(source).toContain('return spawn(');
    expect(source).toContain('application,');
    expect(source).toContain('--talking-quill-installed-readiness-pipe=');
    expect(source).toContain('--talking-quill-installed-automation-validation');
    expect(source).toContain('--talking-quill-installed-lifecycle-user-data=');
    expect(source).toContain("tmpdir(),\n  'TalkingQuillInstalledLifecycle'");
    expect(source).toContain("first.userDataRootSha256 !== createHash('sha256').update(profile)");
  });

  it('requires clean role exit before the successor application starts', () => {
    const source = readFileSync('scripts/windows-package-lifecycle.mjs', 'utf8');
    expect(source).toContain('process.kill(before.gateways[0].ProcessId)');
    const crash = source.indexOf('const crash = await launchArmedLifecycle()');
    const absence = source.indexOf('await waitForRolesAbsent(45_000)', crash);
    const successor = source.indexOf("await readinessLaunch('successor')", absence);
    expect(crash).toBeGreaterThan(-1);
    expect(absence).toBeGreaterThan(crash);
    expect(successor).toBeGreaterThan(absence);
  });
});
