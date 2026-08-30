import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const workflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
const stageScript = readFileSync('scripts/stage-unsigned-release.mjs', 'utf8');
const approvedActions = new Set([
  'actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09',
  'actions/setup-node@a0853c24544627f65ddf259abe73b1d18a591444',
  'actions/upload-artifact@b7c566a772e6b6bfb58ed0dc250532a479d7789f',
  'actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c',
  'pnpm/action-setup@fc06bc1257f339d1d5d8b3a19a8cae5388b55320',
  'dtolnay/rust-toolchain@46511b1c83438f0dd37c02d843619ece5a4abb5b',
  'taiki-e/install-action@1beb33eee6d086258184383af9a538940be190ed',
]);

function section(start: string, end?: string): string {
  const startIndex = workflow.indexOf(`\n  ${start}:`);
  const endIndex = end === undefined ? workflow.length : workflow.indexOf(`\n  ${end}:`);
  expect(startIndex, start).toBeGreaterThan(-1);
  expect(endIndex, end).toBeGreaterThan(startIndex);
  return workflow.slice(startIndex, endIndex);
}

describe('Windows native release workflow', () => {
  it('runs the complete validation, audit, formatting, clippy, security, and source-independence gates', () => {
    const validate = section('validate', 'package');
    expect(validate).toContain('fetch-depth: 0');
    expect(validate).toContain('pnpm rust:fetch-targets');
    expect(validate).toContain('pnpm validate:unsigned-release');
    expect(validate).toContain('cargo-audit@0.22.2');
    expect(validate).toContain('pnpm security:gate');
  });
  it('builds architecture-specific x64 and ARM64 NSIS candidates', () => {
    const packageJob = section('package', 'lifecycle');
    expect(packageJob).toContain('package_script: package:win');
    expect(packageJob).toContain('package_script: package:win:arm64');
    expect(packageJob).toContain('TALKING_QUILL_PACKAGE_ARCH: ${{ matrix.arch }}');
    expect(workflow).not.toContain('predecessor_manifest');
    expect(workflow).toContain('runner: windows-11-arm');
    expect(workflow).toContain('predecessor_x64_gateway_sha256');
    expect(workflow).toContain('predecessor_arm64_gateway_sha256');
    expect(workflow).toContain('predecessor_x64_update_public_key_sha256');
    expect(workflow).toContain('predecessor_arm64_update_public_key_sha256');
    expect(workflow).toContain('TALKING_QUILL_PREDECESSOR_UPDATE_PUBLIC_KEY_SHA256');
    expect(workflow).toContain('environment: release-signing');
    const signingSecret = 'TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64';
    expect(
      packageJob.match(/secrets\.TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64/gu),
    ).toHaveLength(1);
    const signingStep = packageJob.indexOf(
      'name: Authorize updater metadata and stage exact payload',
    );
    expect(signingStep).toBeGreaterThan(packageJob.indexOf('${{ matrix.package_script }}'));
    expect(packageJob.indexOf(signingSecret, signingStep)).toBeGreaterThan(signingStep);
    const jobEnvironment = packageJob.slice(
      packageJob.indexOf('    env:'),
      packageJob.indexOf('    steps:'),
    );
    expect(jobEnvironment).not.toContain(signingSecret);
    expect(workflow).not.toContain('secrets.TALKING_QUILL_WINDOWS_UPDATE_PUBLIC_KEY_SEC1');
    expect(workflow).not.toContain('TALKING_QUILL_WINDOWS_UPDATE_BRIDGE_PUBLIC_KEY_SEC1');
    expect(workflow).not.toContain('TalkingQuillKeyboardAuthority');
    expect(workflow).not.toMatch(/gh release (?:create|edit|upload)/u);
    expect(stageScript).not.toContain("platform === 'win' && arch !== 'x64'");
    expect(stageScript).toContain("!['x64', 'arm64'].includes(arch)");
  });

  it('stages and uploads the bound installer before lifecycle and assembly', () => {
    const packageJob = section('package', 'lifecycle');
    const cleanTree = packageJob.indexOf('git status --porcelain --untracked-files=normal');
    const dependencyInstall = packageJob.indexOf('pnpm install --frozen-lockfile');
    expect(cleanTree).toBeGreaterThan(-1);
    expect(dependencyInstall).toBeGreaterThan(cleanTree);
    const packageCommand = packageJob.indexOf('package:win');
    const stageCommand = packageJob.indexOf('stage-unsigned-release.mjs win ${{ matrix.arch }}');
    const assembleCommand = packageJob.indexOf('node scripts/assemble-release.mjs');
    const immediateUpload = packageJob.indexOf('name: Upload staged native NSIS immediately');
    expect(packageCommand).toBeGreaterThan(-1);
    expect(stageCommand).toBeGreaterThan(packageCommand);
    expect(assembleCommand).toBeGreaterThan(stageCommand);
    expect(immediateUpload).toBeGreaterThan(assembleCommand);
    expect(packageJob).toContain('tmp/release-upload/latest-${{ matrix.arch }}.yml');
    expect(packageJob).toContain('tmp/release-upload/release-identity-win-${{ matrix.arch }}.json');
    expect(packageJob).toContain('tmp/release-upload/provenance-win-${{ matrix.arch }}.json');
    expect(packageJob).toContain('tmp/release-upload/release-manifest.json');
    expect(readFileSync('scripts/assemble-release.mjs', 'utf8')).toContain('promotable: true');
    expect(stageScript).toContain("resolve(output, 'THIRD_PARTY_NOTICES.txt')");
    expect(packageJob).toContain('name: windows-${{ matrix.arch }}-validated-nsis');
    const lifecycle = section('lifecycle', 'assemble');
    expect(lifecycle).toContain(
      'windows-package-lifecycle.mjs --arch ${{ matrix.arch }} --mode unpacked',
    );
    expect(lifecycle).toContain('name: windows-${{ matrix.arch }}-validated-nsis');
    expect(lifecycle).toContain('Start-Process -FilePath $predecessor');
    expect(lifecycle).toContain('PREDECESSOR_INSTALLER_SHA256');
    expect(lifecycle).toContain('Start-Process -FilePath $installers[0].FullName');
    expect(lifecycle).toContain('--mode installed --root');
    expect(lifecycle).toContain('Uninstall Talking Quill.exe');
    const assemble = section('assemble');
    expect(assemble).toContain('windows-x64-validated-nsis');
    expect(assemble).toContain('windows-arm64-validated-nsis');
    expect(assemble).toContain('windows-native-release-candidates');
    expect(assemble).toContain('join(",") !== "gateway,owner"');
    expect(assemble).toContain('value.predecessor === null');
    expect(assemble).toContain('cp preserved/x64/tmp/release-upload/release-manifest.json');
  });

  it('pins every workflow action to its reviewed commit', () => {
    const actions = [...workflow.matchAll(/\buses:\s*([^\s#]+)/gu)].map((match) => match[1]);
    expect(actions.length).toBeGreaterThan(0);
    for (const action of actions) {
      expect(action).toMatch(/^[^@\s]+@[a-f0-9]{40}$/u);
      expect(approvedActions).toContain(action);
    }
  });
});
