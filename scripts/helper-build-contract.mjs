import { readFile, readdir } from 'node:fs/promises';
import { join } from 'node:path';

import { readNativeArchitectures } from './native-architecture.mjs';

export const WINDOWS_TEST_PHYSICAL_MARKER = 0x5451_5445_5354_0008n;
export const WINDOWS_UPDATE_PRIMARY_KEY_MARKER = Buffer.from(
  'TALKING_QUILL_WINDOWS_UPDATE_PRIMARY_KEY_V1=',
  'ascii',
);
export const RETIRED_WINDOWS_UPDATE_BRIDGE_KEY_MARKER = Buffer.from(
  'TALKING_QUILL_WINDOWS_UPDATE_BRIDGE_KEY_V1=',
  'ascii',
);
export const GATEWAY_CANNOT_SUPPRESS_MARKER = Buffer.from(
  'TALKING_QUILL_KEYBOARD_GATEWAY=PROTOCOL_V1_GATEWAY_CANNOT_SUPPRESS',
  'ascii',
);
export const OWNER_SAFE_DISABLED_MARKER = Buffer.from(
  'TALKING_QUILL_KEYBOARD_OWNER=SAFE_DISABLED_ALL_PHYSICAL_EVENTS_PASS',
  'ascii',
);
export const OWNER_LOCAL_ENABLED_MARKER = Buffer.from(
  'TALKING_QUILL_KEYBOARD_OWNER=DEFAULT_ENABLED_OUT_OF_PROCESS_LOCAL_UNSIGNED',
  'ascii',
);
export const OWNER_TEST_SEAMS_MARKER = Buffer.from(
  'TALKING_QUILL_KEYBOARD_OWNER_TEST_SEAMS=ENABLED_SAFE_NON_PROMOTABLE',
  'ascii',
);
export const MACOS_SERVICE_BRIDGE_MARKER = Buffer.from(
  'TALKING_QUILL_MACOS_SERVICE_BRIDGE=LOCAL_MAINTENANCE_AUTHORITY_CANNOT_SUPPRESS',
  'ascii',
);
const MACOS_LIFECYCLE_FIXTURE_MARKERS = Object.freeze([
  Buffer.from('macos-native-lifecycle-fixture', 'ascii'),
  Buffer.from('TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE', 'ascii'),
  Buffer.from('permissioned-ci-v1', 'ascii'),
]);
export const LEGACY_NATIVE_MARKERS = Object.freeze([
  Buffer.from(
    'TALKING_QUILL_NATIVE_KEYBOARD_SUPPRESSION=SAFE_DISABLED_ALL_PHYSICAL_EVENTS_PASS',
    'ascii',
  ),
  Buffer.from('TALKING_QUILL_I1_TRANSACTIONAL_CAPTURE=DEFAULT_ENABLED', 'ascii'),
  Buffer.from('TALKING_QUILL_MACOS_NATIVE_TEST_SEAMS=ENABLED', 'ascii'),
]);

const ROLE_LAYOUTS = Object.freeze({
  win32: Object.freeze([
    Object.freeze({ name: 'talking-quill-helper.exe', role: 'gateway', suppressionCapable: false }),
    Object.freeze({
      name: 'talking-quill-keyboard-owner.exe',
      role: 'owner',
      suppressionCapable: true,
    }),
  ]),
  darwin: Object.freeze([
    Object.freeze({ name: 'talking-quill-helper', role: 'gateway', suppressionCapable: false }),
    Object.freeze({
      name: 'talking-quill-keyboard-owner',
      role: 'owner',
      suppressionCapable: true,
    }),
    Object.freeze({
      name: 'talking-quill-macos-service-bridge',
      role: 'authority',
      suppressionCapable: false,
    }),
  ]),
});

export function nativeRoleLayout(platform) {
  const layout = ROLE_LAYOUTS[platform];
  if (layout === undefined)
    throw new Error(`Unsupported native role platform: ${String(platform)}`);
  return layout;
}

