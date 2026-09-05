import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { sanitizedSubprocessEnvironment } from '../../scripts/environment-policy.mjs';

const windows = process.platform === 'win32' ? describe : describe.skip;

it('does not forward a parent PowerShell module search path to tool subprocesses', () => {
  const environment = sanitizedSubprocessEnvironment(
    {
      SystemRoot: 'C:/Windows',
      PSModulePath: 'C:/Program Files/PowerShell/7/Modules',
      psMODULEpath: 'C:/untrusted/modules',
    },
    { TQ_TEST_ACL: 'C:/fixture' },
  );
  expect(environment).toEqual({ SystemRoot: 'C:/Windows', TQ_TEST_ACL: 'C:/fixture' });
});

windows('Windows PowerShell subprocess environment', () => {
  it('loads the Desktop security module when Node is launched by PowerShell Core', () => {
    const probe = `
      import assert from 'node:assert/strict';
      import { spawnSync } from 'node:child_process';
      import { resolve } from 'node:path';
      import { sanitizedSubprocessEnvironment } from './scripts/environment-policy.mjs';
      assert(process.env.PSModulePath.split(';').some(
        path => path.toLowerCase() === process.env.TQ_CORE_MODULES.toLowerCase()
      ), 'The probe must inherit the Core module path through Node');
      const result = spawnSync(
        resolve(process.env.SystemRoot, 'System32/WindowsPowerShell/v1.0/powershell.exe'),
        ['-NoProfile', '-NonInteractive', '-Command',
          '$ErrorActionPreference="Stop"; ' +
          '$acl=Get-Acl -LiteralPath $env:TQ_TEST_ACL; ' +
          '[pscustomobject]@{Sddl=$acl.Sddl;Module=(Get-Module Microsoft.PowerShell.Security).Path;Home=$PSHOME;Edition=$PSEdition}|ConvertTo-Json -Compress'],
        { env: sanitizedSubprocessEnvironment(process.env, { TQ_TEST_ACL: process.cwd() }),
          encoding: 'utf8', windowsHide: true, timeout: 30_000 }
      );
      assert.equal(result.error, undefined);
      assert.equal(result.status, 0, result.stderr);
      const acl = JSON.parse(result.stdout);
      assert.equal(acl.Edition, 'Desktop');
      assert(acl.Sddl.length > 0, 'Get-Acl must return the descriptor');
      assert(acl.Module.toLowerCase().startsWith(acl.Home.toLowerCase() + '\\\\'),
        'The security module must come from Windows PowerShell, not Core');
      process.stdout.write('desktop-acl-ok');
    `;
    const result = spawnSync(
      'pwsh.exe',
      [
        '-NoProfile',
        '-NonInteractive',
        '-Command',
        "$ErrorActionPreference='Stop'; $env:TQ_CORE_MODULES=Join-Path $PSHOME 'Modules'; & $env:TQ_NODE --input-type=module --eval $env:TQ_PROBE; exit $LASTEXITCODE",
      ],
      {
        cwd: resolve('.'),
        env: sanitizedSubprocessEnvironment(process.env, {
          TQ_NODE: process.execPath,
          TQ_PROBE: probe,
        }),
        encoding: 'utf8',
        windowsHide: true,
        timeout: 45_000,
      },
    );
    expect(result.error).toBeUndefined();
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout).toBe('desktop-acl-ok');
  }, 60_000);
});
