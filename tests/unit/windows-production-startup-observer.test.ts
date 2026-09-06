import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const driver = readFileSync('scripts/windows-production-startup-smoke.ps1', 'utf8');
const observer = readFileSync('scripts/windows-production-startup-observer.cs', 'utf8');

describe('hosted production startup observer boundaries', () => {
  it('guards fresh hosted profiles and launches the production executable without hooks', () => {
    expect(driver).toContain("$env:RUNNER_ENVIRONMENT -cne 'github-hosted'");
    expect(driver).toContain("GetFolderPath('ApplicationData')");
    expect(driver).toContain('if (Test-Path -LiteralPath $profile)');
    expect(driver).toContain('if (@(Package-Processes).Count -ne 0)');
    expect(driver).toContain('[StartupCapture]::new($application, $root)');
    expect(observer).toContain('new ProcessStartInfo(executable)');
    expect(observer).not.toContain('Arguments =');
    expect(driver).not.toContain('--talking-quill-');
    expect(observer).not.toContain('--talking-quill-');
    expect(driver).not.toContain('Set-Content');
  });

  it('requires rendered descendant accessibility controls and exact live roles, not a title alone', () => {
    expect(observer).toContain('IsWindowVisible(window)');
    expect(observer).toContain('AutomationElement.FromHandle');
    expect(observer).toContain('TreeWalker.ControlViewWalker');
    expect(driver).toContain('$_.Depth -gt 0 -and -not $_.Offscreen');
    expect(driver).toContain(
      "$_.Name -ceq 'Continue' -and $_.ControlType -eq 'ControlType.Button'",
    );
    expect(driver).toContain(
      '$helpers.Count -eq 1 -and $owners.Count -eq 1 -and $renderers.Count -gt 0',
    );
    expect(driver).toContain('$_.path -ieq $helper -and $known.ContainsKey($_.pid)');
    expect(driver).toContain('$_.path -ieq $owner -and $known.ContainsKey($_.pid)');
    expect(driver).toContain('$stable -ge 2');
    expect(driver).toContain("$window.ClassName -eq '#32770'");
    expect(driver).toContain('startup-window.png');
    expect(driver).toContain('error-window.png');
  });

  it('bounds observation and output and reports force cleanup without claiming lifecycle success', () => {
    expect(driver).toContain('$clock.ElapsedMilliseconds -lt 90000');
    expect(observer).toContain('task.Wait(3000)');
    expect(observer).toContain('task.Wait(5000)');
    expect(observer).toContain('Limit = 262144');
    expect(driver).toContain("@('', '.1', '.2')");
    expect(driver).toContain('New-Object byte[] 262144');
    expect(driver).toContain('[StartupObserver]::Close($window.Handle)');
    expect(driver).toContain('if (-not $known.ContainsKey($item.pid)) { continue }');
    expect(driver).toContain('$item.created -cne $expected.created');
    expect(driver).toContain('$cleanup.forcedTermination = $true');
    expect(driver).toContain("ownerAuthentication = 'not-observed'");
    expect(driver).toContain("gracefulLifecycle = 'not-asserted'");
    expect(driver).not.toContain('taskkill');
    expect(driver).not.toContain('Stop-Process -Name');
  });
});
