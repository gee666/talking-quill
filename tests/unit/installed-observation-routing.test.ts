import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { classifyWindowsLoginStartArguments } from '../../app/src/main/app/launch-at-login-service';

describe('installed observation routing', () => {
  it('observes the helper owned by the normally started application', () => {
    const index = readFileSync('app/src/main/index.ts', 'utf8');
    const application = readFileSync('app/src/main/app/application.ts', 'utf8');
    expect(index).toContain('application = new TalkingQuillApplication({ windowsLoginStart })');
    expect(index).toContain('await application.start()');
    expect(index).toContain('await application.runInstalledObservation({');
    expect(index).toMatch(/finally\s*\{\s*await application\.stop\(\)/u);
    expect(index).not.toContain('process.exit(0)');
    expect(index).not.toContain('new HelperClient');
    expect(index).not.toContain('--talking-quill-diagnostic-capability=');
    expect(index).not.toContain('installedDiagnosticCapability');
    expect(application).not.toContain('installedObservationHelper()');
    expect(application).toContain('runInstalledObservation(request: InstalledObservationRequest)');
    expect(application).toContain('await runInstalledObservation(this.#helper, request, {');
    expect(application).toContain('profiles: this.#settings.get().dictationProfiles');
    expect(application).toContain('persistentWindowRolesReady: windows.hasPersistentWindowRoles()');
    expect(application).toContain('windows.createWidgetForActivation()');
    expect(application).toContain('windows.showWidget(');
    expect(application).toContain('hideValidationWidget: () => windows.removeWidget()');
    const observation = readFileSync('app/src/main/app/installed-observation.ts', 'utf8');
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
