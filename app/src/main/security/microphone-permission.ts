import { shell, systemPreferences } from 'electron';
import { MICROPHONE_AUTHORIZATION_TTL_MS } from '../../shared/constants/audio';
import type { MicrophonePermissionState } from '../../shared/schemas/audio';

export const MICROPHONE_SETTINGS_URLS = Object.freeze({
  win32: 'ms-settings:privacy-microphone',
  darwin: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone',
});

export interface MicrophonePermissionPlatform {
  readonly platform: NodeJS.Platform;
  readonly getStatus: () => MicrophonePermissionState;
  readonly openExternal: (url: string) => Promise<void>;
}

export interface MicrophonePermissionRequest {
  readonly webContentsId: number;
  readonly permission: string;
  readonly mediaTypes: readonly string[];
  readonly isMainFrame: boolean;
  readonly requestingUrl: string | null;
  readonly requestingOrigin: string | null;
  readonly securityOrigin: string | null;
  readonly embeddingOrigin: string | null;
  readonly expectedUrl: string | null;
  readonly expectedOrigin: string | null;
}

interface AuthorizationLease {
  readonly webContentsId: number;
  readonly captureId: string;
  readonly mode: 'acquire' | 'enumerate';
  readonly expiresAt: number;
  remainingRequests: number;
}

export class MicrophonePermissionController {
  readonly #platform: MicrophonePermissionPlatform;
  readonly #now: () => number;
  #lease: AuthorizationLease | null = null;
  #lastKnownStatus: MicrophonePermissionState = 'not-determined';
  #lastPolicyDenialCaptureId: string | null = null;

  constructor(
    platform: MicrophonePermissionPlatform = createElectronPermissionPlatform(),
    now: () => number = Date.now,
  ) {
    this.#platform = platform;
    this.#now = now;
    this.#lastKnownStatus = platform.getStatus();
  }

  getStatus(): MicrophonePermissionState {
    const status = this.#platform.getStatus();
    if (status !== 'not-determined' || this.#lastKnownStatus === 'not-determined') {
      this.#lastKnownStatus = status;
    }
    return this.#lastKnownStatus;
  }

  authorize(webContentsId: number, captureId: string, acquisitionLimit = 1): void {
    if (!Number.isInteger(acquisitionLimit) || acquisitionLimit < 1 || acquisitionLimit > 2) {
      throw new Error('Invalid microphone acquisition limit');
    }
    this.#lease = {
      webContentsId,
      captureId,
      mode: 'acquire',
      expiresAt: this.#now() + MICROPHONE_AUTHORIZATION_TTL_MS,
      remainingRequests: acquisitionLimit,
    };
    this.#lastPolicyDenialCaptureId = null;
  }

  authorizeEnumeration(webContentsId: number, captureId: string): void {
    this.#lease = {
      webContentsId,
      captureId,
      mode: 'enumerate',
      expiresAt: this.#now() + MICROPHONE_AUTHORIZATION_TTL_MS,
      remainingRequests: 0,
    };
  }

  allowsCheck(request: MicrophonePermissionRequest): boolean {
    return this.#matchingLease(request)?.mode === 'enumerate';
  }

  allowsRequest(request: MicrophonePermissionRequest): boolean {
    const lease = this.#matchingLease(request);
    if (lease?.mode !== 'acquire' || lease.remainingRequests === 0) return false;
    lease.remainingRequests -= 1;
    this.#lastPolicyDenialCaptureId = null;
    this.#lastKnownStatus = 'granted';
    return true;
  }

  notePolicyDenied(request: MicrophonePermissionRequest): void {
    const lease = this.#lease;
    if (
      lease !== null &&
      request.webContentsId === lease.webContentsId &&
      !(
        lease.mode === 'acquire' &&
        lease.remainingRequests > 0 &&
        this.#matchesTrustContext(request, lease)
      )
    ) {
      this.#lastPolicyDenialCaptureId = lease.captureId;
    }
  }

  takePolicyDenial(captureId: string): boolean {
    if (this.#lastPolicyDenialCaptureId !== captureId) return false;
    this.#lastPolicyDenialCaptureId = null;
    return true;
  }

  seal(captureId: string): void {
    if (this.#lease?.captureId === captureId) this.#lease = null;
  }

  release(captureId: string): void {
    if (this.#lease?.captureId === captureId) this.#lease = null;
  }

  releaseAll(): void {
    this.#lease = null;
    this.#lastPolicyDenialCaptureId = null;
  }

  async openSettings(): Promise<void> {
    const platform = this.#platform.platform;
    if (platform !== 'win32' && platform !== 'darwin') {
      throw new Error('Microphone settings are unavailable on this platform');
    }
    await this.#platform.openExternal(MICROPHONE_SETTINGS_URLS[platform]);
  }

  #matchingLease(request: MicrophonePermissionRequest): AuthorizationLease | null {
    const lease = this.#lease;
    if (lease === null) return null;
    if (this.#now() > lease.expiresAt) {
      this.#lease = null;
      return null;
    }
    return this.#matchesTrustContext(request, lease) ? lease : null;
  }

  #matchesTrustContext(request: MicrophonePermissionRequest, lease: AuthorizationLease): boolean {
    return !(
      request.permission !== 'media' ||
      request.webContentsId !== lease.webContentsId ||
      request.mediaTypes.length !== 1 ||
      request.mediaTypes[0] !== 'audio' ||
      !request.isMainFrame ||
      request.expectedUrl === null ||
      request.expectedOrigin === null ||
      request.requestingUrl !== request.expectedUrl ||
      !sameSecurityOrigin(request.requestingOrigin, request.expectedOrigin) ||
      !sameSecurityOrigin(request.securityOrigin, request.expectedOrigin) ||
      request.embeddingOrigin !== null
    );
  }
}

export function sameSecurityOrigin(first: string | null, second: string | null): boolean {
  if (first === null || second === null) return false;
  try {
    const left = new URL(first);
    const right = new URL(second);
    return (
      left.protocol === right.protocol &&
      left.hostname === right.hostname &&
      effectivePort(left) === effectivePort(right)
    );
  } catch {
    return false;
  }
}

function effectivePort(url: URL): string {
  if (url.port.length > 0) return url.port;
  if (url.protocol === 'http:') return '80';
  if (url.protocol === 'https:') return '443';
  return '';
}

function createElectronPermissionPlatform(): MicrophonePermissionPlatform {
  return {
    platform: process.platform,
    getStatus: () => {
      if (process.platform !== 'win32' && process.platform !== 'darwin') return 'unavailable';
      try {
        const status = systemPreferences.getMediaAccessStatus('microphone');
        if (status === 'granted' || status === 'denied' || status === 'restricted') return status;
        return 'not-determined';
      } catch {
        return 'not-determined';
      }
    },
    openExternal: async (url) => {
      await shell.openExternal(url);
    },
  };
}
