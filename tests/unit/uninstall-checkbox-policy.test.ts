import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';

describe('Windows uninstall checkbox policy', () => {
  it('fails closed when elevation does not transfer an exact target', async () => {
    const source = await readFile('app/src/main/bootstrap.ts', 'utf8');
    expect(source).toContain("throw new Error('Uninstall reset target transfer is unavailable')");
    expect(source).not.toMatch(/uninstallResetTargetArgument === undefined\s*\? app\.getPath/u);
    expect(source).toContain('allowedBase: dirname(dirname(dirname(resetTarget)))');
  });

  it('keeps isolated validation off the real signed-in profile resolver', async () => {
    const installer = await readFile('build/installer.nsh', 'utf8');
    const armedBranch = installer.slice(
      installer.indexOf(
        '${If} $TalkingQuillTestEvidenceRoot != ""',
        installer.indexOf('!macro customUnInstall'),
      ),
      installer.indexOf(
        '${Else}',
        installer.indexOf(
          '${If} $TalkingQuillTestEvidenceRoot != ""',
          installer.indexOf('!macro customUnInstall'),
        ),
      ),
    );
    expect(armedBranch).toContain('talking-quill-user-data-test-target.ps1');
    expect(armedBranch).not.toContain('talking-quill-user-data-target.ps1"');
    expect(installer).toContain('${NSD_Uncheck} $DeleteTalkingQuillDataCheckbox');
    expect(installer).toContain('MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2');
    expect(installer).toContain('Abort\n    data_removal_confirmed:');
    expect(installer).toContain('talking-quill-uninstall-reset-v1');
  });
});
