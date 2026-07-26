import { describe, expect, it, vi } from 'vitest';
import type { Session, WebContents } from 'electron';
import type { WindowRoleRegistry } from '../../app/src/main/app/window-role-registry';
import {
  MicrophonePermissionController,
  type MicrophonePermissionPlatform,
} from '../../app/src/main/security/microphone-permission';
import {
  getTrustedCaptureDocument,
  secureSession,
} from '../../app/src/main/security/session-policy';

type RequestHandler = (
  webContents: WebContents,
  permission: string,
  callback: (granted: boolean) => void,
  details: unknown,
) => void;
type CheckHandler = (
  webContents: WebContents | null,
  permission: string,
  origin: string,
  details: unknown,
) => boolean;

function webContents(id = 7, url = 'talking-quill://app/capture/index.html', destroyed = false) {
  return {
    id,
    isDestroyed: () => destroyed,
    mainFrame: { url },
  } as unknown as WebContents;
}

function roles(
  role: 'capture' | 'main' = 'capture',
  expectedUrl = 'talking-quill://app/capture/index.html',
) {
  return {
    get: () => ({ role, expectedUrl }),
  } as unknown as WindowRoleRegistry;
}

function harness() {
  const installed: { request?: RequestHandler; check?: CheckHandler } = {};
  const mocks = {
    setPermissionRequestHandler: vi.fn((handler: unknown) => {
      installed.request = handler as RequestHandler;
    }),
    setPermissionCheckHandler: vi.fn((handler: unknown) => {
      installed.check = handler as CheckHandler;
    }),
    on: vi.fn(),
    removeListener: vi.fn(),
    onBeforeRequest: vi.fn(),
    onHeadersReceived: vi.fn(),
  };
  const target = {
    setPermissionRequestHandler: mocks.setPermissionRequestHandler,
    setPermissionCheckHandler: mocks.setPermissionCheckHandler,
    on: mocks.on,
    removeListener: mocks.removeListener,
    webRequest: {
      onBeforeRequest: mocks.onBeforeRequest,
      onHeadersReceived: mocks.onHeadersReceived,
    },
  } as unknown as Session;
  const platform: MicrophonePermissionPlatform = {
    platform: 'win32',
    getStatus: () => 'not-determined',
    openExternal: () => Promise.resolve(),
  };
  const controller = new MicrophonePermissionController(platform, () => 1_000);
  const registry = roles();
  const dispose = secureSession(target, null, {
    microphone: {
      controller,
      getTrustedCaptureDocument: (contents) => getTrustedCaptureDocument(contents, registry),
    },
  });
  const requestHandler = installed.request;
  const checkHandler = installed.check;
  if (requestHandler === undefined || checkHandler === undefined) {
    throw new Error('Handlers not installed');
  }
  return { checkHandler, controller, dispose, mocks, requestHandler, target };
}

const captureUrl = 'talking-quill://app/capture/index.html';
const captureOrigin = 'talking-quill://app';
const electronCaptureOrigin = 'talking-quill://app/';
const requestDetails = {
  isMainFrame: true,
  requestingUrl: captureUrl,
  securityOrigin: electronCaptureOrigin,
  mediaTypes: ['audio'],
};
const checkDetails = {
  isMainFrame: true,
  requestingUrl: captureUrl,
  securityOrigin: electronCaptureOrigin,
  mediaType: 'audio',
};

function requestPermission(
  handler: RequestHandler,
  contents: WebContents,
  details: unknown,
): boolean {
  let result = false;
  handler(
    contents,
    'media',
    (granted) => {
      result = granted;
    },
    details,
  );
  return result;
}