export async function verifyOwnerBuildContract(path) {
  const executable = await readFile(path);
  if (!executable.includes(OWNER_LOCAL_ENABLED_MARKER)) {
    throw new Error('Owner artifact is missing the local enabled out-of-process marker');
  }
  for (const marker of [
    GATEWAY_CANNOT_SUPPRESS_MARKER,
    OWNER_SAFE_DISABLED_MARKER,
    OWNER_TEST_SEAMS_MARKER,
    MACOS_SERVICE_BRIDGE_MARKER,
    ...MACOS_LIFECYCLE_FIXTURE_MARKERS,
    ...LEGACY_NATIVE_MARKERS,
  ]) {
    if (executable.includes(marker))
      throw new Error('Owner artifact contains a forbidden gateway/safe/test marker');
  }
}

export async function verifyHelperBuildContract(path, { windows }) {
  const executable = await readFile(path);
  if (!executable.includes(GATEWAY_CANNOT_SUPPRESS_MARKER)) {
    throw new Error('Helper artifact is missing the permanent gateway-cannot-suppress marker');
  }
  forbidMarkersExcept(executable, 'Gateway', GATEWAY_CANNOT_SUPPRESS_MARKER);
  if (windows) {
    if (executable.includes(RETIRED_WINDOWS_UPDATE_BRIDGE_KEY_MARKER)) {
      throw new Error('Gateway artifact contains a retired updater bridge key');
    }
    const keyOffset = executable.indexOf(WINDOWS_UPDATE_PRIMARY_KEY_MARKER);
    const keyStart = keyOffset + WINDOWS_UPDATE_PRIMARY_KEY_MARKER.length;
    const embeddedKey = executable.subarray(keyStart, keyStart + 130).toString('ascii');
    if (
      keyOffset < 0 ||
      executable.indexOf(WINDOWS_UPDATE_PRIMARY_KEY_MARKER, keyOffset + 1) >= 0 ||
      !/^04[0-9a-f]{128}$/u.test(embeddedKey)
    ) {
      throw new Error('Gateway artifact has no unique valid embedded updater key');
    }
    const testPhysicalMarker = Buffer.alloc(8);
    testPhysicalMarker.writeBigUInt64LE(WINDOWS_TEST_PHYSICAL_MARKER);
    if (executable.includes(testPhysicalMarker)) {
      throw new Error('Gateway artifact contains the test-only physical input marker');
    }
  }
}

export async function verifyMacosServiceBridgeBuildContract(path) {
  const executable = await readFile(path);
  if (!executable.includes(MACOS_SERVICE_BRIDGE_MARKER)) {
    throw new Error('macOS service bridge role marker is missing');
  }
  forbidMarkersExcept(executable, 'macOS service bridge', MACOS_SERVICE_BRIDGE_MARKER);
}

export const CANONICAL_WINDOWS_NATIVE_ROLES = Object.freeze(['gateway', 'owner']);

export async function verifyCompleteNativeRoleInventory(paths, assignedRolePaths) {
  if (
    new Set(paths).size !== paths.length ||
    new Set(assignedRolePaths).size !== assignedRolePaths.length
  ) {
    throw new Error('Native role inventory paths must be distinct');
  }
  const assigned = new Set(assignedRolePaths);
  for (const path of assigned) {
    if (!paths.includes(path)) {
      throw new Error(`Assigned role is not a recognized native executable: ${path}`);
    }
  }
  for (const path of paths) {
    const executable = await readFile(path);
    if (!assigned.has(path)) {
      for (const marker of [
        GATEWAY_CANNOT_SUPPRESS_MARKER,
        OWNER_LOCAL_ENABLED_MARKER,
        OWNER_SAFE_DISABLED_MARKER,
        OWNER_TEST_SEAMS_MARKER,
        MACOS_SERVICE_BRIDGE_MARKER,
        ...MACOS_LIFECYCLE_FIXTURE_MARKERS,
        ...LEGACY_NATIVE_MARKERS,
      ]) {
        if (executable.includes(marker)) {
          throw new Error(`Role/build marker found outside its assigned executable: ${path}`);
        }
      }
    }
  }
  await verifyExactlyOneSuppressionCapableExecutable(paths);
}

