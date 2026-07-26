import { describe, expect, it, vi } from 'vitest';
import {
  MICROPHONE_SETTINGS_URLS,
  MicrophonePermissionController,
  sameSecurityOrigin,
  type MicrophonePermissionPlatform,
} from '../../app/src/main/security/microphone-permission';
import {
  readPermissionCheckMediaTypes,
  readPermissionRequestMediaTypes,
} from '../../app/src/main/security/session-policy';

function platform(
  name: NodeJS.Platform = 'win32',
  status: ReturnType<MicrophonePermissionPlatform['getStatus']> = 'not-determined',
) {
  const openExternal = vi.fn<(url: string) => Promise<void>>().mockResolvedValue(undefined);
  return {
    value: {
      platform: name,
      getStatus: () => status,
      openExternal,
    } satisfies MicrophonePermissionPlatform,
    openExternal,
  };
}

const captureUrl = 'talking-quill://app/capture/index.html';
const captureOrigin = 'talking-quill://app';
const electronCaptureOrigin = 'talking-quill://app/';
const audioRequest = {
  webContentsId: 7,
  permission: 'media',
  mediaTypes: ['audio'],
  isMainFrame: true,
  requestingUrl: captureUrl,
  requestingOrigin: electronCaptureOrigin,
  securityOrigin: electronCaptureOrigin,
  embeddingOrigin: null,
  expectedUrl: captureUrl,
  expectedOrigin: captureOrigin,
} as const;

describe('microphone permission policy', () => {
  it('reads Electron request and check handler details using their actual distinct fields', () => {
    expect(readPermissionRequestMediaTypes({ mediaTypes: ['audio'] })).toEqual(['audio']);
    expect(readPermissionRequestMediaTypes({ mediaType: 'audio' })).toEqual([]);
    expect(readPermissionCheckMediaTypes({ mediaType: 'audio' })).toEqual(['audio']);
    expect(readPermissionCheckMediaTypes({ mediaTypes: ['audio'] })).toEqual([]);
  });

  it('forces acquisition through a bounded request and grants enumeration checks separately', () => {
    const source = platform();
    const controller = new MicrophonePermissionController(source.value, () => 1_000);
    controller.authorize(7, 'capture-id');

    expect(controller.allowsCheck(audioRequest)).toBe(false);
    controller.notePolicyDenied(audioRequest);
    expect(controller.takePolicyDenial('capture-id')).toBe(false);
    expect(controller.allowsRequest(audioRequest)).toBe(true);
    expect(controller.allowsRequest(audioRequest)).toBe(false);

    controller.authorize(7, 'fallback-capture', 2);
    expect(controller.allowsRequest(audioRequest)).toBe(true);
    expect(controller.allowsRequest(audioRequest)).toBe(true);
    expect(controller.allowsRequest(audioRequest)).toBe(false);

    controller.authorizeEnumeration(7, 'capture-id');
    expect(controller.allowsCheck(audioRequest)).toBe(true);
    expect(controller.allowsRequest(audioRequest)).toBe(false);
    controller.seal('capture-id');
    expect(controller.allowsCheck(audioRequest)).toBe(false);
  });

  it.each([
    { ...audioRequest, webContentsId: 8 },
    { ...audioRequest, permission: 'camera' },
    { ...audioRequest, mediaTypes: ['video'] },
    { ...audioRequest, mediaTypes: ['audio', 'video'] },
    { ...audioRequest, isMainFrame: false },
    { ...audioRequest, requestingUrl: 'talking-quill://app/main/index.html' },
    { ...audioRequest, requestingOrigin: 'https://attacker.invalid' },
    { ...audioRequest, securityOrigin: 'https://attacker.invalid' },
    { ...audioRequest, embeddingOrigin: captureOrigin },
    { ...audioRequest, expectedUrl: null },
  ])('denies wrong sender, media, frame, URL, origin, embedding, and trust context', (request) => {
    const source = platform();
    const controller = new MicrophonePermissionController(source.value, () => 1_000);
    controller.authorize(7, 'capture-id');
    expect(controller.allowsRequest(request)).toBe(false);
  });

  it('expires unused leases and releases active leases by capture id', () => {
    let now = 0;
    const source = platform();
    const controller = new MicrophonePermissionController(source.value, () => now);
    controller.authorize(7, 'capture-id');
    now = 15_000;
    expect(controller.allowsRequest(audioRequest)).toBe(false);
    controller.authorizeEnumeration(7, 'capture-id');
    expect(controller.allowsCheck(audioRequest)).toBe(true);
    controller.release('different');
    expect(controller.allowsCheck(audioRequest)).toBe(true);
    controller.release('capture-id');
    expect(controller.allowsCheck(audioRequest)).toBe(false);
  });

  it('normalizes Electron custom and development origin variants without broadening hosts', () => {
    expect(sameSecurityOrigin('talking-quill://app/', 'talking-quill://app')).toBe(true);
    expect(sameSecurityOrigin('http://localhost:5173/', 'http://localhost:5173')).toBe(true);
    expect(sameSecurityOrigin('http://localhost:5173', 'http://127.0.0.1:5173')).toBe(false);
    expect(sameSecurityOrigin('https://app:443', 'https://app')).toBe(true);
    expect(sameSecurityOrigin('not a URL', captureOrigin)).toBe(false);
  });

  it('records a policy denial for the owning capture without changing OS status', () => {
    const source = platform('win32', 'granted');
    const controller = new MicrophonePermissionController(source.value, () => 1_000);
    controller.authorize(7, 'capture-id');
    const malformed = { ...audioRequest, securityOrigin: 'https://attacker.invalid' };
    expect(controller.allowsRequest(malformed)).toBe(false);
    controller.notePolicyDenied(malformed);
    expect(controller.takePolicyDenial('capture-id')).toBe(true);
    expect(controller.takePolicyDenial('capture-id')).toBe(false);
    expect(controller.getStatus()).toBe('granted');
  });

  it('publishes only the literal supported operating-system settings links', () => {
    expect(MICROPHONE_SETTINGS_URLS).toEqual({
      win32: 'ms-settings:privacy-microphone',
      darwin: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone',
    });
  });

  it.each([
    ['win32', 'ms-settings:privacy-microphone'],
    ['darwin', 'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone'],
  ] as const)('opens only the fixed %s microphone settings URL', async (name, expectedUrl) => {
    const source = platform(name);
    const controller = new MicrophonePermissionController(source.value);
    await controller.openSettings();
    expect(source.openExternal).toHaveBeenCalledWith(expectedUrl);
  });

  it('rejects unsupported platforms without invoking an arbitrary URL', async () => {
    const source = platform('linux');
    const controller = new MicrophonePermissionController(source.value);
    await expect(controller.openSettings()).rejects.toThrow('unavailable');
    expect(source.openExternal).not.toHaveBeenCalled();
  });
});
