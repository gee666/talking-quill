import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const buildHelper = readFileSync('scripts/build-helper.mjs', 'utf8');
const prepackage = readFileSync('scripts/prepackage-check.mjs', 'utf8');
const cargo = readFileSync('helper/Cargo.toml', 'utf8');
const gateway = readFileSync('helper/src/owner/platform_client.rs', 'utf8');
const ownerClient = readFileSync('helper/src/owner/client.rs', 'utf8');
const helperMain = readFileSync('helper/src/main.rs', 'utf8');
const acceptanceLauncher = readFileSync('helper/src/windows_acceptance_launcher.rs', 'utf8');
const installerCargo = readFileSync('installer/windows-setup/Cargo.toml', 'utf8');
const installerPackage = readFileSync('installer/windows-setup/src/package.rs', 'utf8');
const faultSetup = readFileSync('scripts/build-windows-acceptance-fault-setup.mjs', 'utf8');
const repairSetup = readFileSync('scripts/build-windows-acceptance-repair-setup.mjs', 'utf8');

const environment = 'TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD';

describe('Windows installed-acceptance helper feature gate', () => {
  it('declares a non-default Cargo feature and enables it only for an explicit Windows build', () => {
    expect(cargo).toMatch(
      /\[features\][\s\S]*default = \[\][\s\S]*windows-installed-acceptance = \[\]/u,
    );
    expect(buildHelper).toContain(`const acceptanceBuildEnvironment = '${environment}'`);
    expect(buildHelper).toContain("acceptanceBuildValue === '1' && platform === 'win32'");
    expect(buildHelper).toContain("['windows-installed-acceptance']");
    expect(buildHelper).toContain(
      "buildCargoRole('talking-quill-helper', 'talking-quill-helper', gatewayFeatures)",
    );
  });

  it('requires an explicit nonpromotable installer feature for acceptance repair packages', () => {
    expect(installerCargo).toContain('installed-acceptance-repair = []');
    expect(installerCargo).toContain('acceptance-faults = ["installed-acceptance-repair"]');
    expect(installerPackage).toContain('cfg!(feature = "installed-acceptance-repair")');
    expect(faultSetup).toContain('installed-acceptance-repair,acceptance-faults');
    expect(faultSetup).toContain('installedAcceptanceRepair: true');
    expect(repairSetup).toContain("'installed-acceptance-repair,machine-lock-test-namespace'");
    expect(repairSetup).toContain('acceptanceFaults: false');
  });

  it('keeps the long-lived broker CLI inside the acceptance feature gate', () => {
    expect(helperMain).toContain('feature = "windows-installed-acceptance"');
    expect(helperMain).toContain('--windows-installed-acceptance-broker-v1');
    expect(acceptanceLauncher).toContain('MAX_BROKER_FRAME_BYTES');
    expect(acceptanceLauncher).toContain('_retained_broker_image');
  });

  it('keeps the lease-expiry probe fixed, disabled-first, and unable to acquire a replacement lease', () => {
    expect(gateway).toContain(
      'const ACCEPTANCE_LEASE_RENEWAL_PAUSE: Duration = Duration::from_millis(6_500)',
    );
    expect(gateway).toContain('.force_capture_safe_disabled_until(deadline, cancelled)');
    expect(gateway).toContain('.connect_existing_capture()');
    expect(gateway).toContain('state.acceptance_safe_disabled = true');
    const observerStart = ownerClient.indexOf('pub fn acceptance_observability_without_lease');
    const observerEnd = ownerClient.indexOf('#[derive(Clone, Copy', observerStart);
    const observer = ownerClient.slice(observerStart, observerEnd);
    expect(observer).toContain('Request::ObservabilityGet(Empty {})');
    expect(observer).not.toContain('LeaseAcquire');
  });

  it('rejects and strips the acceptance build variable from canonical production packaging', () => {
    expect(prepackage).toContain(`const ACCEPTANCE_BUILD_ENV = '${environment}'`);
    expect(prepackage).toContain('normalizedName === ACCEPTANCE_BUILD_ENV');
    expect(prepackage).toContain('env: sanitizedSubprocessEnvironment(environment)');
  });
});
