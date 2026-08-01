import { desktopCapturer, screen, type Session, type WebContents } from 'electron';
import { SYSTEM_AUDIO_AUTHORIZATION_TTL_MS } from '../../shared/constants/audio';
import { sameSecurityOrigin } from './microphone-permission';

interface SystemAudioLease {
  readonly captureId: string;
  readonly processId: number;
  readonly routingId: number;
  readonly url: string;
  readonly origin: string;
  readonly expiresAt: number;
  consumed: boolean;
}

export interface SystemAudioPermissionRequest {
  readonly webContents: WebContents;
  readonly permission: string;
  readonly mediaTypes: readonly string[];
  readonly isMainFrame: boolean;
  readonly requestingUrl: string | null;
  readonly securityOrigin: string | null;
  readonly expectedUrl: string | null;
  readonly expectedOrigin: string | null;
}

export interface SystemAudioCapturePlatform {
  readonly supported: boolean;
  readonly getPrimaryDisplayId: () => string;
  readonly getScreenSources: () => Promise<readonly Electron.DesktopCapturerSource[]>;
}

/** Grants one tightly scoped Windows loopback request from the private capture renderer. */
export class SystemAudioCaptureController {
  readonly #session: Session;
  readonly #platform: SystemAudioCapturePlatform;
  readonly #now: () => number;
  #lease: SystemAudioLease | null = null;
  #disposed = false;

  constructor(
    target: Session,
    platform: SystemAudioCapturePlatform = createElectronSystemAudioPlatform(),
    now: () => number = Date.now,
  ) {
    this.#session = target;
    this.#platform = platform;
    this.#now = now;
    target.setDisplayMediaRequestHandler((request, callback) => {
      const lease = this.#takeMatchingLease(request);
      if (lease === null) {
        respondToDisplayRequest(callback, {});
        return;
      }
      void this.#platform.getScreenSources().then(
        (sources) => {
          if (!this.#grantIsCurrent(lease)) {
            respondToDisplayRequest(callback, {});
            return;
          }
          const primaryDisplayId = this.#platform.getPrimaryDisplayId();
          const source = sources.find((candidate) => candidate.display_id === primaryDisplayId);
          this.#lease = null;
          respondToDisplayRequest(
            callback,
            source === undefined ? {} : { video: source, audio: 'loopback' },
          );
        },
        () => {
          if (this.#lease === lease) this.#lease = null;
          respondToDisplayRequest(callback, {});
        },
      );
    });
  }

  get supported(): boolean {
    return this.#platform.supported;
  }

  allowsPermissionRequest(request: SystemAudioPermissionRequest): boolean {
    const lease = this.#currentLease();
    const isDisplayPermission =
      request.permission === 'display-capture' ||
      (request.permission === 'media' && request.mediaTypes.length === 0);
    if (lease === null || lease.consumed || !isDisplayPermission) return false;
    const frame = request.webContents.mainFrame;
    return (
      !request.webContents.isDestroyed() &&
      request.isMainFrame &&
      frame.processId === lease.processId &&
      frame.routingId === lease.routingId &&
      frame.url === lease.url &&
      request.requestingUrl === lease.url &&
      request.expectedUrl === lease.url &&
      sameSecurityOrigin(frame.origin, lease.origin) &&
      sameSecurityOrigin(request.securityOrigin, lease.origin) &&
      sameSecurityOrigin(request.expectedOrigin, lease.origin)
    );
  }

  authorize(webContents: WebContents, captureId: string): void {
    if (this.#disposed || !this.#platform.supported || webContents.isDestroyed()) {
      throw new Error('System audio capture is unavailable');
    }
    const frame = webContents.mainFrame;
    this.#lease = {
      captureId,
      processId: frame.processId,
      routingId: frame.routingId,
      url: frame.url,
      origin: frame.origin,
      expiresAt: this.#now() + SYSTEM_AUDIO_AUTHORIZATION_TTL_MS,
      consumed: false,
    };
  }

  release(captureId: string): void {
    if (this.#lease?.captureId === captureId) this.#lease = null;
  }

  releaseAll(): void {
    this.#lease = null;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#lease = null;
    this.#session.setDisplayMediaRequestHandler(null);
  }

  #takeMatchingLease(
    request: Electron.DisplayMediaRequestHandlerHandlerRequest,
  ): SystemAudioLease | null {
    const lease = this.#currentLease();
    if (lease === null || lease.consumed) return null;
    const frame = request.frame;
    if (
      !this.#platform.supported ||
      frame === null ||
      frame.isDestroyed() ||
      frame.parent !== null ||
      frame.processId !== lease.processId ||
      frame.routingId !== lease.routingId ||
      frame.url !== lease.url ||
      !sameSecurityOrigin(frame.origin, lease.origin) ||
      !sameSecurityOrigin(request.securityOrigin, lease.origin) ||
      !request.audioRequested ||
      !request.videoRequested
    ) {
      return null;
    }
    lease.consumed = true;
    return lease;
  }

  #grantIsCurrent(lease: SystemAudioLease): boolean {
    return this.#currentLease() === lease && lease.consumed;
  }

  #currentLease(): SystemAudioLease | null {
    const lease = this.#lease;
    if (this.#disposed || lease === null) return null;
    if (this.#now() < lease.expiresAt) return lease;
    this.#lease = null;
    return null;
  }
}

function respondToDisplayRequest(
  callback: (streams: Electron.Streams) => void,
  streams: Electron.Streams,
): void {
  try {
    callback(streams);
  } catch {
    // The requesting frame may disappear while desktop-source enumeration is in flight.
  }
}

function createElectronSystemAudioPlatform(): SystemAudioCapturePlatform {
  return {
    supported: process.platform === 'win32',
    getPrimaryDisplayId: () => String(screen.getPrimaryDisplay().id),
    getScreenSources: () =>
      desktopCapturer.getSources({
        types: ['screen'],
        thumbnailSize: { width: 0, height: 0 },
        fetchWindowIcons: false,
      }),
  };
}