describe('integrated Electron microphone session policy', () => {
  it('accepts Electron callback shapes and custom-origin variants without consuming intent', () => {
    const test = harness();
    const contents = webContents();
    test.controller.authorize(contents.id, 'first-capture');

    expect(test.checkHandler(contents, 'media', electronCaptureOrigin, checkDetails)).toBe(false);
    expect(
      test.checkHandler(contents, 'media', captureOrigin, {
        ...checkDetails,
        mediaType: undefined,
        mediaTypes: ['audio'],
      }),
    ).toBe(false);
    expect(
      test.checkHandler(contents, 'media', captureOrigin, {
        ...checkDetails,
        mediaType: 'video',
      }),
    ).toBe(false);
    expect(requestPermission(test.requestHandler, contents, requestDetails)).toBe(true);
    expect(requestPermission(test.requestHandler, contents, requestDetails)).toBe(false);
    test.controller.release('first-capture');
    expect(requestPermission(test.requestHandler, contents, requestDetails)).toBe(false);

    test.controller.authorize(contents.id, 'second-capture');
    expect(requestPermission(test.requestHandler, contents, requestDetails)).toBe(true);
    expect(
      requestPermission(test.requestHandler, contents, {
        ...requestDetails,
        mediaTypes: ['audio', 'video'],
      }),
    ).toBe(false);

    test.controller.authorizeEnumeration(contents.id, 'enumeration');
    expect(test.checkHandler(contents, 'media', electronCaptureOrigin, checkDetails)).toBe(true);
    expect(requestPermission(test.requestHandler, contents, requestDetails)).toBe(false);
    test.controller.seal('enumeration');

    test.controller.authorize(contents.id, 'embedded-capture');
    expect(
      requestPermission(test.requestHandler, contents, {
        ...requestDetails,
        embeddingOrigin: captureOrigin,
      }),
    ).toBe(false);
  });

  it('denies the wrong role, URL, destroyed sender, missing lease, and non-media permission', () => {
    expect(getTrustedCaptureDocument(webContents(), roles())).not.toBeNull();
    expect(getTrustedCaptureDocument(webContents(), roles('main'))).toBeNull();
    expect(
      getTrustedCaptureDocument(webContents(7, 'talking-quill://app/main/index.html'), roles()),
    ).toBeNull();
    expect(getTrustedCaptureDocument(webContents(7, undefined, true), roles())).toBeNull();

    const test = harness();
    const contents = webContents();
    expect(test.checkHandler(contents, 'media', captureOrigin, checkDetails)).toBe(false);
    test.controller.authorize(contents.id, 'capture');
    let granted = true;
    test.requestHandler(
      contents,
      'camera',
      (value) => {
        granted = value;
      },
      requestDetails,
    );
    expect(granted).toBe(false);
  });

  it('removes every installed session policy idempotently', () => {
    const test = harness();
    test.dispose();
    test.dispose();

    expect(test.mocks.setPermissionRequestHandler).toHaveBeenLastCalledWith(null);
    expect(test.mocks.setPermissionCheckHandler).toHaveBeenLastCalledWith(null);
    expect(test.mocks.removeListener).toHaveBeenCalledOnce();
    expect(test.mocks.onBeforeRequest).toHaveBeenLastCalledWith(null);
    expect(test.mocks.onHeadersReceived).toHaveBeenLastCalledWith(null);
  });

  it('does not let a stale disposer clear a replacement policy', () => {
    const test = harness();
    const replacement = secureSession(test.target, null);
    const clearsAfterReplacement = test.mocks.setPermissionRequestHandler.mock.calls.length;

    test.dispose();
    expect(test.mocks.setPermissionRequestHandler).toHaveBeenCalledTimes(clearsAfterReplacement);
    replacement();
    expect(test.mocks.setPermissionRequestHandler).toHaveBeenCalledTimes(
      clearsAfterReplacement + 1,
    );
  });

  it.each([
    [captureOrigin, { ...checkDetails, isMainFrame: false }],
    [captureOrigin, { ...checkDetails, requestingUrl: 'talking-quill://app/main/index.html' }],
    ['https://attacker.invalid', checkDetails],
    [captureOrigin, { ...checkDetails, securityOrigin: 'https://attacker.invalid' }],
    [captureOrigin, { ...checkDetails, embeddingOrigin: captureOrigin }],
  ] as const)('denies mismatched check frame and origin context', (origin, details) => {
    const test = harness();
    const contents = webContents();
    test.controller.authorize(contents.id, 'capture');
    expect(test.checkHandler(contents, 'media', origin, details)).toBe(false);
  });
});
