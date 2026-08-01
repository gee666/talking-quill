import { describe, expect, it, vi } from 'vitest';
import {
  SystemAudioCaptureController,
  type SystemAudioCapturePlatform,
} from '../../app/src/main/security/system-audio-capture';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function harness(
  options: {
    readonly supported?: boolean;
    readonly getScreenSources?: () => Promise<readonly Electron.DesktopCapturerSource[]>;
    readonly now?: () => number;
  } = {},
) {
  let handler:
    | ((
        request: Electron.DisplayMediaRequestHandlerHandlerRequest,
        callback: (streams: Electron.Streams) => void,
      ) => void)
    | null = null;
  const target = {
    setDisplayMediaRequestHandler: vi.fn((next: typeof handler) => {
      handler = next;
    }),
  };
  const primary = { id: 'screen:primary', display_id: '42' } as Electron.DesktopCapturerSource;
  const secondary = { id: 'screen:secondary', display_id: '7' } as Electron.DesktopCapturerSource;
  const platform: SystemAudioCapturePlatform = {
    supported: options.supported ?? true,
    getPrimaryDisplayId: () => '42',
    getScreenSources: vi.fn(
      options.getScreenSources ?? (() => Promise.resolve([secondary, primary])),
    ),
  };
  const controller = new SystemAudioCaptureController(
    target as unknown as Electron.Session,
    platform,
    options.now ?? (() => 1_000),
  );
  const frame = {
    processId: 8,
    routingId: 9,
    url: 'talking-quill://app/capture/index.html',
    origin: 'talking-quill://app',
    parent: null,
    isDestroyed: () => false,
  } as unknown as Electron.WebFrameMain;
  const webContents = {
    isDestroyed: () => false,
    mainFrame: frame,
  } as unknown as Electron.WebContents;
  const request = {
    frame,
    securityOrigin: 'talking-quill://app/',
    videoRequested: true,
    audioRequested: true,
    userGesture: false,
  } satisfies Electron.DisplayMediaRequestHandlerHandlerRequest;
  return {
    controller,
    frame,
    handler: () => {
      if (handler === null) throw new Error('Display media handler is unavailable');
      return handler;
    },
    platform,
    primary,
    request,
    target,
    webContents,
  };
}

describe('SystemAudioCaptureController', () => {
  it('grants one exact capture-frame request and selects the primary Windows display', async () => {
    const test = harness();
    test.controller.authorize(test.webContents, 'capture-1');
    expect(
      test.controller.allowsPermissionRequest({
        webContents: test.webContents,
        permission: 'media',
        mediaTypes: [],
        isMainFrame: true,
        requestingUrl: test.frame.url,
        securityOrigin: 'talking-quill://app/',
        expectedUrl: test.frame.url,
        expectedOrigin: test.frame.origin,
      }),
    ).toBe(true);
    const callback = vi.fn<(streams: Electron.Streams) => void>();

    test.handler()(test.request, callback);
    expect(
      test.controller.allowsPermissionRequest({
        webContents: test.webContents,
        permission: 'media',
        mediaTypes: [],
        isMainFrame: true,
        requestingUrl: test.frame.url,
        securityOrigin: 'talking-quill://app/',
        expectedUrl: test.frame.url,
        expectedOrigin: test.frame.origin,
      }),
    ).toBe(false);
    await vi.waitFor(() =>
      expect(callback).toHaveBeenCalledWith({ video: test.primary, audio: 'loopback' }),
    );

    const reused = vi.fn<(streams: Electron.Streams) => void>();
    test.handler()(test.request, reused);
    expect(reused).toHaveBeenCalledWith({});
    test.controller.dispose();
    expect(test.target.setDisplayMediaRequestHandler).toHaveBeenLastCalledWith(null);
  });

  it('rejects an untrusted frame without consuming the authorized request', async () => {
    const test = harness();
    test.controller.authorize(test.webContents, 'capture-2');
    const rejected = vi.fn<(streams: Electron.Streams) => void>();
    test.handler()({ ...test.request, securityOrigin: 'talking-quill://attacker' }, rejected);
    expect(rejected).toHaveBeenCalledWith({});

    const allowed = vi.fn<(streams: Electron.Streams) => void>();
    test.handler()(test.request, allowed);
    await vi.waitFor(() =>
      expect(allowed).toHaveBeenCalledWith(
        expect.objectContaining({
          audio: 'loopback',
        }),
      ),
    );
    test.controller.dispose();
  });

  it('revokes a consumed request while source enumeration is pending', async () => {
    const sources = deferred<readonly Electron.DesktopCapturerSource[]>();
    const test = harness({ getScreenSources: () => sources.promise });
    test.controller.authorize(test.webContents, 'capture-pending');
    const callback = vi.fn<(streams: Electron.Streams) => void>();
    test.handler()(test.request, callback);
    test.controller.release('capture-pending');
    sources.resolve([test.primary]);

    await vi.waitFor(() => expect(callback).toHaveBeenCalledWith({}));
    test.controller.dispose();
  });

  it('fails closed when the exact primary display is unavailable', async () => {
    const test = harness({
      getScreenSources: () =>
        Promise.resolve([{ id: 'secondary', display_id: '7' } as Electron.DesktopCapturerSource]),
    });
    test.controller.authorize(test.webContents, 'capture-secondary-only');
    const callback = vi.fn<(streams: Electron.Streams) => void>();
    test.handler()(test.request, callback);
    await vi.waitFor(() => expect(callback).toHaveBeenCalledWith({}));
    test.controller.dispose();
  });

  it('fails closed when loopback capture is unsupported', () => {
    const test = harness({ supported: false });
    expect(() => test.controller.authorize(test.webContents, 'capture-3')).toThrow(
      'System audio capture is unavailable',
    );
    const callback = vi.fn<(streams: Electron.Streams) => void>();
    test.handler()(test.request, callback);
    expect(callback).toHaveBeenCalledWith({});
    test.controller.dispose();
  });
});