export async function verifyExactlyOneSuppressionCapableExecutable(paths) {
  if (!Array.isArray(paths) || paths.length < 2 || new Set(paths).size !== paths.length) {
    throw new Error('Suppression authority verification requires distinct executable paths');
  }
  let count = 0;
  for (const path of paths) {
    if ((await readFile(path)).includes(OWNER_LOCAL_ENABLED_MARKER)) count += 1;
  }
  if (count !== 1) {
    throw new Error(
      `Native roles require exactly one suppression-capable executable; found ${String(count)}`,
    );
  }
}

/**
 * Verifies an atomic app/native staging directory. The allowlist, architecture,
 * permanent role markers, and exactly-one suppression owner are one contract.
 */
export async function verifyStagedNativeRoleSet(directory, { platform, architecture }) {
  if (!['x64', 'arm64'].includes(architecture)) {
    throw new Error(`Unsupported staged native architecture: ${String(architecture)}`);
  }
  const layout = nativeRoleLayout(platform);
  const entries = await readdir(directory, { withFileTypes: true });
  const names = entries.map((entry) => entry.name).sort();
  const expected = layout.map((entry) => entry.name).sort();
  if (
    entries.some((entry) => !entry.isFile()) ||
    names.length !== expected.length ||
    names.some((name, index) => name !== expected[index])
  ) {
    throw new Error(
      `Staged native role set mismatch: expected ${expected.join(', ')}, got ${names.join(', ')}`,
    );
  }

  const rolePaths = layout.map((role) => join(directory, role.name));
  for (const role of layout) {
    const path = join(directory, role.name);
    const executable = await readFile(path);
    const markedSuppressionCapable = executable.includes(OWNER_LOCAL_ENABLED_MARKER);
    if (markedSuppressionCapable !== role.suppressionCapable) {
      throw new Error(`Native role suppression authority mismatch: ${role.name}`);
    }
    const native = await readNativeArchitectures(path);
    const expectedFormat = platform === 'win32' ? 'pe' : 'mach-o';
    if (
      native === null ||
      (platform === 'win32' ? native.format !== 'pe' : native.format === 'pe') ||
      native.architectures.length !== 1 ||
      native.architectures[0] !== architecture
    ) {
      throw new Error(
        `Native role architecture mismatch for ${role.name}: expected ${expectedFormat}/${architecture}`,
      );
    }
  }
  await verifyExactlyOneSuppressionCapableExecutable(rolePaths);

  const gateway = join(directory, layout.find((role) => role.role === 'gateway').name);
  const owner = join(directory, layout.find((role) => role.role === 'owner').name);
  await verifyHelperBuildContract(gateway, { windows: platform === 'win32' });
  await verifyOwnerBuildContract(owner);
  if (platform !== 'win32') {
    await verifyMacosServiceBridgeBuildContract(
      join(directory, 'talking-quill-macos-service-bridge'),
    );
  }
}

function forbidMarkersExcept(executable, label, allowedRoleMarker) {
  for (const marker of [
    GATEWAY_CANNOT_SUPPRESS_MARKER,
    OWNER_LOCAL_ENABLED_MARKER,
    OWNER_SAFE_DISABLED_MARKER,
    OWNER_TEST_SEAMS_MARKER,
    MACOS_SERVICE_BRIDGE_MARKER,
    ...MACOS_LIFECYCLE_FIXTURE_MARKERS,
    ...LEGACY_NATIVE_MARKERS,
  ]) {
    if (marker !== allowedRoleMarker && executable.includes(marker)) {
      throw new Error(`${label} contains a forbidden or mixed role/build marker`);
    }
  }
}
